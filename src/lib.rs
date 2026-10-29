pub mod cli;
pub mod config;
pub mod endpoint;
pub mod http;
pub mod meta;
pub mod output;
pub mod pipeline;
pub mod rename;
pub mod retrieval;

use std::io::Write;
use std::path::PathBuf;

use anyhow::{Context, Result};
use clap::Parser;
use sonic_rs::json;

use crate::cli::{Cli, DumpType, EffectiveSearch, SearchType};
use crate::endpoint::{Direction, Endpoint};
use crate::pipeline::{DocWindow, FileSink, Sink};

/// A usage error: reported with exit status 2.
#[derive(Debug)]
pub struct UsageError(pub String);

impl std::fmt::Display for UsageError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(&self.0)
    }
}

impl std::error::Error for UsageError {}

fn usage(msg: impl Into<String>) -> anyhow::Error {
    anyhow::Error::new(UsageError(msg.into()))
}

/// Exit status for an error returned by [`run`].
/// `--parseExtraFields a,_b`: names (leading `_` dropped) must match
/// `^[a-z][a-z0-9_]*$` so they can never inject keys into the bulk action
/// line (INJ-F4). Duplicates are dropped.
fn parse_extra_fields(raw: &str) -> Result<Vec<String>> {
    let mut out: Vec<String> = Vec::new();
    for f in raw.split(',').map(|f| f.trim().trim_start_matches('_')) {
        if f.is_empty() {
            continue;
        }
        let mut chars = f.chars();
        let ok = chars.next().is_some_and(|c| c.is_ascii_lowercase())
            && chars.all(|c| c.is_ascii_lowercase() || c.is_ascii_digit() || c == '_');
        if !ok {
            return Err(usage(format!(
                "--parseExtraFields: invalid field name {f:?} (allowed: a-z, 0-9 and _, starting with a letter)"
            )));
        }
        if !out.iter().any(|o| o == f) {
            out.push(f.to_string());
        }
    }
    Ok(out)
}

pub fn exit_code(error: &anyhow::Error) -> i32 {
    if error.downcast_ref::<UsageError>().is_some() {
        2
    } else {
        1
    }
}

fn init_logging(cli: &Cli) {
    let default = if cli.debug() {
        "debug"
    } else if cli.quiet() {
        "error"
    } else {
        "warn"
    };
    let _ = env_logger::Builder::from_env(env_logger::Env::default().default_filter_or(default))
        .format_target(false)
        .try_init();
}

fn validate(cli: &Cli, input: &Endpoint, output: &Endpoint) -> Result<()> {
    if let Some(extra) = &cli.parse_extra_fields {
        parse_extra_fields(extra)?;
    }
    if cli.file_size.is_some() && cli.max_rows.is_some() {
        return Err(usage("--fileSize and --maxRows cannot be combined"));
    }
    if (cli.file_size.is_some() || cli.max_rows.is_some()) && !matches!(output, Endpoint::File(_)) {
        return Err(usage("--fileSize/--maxRows require a file --output"));
    }
    if !input.is_es() && !output.is_es() && cli.r#type != DumpType::Data {
        return Err(usage(format!(
            "--type {} needs Elasticsearch/OpenSearch on at least one side",
            cli.r#type.name()
        )));
    }
    let renaming = !cli.rename_prefix.is_empty() || !cli.rename_index.is_empty();
    if renaming {
        match output {
            Endpoint::Es(ep) if ep.index.is_some() => {
                return Err(usage(format!(
                    "--renamePrefix/--renameIndex rename source index names and cannot be combined with an explicit output index ('{}'); use a host-only --output",
                    ep.index.as_deref().unwrap_or_default()
                )));
            }
            Endpoint::Es(_) => {}
            _ => {
                return Err(usage(
                    "--renamePrefix/--renameIndex apply when restoring or copying to a cluster, not to file output",
                ));
            }
        }
        renamer(cli)?;
    } else if cli.rename_aliases.unwrap_or(false) || cli.rename_patterns.unwrap_or(false) {
        return Err(usage(
            "--renameAliases/--renamePatterns need --renamePrefix or --renameIndex rules",
        ));
    }
    if cli.scroll_id.is_some()
        && (cli.slices > 1 || cli.effective_search() != EffectiveSearch::Scroll)
    {
        return Err(usage(
            "--scrollId cannot be combined with --slices, --searchAfter or PIT",
        ));
    }
    if cli.r#type != DumpType::Data && (cli.offset > 0 || cli.size >= 0) {
        log::warn!(
            "--offset/--size are ignored for --type {}",
            cli.r#type.name()
        );
    }
    Ok(())
}

fn file_path(ep: &Endpoint) -> Option<PathBuf> {
    match ep {
        Endpoint::File(p) => Some(p.clone()),
        _ => None,
    }
}

const DEFAULT_MAX_BULK_BYTES: u64 = 100 * 1024 * 1024;

/// `--throttleInterval`: only when given explicitly. Node's 1 ms default
/// would add ~10 s per million documents at --limit 100 for no benefit.
fn throttle(cli: &Cli) -> Option<std::time::Duration> {
    cli.throttle_interval
        .filter(|ms| *ms > 0)
        .map(std::time::Duration::from_millis)
}

fn split_requested(cli: &Cli) -> bool {
    cli.file_size.as_deref().is_some_and(|s| s.trim() != "-1")
        || cli.max_rows.is_some_and(|n| n > 0)
}

/// File/stdout output options: gzip and splitting.
fn file_options(cli: &Cli, output: &Endpoint) -> Result<pipeline::file_sink::FileOptions> {
    use crate::cli::CompressionType;
    use pipeline::file_sink::Codec;
    use pipeline::file_sink::{FileOptions, SplitBy, parse_size};
    let kind = match (cli.compression_type, cli.fs_compress()) {
        (Some(t), _) => Some(t),
        (None, true) => Some(CompressionType::Gzip),
        (None, false) => None,
    };
    let codec = match (kind, cli.compression_level) {
        (None, Some(_)) => {
            log::warn!(
                "--compressionLevel has no effect without --fsCompress or --compressionType"
            );
            None
        }
        (None, None) => None,
        (Some(CompressionType::Gzip), None | Some(-1)) => Some(Codec::Gzip(6)),
        (Some(CompressionType::Gzip), Some(l @ 0..=9)) => Some(Codec::Gzip(l as u32)),
        (Some(CompressionType::Zstd), None | Some(-1)) => Some(Codec::Zstd(3)),
        (Some(CompressionType::Zstd), Some(l @ 1..=22)) => Some(Codec::Zstd(l)),
        (Some(t), Some(l)) => {
            let range = if t == CompressionType::Gzip {
                "0-9"
            } else {
                "1-22"
            };
            return Err(usage(format!(
                "--compressionLevel {l} is out of range for {t:?} (-1 or {range})"
            )));
        }
    };
    let human = match cli.format.as_deref().map(str::trim) {
        None | Some("") | Some("json") => false,
        Some("human") if split_requested(cli) => {
            return Err(usage(
                "--format human cannot be combined with --fileSize/--maxRows",
            ));
        }
        Some("human") => true,
        Some(other) => {
            return Err(usage(format!(
                "--format must be '' or 'human', got '{other}'"
            )));
        }
    };
    let path = file_path(output);
    if codec.is_some() && path.is_none() {
        log::info!("compression does not apply to stdout output (elasticdump behaviour)");
    }
    let split = match (&cli.file_size, cli.max_rows) {
        (Some(s), _) if s.trim() != "-1" => Some(SplitBy::Bytes(
            parse_size(s).map_err(|e| usage(format!("--fileSize: {e:#}")))?,
        )),
        (_, Some(n)) if n > 0 => Some(SplitBy::Rows(n)),
        _ => None,
    };
    Ok(FileOptions {
        path,
        overwrite: cli.overwrite(),
        source_only: cli.source_only(),
        codec,
        split,
        throttle: throttle(cli),
        human,
    })
}

async fn build_sink(
    cli: &Cli,
    output: &Endpoint,
    retry: crate::http::RetryPolicy,
    gate: crate::http::gate::RequestGate,
) -> Result<Sink> {
    match output {
        Endpoint::Es(ep) => {
            if cli.source_only() {
                log::warn!("--sourceOnly with an Elasticsearch output: documents get new ids");
            }
            let client = config::build_client(cli, ep, Direction::Output, gate)?;
            let info = client
                .info(retry, &tokio_util::sync::CancellationToken::new())
                .await?
                .clone();
            let mut meta_fields: Vec<String> = ["routing", "parent", "timestamp", "ttl"]
                .map(String::from)
                .to_vec();
            if let Some(extra) = &cli.parse_extra_fields {
                for f in parse_extra_fields(extra)? {
                    if !meta_fields.contains(&f) {
                        meta_fields.push(f);
                    }
                }
            }
            let max_bulk_bytes = match &cli.max_bulk_bytes {
                Some(raw) => raw
                    .parse::<bytesize::ByteSize>()
                    .map_err(|e| usage(format!("--maxBulkBytes: {e}")))?
                    .as_u64(),
                None => DEFAULT_MAX_BULK_BYTES,
            };
            let opts = pipeline::BulkOptions {
                info,
                index: ep.index.clone(),
                doc_type: ep.doc_type.clone(),
                prefix: cli.prefix.clone(),
                suffix: cli.suffix.clone(),
                action: pipeline::BulkAction::parse(&cli.bulk_action)?,
                handle_version: cli.handle_version(),
                version_type: cli.version_type.clone(),
                meta_fields,
                refresh: !cli.no_refresh(),
                ignore_es_write_errors: cli.ignore_es_write_errors(),
                ignore_errors: cli.ignore_errors(),
                max_bulk_bytes: max_bulk_bytes.max(1) as usize,
                retry,
                rename: renamer(cli)?.map(std::sync::Arc::new),
            };
            Ok(Sink::Bulk(Box::new(
                pipeline::BulkSink::new(client, opts, tokio_util::sync::CancellationToken::new())
                    .with_concurrency(cli.concurrency, throttle(cli)),
            )))
        }
        out => {
            if cli.concurrency > 1 {
                log::info!(
                    "--concurrency applies to Elasticsearch outputs; file output stays sequential to keep order"
                );
            }
            Ok(Sink::File(FileSink::create(file_options(cli, out)?).await?))
        }
    }
}

/// `--searchWithTemplate`: render `--searchBody` (`{"id"|"source", "params"}`)
/// with `POST /_render/template` and search with the rendered body.
async fn render_search_template(
    client: &crate::http::EsClient,
    retry: crate::http::RetryPolicy,
    template: &sonic_rs::Value,
) -> Result<sonic_rs::Value> {
    use sonic_rs::JsonValueTrait;
    if template.get("id").is_none() && template.get("source").is_none() {
        return Err(usage(
            "--searchWithTemplate needs a --searchBody with \"id\" (stored template) or \"source\", plus \"params\"",
        ));
    }
    let body: serde_json::Value = serde_json::from_str(&sonic_rs::to_string(template)?)?;
    let body = crate::http::Body::json(&body);
    let body = &body;
    let cancel = tokio_util::sync::CancellationToken::new();
    let resp = crate::http::send_with_retry(
        retry,
        &cancel,
        crate::http::RetryMode::Idempotent,
        "render search template",
        move || {
            client.send(
                reqwest::Method::POST,
                &["_render", "template"],
                &[],
                true,
                body.clone(),
            )
        },
    )
    .await
    .context("--searchWithTemplate: rendering the search template failed")?;
    let v: sonic_rs::Value = sonic_rs::from_slice(&resp)?;
    let out = v
        .get("template_output")
        .cloned()
        .ok_or_else(|| anyhow::anyhow!("_render/template returned no template_output"))?;
    log::debug!("Rendered search template: {}", sonic_rs::to_string(&out)?);
    Ok(out)
}

async fn es_source_options(
    cli: &Cli,
    ep: &endpoint::EsEndpoint,
    retry: crate::http::RetryPolicy,
    gate: crate::http::gate::RequestGate,
) -> Result<retrieval::EsSourceOptions> {
    let client = config::build_client(cli, ep, Direction::Input, gate)?;
    let mut search_type = match cli.effective_search() {
        EffectiveSearch::Scroll => SearchType::Scroll,
        EffectiveSearch::Pit => SearchType::PointInTime,
        EffectiveSearch::SearchAfter => SearchType::SearchAfter,
    };
    // Cluster detection (`GET /`) is only needed for PIT; scroll dumps work
    // the same on every flavor and version.
    let (pit_api, pit_sort) = if search_type == SearchType::PointInTime {
        let info = client
            .info(retry, &tokio_util::sync::CancellationToken::new())
            .await?
            .clone();
        match info.pit_api() {
            Some(_) if cli.slices > 1 && !info.supports_sliced_pit() => {
                log::warn!(
                    "{} {} cannot slice a point-in-time search (needs Elasticsearch >= 7.15); using sliced scroll instead",
                    info.flavor_name(),
                    info.version
                );
                search_type = SearchType::Scroll;
                (crate::http::PitApi::Elastic, json!(["_shard_doc"]))
            }
            Some(api) => {
                let sort = if info.supports_shard_doc_sort() {
                    json!(["_shard_doc"])
                } else {
                    // `_doc` alone loses documents across PIT pages on
                    // OpenSearch; `_id` is the tiebreaker. `unmapped_type`
                    // keeps indices without any mapping searchable.
                    json!([{"_doc": "asc"}, {"_id": {"order": "asc", "unmapped_type": "keyword"}}])
                };
                (api, sort)
            }
            None => {
                // ES < 7.10 and OpenSearch < 2.4 have no PIT: scroll gives the
                // same snapshot-consistent result.
                log::warn!(
                    "{} {}{} does not support point-in-time search (needs Elasticsearch >= 7.10 with X-Pack, or OpenSearch >= 2.4); using scroll instead",
                    info.flavor_name(),
                    info.version,
                    if info.oss { " (OSS build)" } else { "" }
                );
                search_type = SearchType::Scroll;
                (crate::http::PitApi::Elastic, json!(["_shard_doc"]))
            }
        }
    } else {
        (crate::http::PitApi::Elastic, json!(["_shard_doc"]))
    };
    let size = (cli.size >= 0).then_some(cli.size as u64 + cli.offset);
    let search_body = if cli.search_with_template() {
        let template = retrieval::search_body::load_search_body(cli.search_body.as_deref()).await?;
        let rendered = render_search_template(&client, retry, &template).await?;
        retrieval::search_body::finalize_search_body(rendered, cli.limit, size)
    } else {
        retrieval::search_body::prepare_search_body(cli.search_body.as_deref(), cli.limit, size)
            .await?
    };
    if search_type == SearchType::SearchAfter {
        use sonic_rs::{JsonContainerTrait, JsonValueTrait};
        let has_sort = search_body
            .get("sort")
            .is_some_and(|s| s.as_array().is_none_or(|a| !a.is_empty()));
        if !has_sort {
            return Err(usage(
                "--searchAfter without --pit needs a \"sort\" in --searchBody ending in a unique tiebreaker",
            ));
        }
        if cli.slices > 1 {
            return Err(usage(
                "--slices needs scroll or PIT; it cannot be used with --searchAfter alone",
            ));
        }
        log::warn!(
            "--searchAfter without --pit is not snapshot-consistent; concurrent writes may be missed or duplicated"
        );
    }
    Ok(retrieval::EsSourceOptions {
        client,
        index: ep.index.clone(),
        search_body,
        search_type,
        pit_api,
        pit_sort,
        scroll_ttl: cli.scroll.clone(),
        pit_keep_alive: cli.pit_keep_alive.clone(),
        slices: cli.slices,
        workers: cli.workers,
        buffer_size: cli.buffer_size,
        quiet: cli.quiet(),
        retry,
        resume_scroll_id: cli.scroll_id.clone(),
    })
}

/// `--renamePrefix`/`--renameIndex` rules (with `--prefix`/`--suffix`), if any.
fn renamer(cli: &Cli) -> Result<Option<rename::IndexRenamer>> {
    if cli.rename_prefix.is_empty() && cli.rename_index.is_empty() {
        return Ok(None);
    }
    rename::IndexRenamer::new(
        &cli.rename_prefix,
        &cli.rename_index,
        &cli.prefix,
        &cli.suffix,
    )
    .map(Some)
    .map_err(|e| usage(format!("{e:#}")))
}

fn csv_list(raw: Option<&str>) -> Vec<String> {
    raw.map(|s| {
        s.split(',')
            .map(|p| p.trim().to_string())
            .filter(|p| !p.is_empty())
            .collect()
    })
    .unwrap_or_default()
}

async fn run_meta(
    cli: &Cli,
    kind: meta::MetaKind,
    input: &Endpoint,
    output: &Endpoint,
    retry: crate::http::RetryPolicy,
    gate: crate::http::gate::RequestGate,
) -> Result<pipeline::WriteReport> {
    let cancel = tokio_util::sync::CancellationToken::new();
    let (src_client, src_info, src_index, src_doc_type) = match input {
        Endpoint::Es(ep) => {
            let c = config::build_client(cli, ep, Direction::Input, gate.clone())?;
            let info = c.info(retry, &cancel).await?.clone();
            (Some(c), Some(info), ep.index.clone(), ep.doc_type.clone())
        }
        _ => (None, None, None, None),
    };
    let template_filter = match (kind, cli.filter_system_templates()) {
        (meta::MetaKind::Template(_), true) => Some(regex::Regex::new(&cli.template_regex).map_err(|e| {
            usage(format!(
                "--templateRegex: unsupported pattern (JavaScript look-around and backreferences are not available): {e}"
            ))
        })?),
        _ => None,
    };
    if cli.close_index.unwrap_or(false) && kind != meta::MetaKind::Analyzer {
        log::warn!("--closeIndex only applies to --type analyzer");
    }
    let mut opts = meta::MetaOptions::new(kind, retry);
    opts.skip_existing = cli.skip_existing();
    opts.ignore_errors = cli.ignore_errors();
    opts.exclude_settings = csv_list(cli.exclude_settings.as_deref());
    // Engine-specific settings and fields are stripped automatically only for
    // cluster→cluster copies, where both flavors are known.
    opts.source_info = match (&src_info, output) {
        (Some(s), Endpoint::Es(_)) => Some(s.clone()),
        _ => None,
    };
    opts.object_format = cli.meta_format == "object";
    opts.template_filter = template_filter;
    opts.filter_managed = cli.filter_managed();
    opts.close_index = cli.close_index.unwrap_or(false);
    opts.exclude_template_fields = csv_list(cli.exclude_template_fields.as_deref());
    opts.convert_ilm_to_ism = cli.convert_ilm_to_ism.unwrap_or(false);
    opts.lenient_policy_conversion = cli.lenient_policy_conversion.unwrap_or(false);
    opts.rename_aliases = cli.rename_aliases.unwrap_or(false);
    opts.rename_patterns = cli.rename_patterns.unwrap_or(false);
    if opts.lenient_policy_conversion && !opts.convert_ilm_to_ism {
        log::warn!("--lenientPolicyConversion has no effect without --convertIlmToIsm");
    }
    let src = meta::MetaSource {
        client: src_client,
        info: src_info,
        index: src_index,
        doc_type: src_doc_type,
        file: file_path(input),
    };
    // read (and validate) the source before touching the target
    let doc = meta::read(&src, &opts).await?;
    let (dst_client, dst_info, dst_ep) = match output {
        Endpoint::Es(ep) => {
            let c = config::build_client(cli, ep, Direction::Output, gate)?;
            let info = c.info(retry, &cancel).await?.clone();
            (Some(c), Some(info), Some(ep))
        }
        _ => (None, None, None),
    };
    let renamer = match renamer(cli)? {
        Some(r) => r,
        None => rename::IndexRenamer::new(&[], &[], &cli.prefix, &cli.suffix)?,
    };
    let dst = meta::MetaTarget {
        client: dst_client,
        info: dst_info,
        index: dst_ep.and_then(|e| e.index.clone()),
        doc_type: dst_ep.and_then(|e| e.doc_type.clone()),
        renamer,
        file: file_options(cli, output)?,
    };
    meta::write(doc, &dst, &opts).await
}

/// `--type alias` with a two-segment `--input` URL and no `--input-index`:
/// elasticdump reads `host/index/alias`; elasticdump-rs would read a base
/// path. Follow elasticdump, with a warning.
fn node_alias_url(cli: &Cli, input: &mut Endpoint) {
    let Endpoint::Es(ep) = input else { return };
    if cli.input_index.is_some() {
        return;
    }
    let segs: Vec<String> = ep
        .base
        .path_segments()
        .map(|s| s.filter(|x| !x.is_empty()).map(String::from).collect())
        .unwrap_or_default();
    if segs.len() != 1 || ep.index.is_none() {
        return;
    }
    let index = segs[0].clone();
    let alias = ep.index.take();
    log::warn!(
        "--type alias: reading the --input URL path as <index>/<alias> ('{index}' / '{}') like elasticdump; use --input-index {index}/{} to say so explicitly",
        alias.as_deref().unwrap_or_default(),
        alias.as_deref().unwrap_or_default()
    );
    ep.base.set_path("/");
    ep.index = Some(index);
    ep.doc_type = alias;
}

pub async fn run() -> Result<()> {
    let argv: Vec<String> = std::env::args().collect();
    let (argv, oos_warnings) = cli::prescan_out_of_scope(argv).map_err(usage)?;
    let cli = Cli::parse_from(argv);
    init_logging(&cli);
    for w in &oos_warnings {
        log::warn!("{w}");
    }
    if !cli.quiet() {
        console::set_colors_enabled(true);
    }

    let mut input = endpoint::parse(&cli.input, Direction::Input, cli.input_index.as_deref())
        .map_err(|e| usage(format!("{e:#}")))?;
    let mut output = endpoint::parse(&cli.output, Direction::Output, cli.output_index.as_deref())
        .map_err(|e| usage(format!("{e:#}")))?;
    if cli.r#type == DumpType::Meta(meta::MetaKind::Alias) {
        node_alias_url(&cli, &mut input);
    }
    validate(&cli, &input, &output)?;

    let auth_file = match &cli.http_auth_file {
        Some(p) => Some(endpoint::read_auth_file(p)?),
        None => None,
    };
    if let Endpoint::Es(ep) = &mut input {
        config::resolve_endpoint_auth(&cli, ep, Direction::Input, auth_file.as_ref());
    }
    if let Endpoint::Es(ep) = &mut output {
        config::resolve_endpoint_auth(&cli, ep, Direction::Output, auth_file.as_ref());
    }

    log::info!(
        "Starting dump: {} -> {} (type {})",
        endpoint::redacted_endpoint(&cli.input),
        endpoint::redacted_endpoint(&cli.output),
        cli.r#type.name()
    );

    let retry = config::retry_policy(&cli)?;
    let gate = config::request_gate(&cli);
    let window = DocWindow::new(cli.offset, cli.size);

    let report = match cli.r#type {
        DumpType::Data => {
            let sink = build_sink(&cli, &output, retry, gate.clone()).await?;
            match &input {
                Endpoint::Es(ep) => {
                    let opts = es_source_options(&cli, ep, retry, gate.clone()).await?;
                    retrieval::dump_data(opts, sink, window).await?
                }
                inp => {
                    pipeline::file_to_sink(
                        file_path(inp),
                        cli.limit,
                        window,
                        sink,
                        tokio_util::sync::CancellationToken::new(),
                    )
                    .await?
                }
            }
        }
        DumpType::Meta(kind) => run_meta(&cli, kind, &input, &output, retry, gate.clone()).await?,
    };

    if !cli.quiet() {
        let mut err = std::io::stderr().lock();
        let _ = writeln!(err, "Total Writes: {}", report.written);
        if report.failed > 0 {
            let _ = writeln!(err, "Failed Writes: {}", report.failed);
        }
        let _ = writeln!(err, "dump complete");
    }
    if report.rejected > 0 {
        // A malformed line means the input framing is broken (INJ-F1):
        // never report success, even with --ignore-errors.
        return Err(anyhow::anyhow!(
            "{} document line(s) were rejected because they were not exactly one JSON object; nothing was written for them",
            report.rejected
        ));
    }
    Ok(())
}

pub mod cli;
pub mod config;
pub mod endpoint;
pub mod http;
pub mod meta;
pub mod output;
pub mod pipeline;
pub mod retrieval;

use std::io::Write;
use std::path::PathBuf;

use anyhow::{Result, bail};
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
    if cli.file_size.is_some() && cli.max_rows.is_some() {
        return Err(usage("--fileSize and --maxRows cannot be combined"));
    }
    if (cli.file_size.is_some() || cli.max_rows.is_some()) && !matches!(output, Endpoint::File(_)) {
        return Err(usage("--fileSize/--maxRows require a file --output"));
    }
    if !input.is_es() && !output.is_es() && cli.r#type != DumpType::Data {
        return Err(usage(
            format!(
                "--type {:?} needs Elasticsearch/OpenSearch on at least one side",
                cli.r#type
            )
            .to_lowercase(),
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
        log::warn!("--offset/--size are ignored for --type {:?}", cli.r#type);
    }
    Ok(())
}

fn not_yet(feature: &str) -> anyhow::Error {
    usage(format!("{feature} is not implemented yet in this build"))
}

fn file_path(ep: &Endpoint) -> Option<PathBuf> {
    match ep {
        Endpoint::File(p) => Some(p.clone()),
        _ => None,
    }
}

const DEFAULT_MAX_BULK_BYTES: u64 = 100 * 1024 * 1024;

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
                meta_fields.extend(
                    extra
                        .split(',')
                        .map(|f| f.trim().trim_start_matches('_').to_string())
                        .filter(|f| !f.is_empty()),
                );
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
            };
            Ok(Sink::Bulk(Box::new(pipeline::BulkSink::new(
                client,
                opts,
                tokio_util::sync::CancellationToken::new(),
            ))))
        }
        out => Ok(Sink::File(FileSink::create(file_options(cli, out)?).await?)),
    }
}

async fn es_source_options(
    cli: &Cli,
    ep: &endpoint::EsEndpoint,
    retry: crate::http::RetryPolicy,
    gate: crate::http::gate::RequestGate,
) -> Result<retrieval::EsSourceOptions> {
    let client = config::build_client(cli, ep, Direction::Input, gate)?;
    let search_type = match cli.effective_search() {
        EffectiveSearch::Scroll => SearchType::Scroll,
        EffectiveSearch::Pit => SearchType::PointInTime,
        EffectiveSearch::SearchAfter => SearchType::SearchAfter,
    };
    if cli.search_with_template() {
        return Err(not_yet("--searchWithTemplate"));
    }
    // Cluster detection (`GET /`) is only needed for PIT; scroll dumps work
    // the same on every flavor and version.
    let (pit_api, pit_sort) = if search_type == SearchType::PointInTime {
        let info = client
            .info(retry, &tokio_util::sync::CancellationToken::new())
            .await?
            .clone();
        let Some(api) = info.pit_api() else {
            bail!(
                "{} {} does not support point-in-time search",
                info.flavor_name(),
                info.version
            );
        };
        let sort = if info.supports_shard_doc_sort() {
            json!(["_shard_doc"])
        } else {
            // `_doc` alone loses documents across PIT pages on OpenSearch;
            // `_id` is the tiebreaker. `unmapped_type` keeps indices without
            // any mapping searchable.
            json!([{"_doc": "asc"}, {"_id": {"order": "asc", "unmapped_type": "keyword"}}])
        };
        (api, sort)
    } else {
        (crate::http::PitApi::Elastic, json!(["_shard_doc"]))
    };
    let size = (cli.size >= 0).then_some(cli.size as u64 + cli.offset);
    let search_body =
        retrieval::search_body::prepare_search_body(cli.search_body.as_deref(), cli.limit, size)
            .await?;
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

async fn run_meta(
    cli: &Cli,
    input: &Endpoint,
    output: &Endpoint,
    retry: crate::http::RetryPolicy,
    gate: crate::http::gate::RequestGate,
) -> Result<pipeline::WriteReport> {
    let kind = if cli.r#type == DumpType::Mapping {
        meta::MetaKind::Mapping
    } else {
        meta::MetaKind::Settings
    };
    let cancel = tokio_util::sync::CancellationToken::new();
    let (src_client, src_index) = match input {
        Endpoint::Es(ep) => (
            Some(config::build_client(
                cli,
                ep,
                Direction::Input,
                gate.clone(),
            )?),
            ep.index.clone(),
        ),
        _ => (None, None),
    };
    let (dst_client, dst_info, dst_ep) = match output {
        Endpoint::Es(ep) => {
            let c = config::build_client(cli, ep, Direction::Output, gate)?;
            let info = c.info(retry, &cancel).await?.clone();
            (Some(c), Some(info), Some(ep))
        }
        _ => (None, None, None),
    };
    // Engine-specific settings are stripped automatically only for
    // cluster→cluster copies, where both flavors are known.
    let source_info = match (&src_client, &dst_client) {
        (Some(s), Some(_)) if kind == meta::MetaKind::Settings => {
            Some(s.info(retry, &cancel).await?.clone())
        }
        _ => None,
    };
    let opts = meta::MetaOptions {
        kind,
        retry,
        skip_existing: cli.skip_existing(),
        ignore_errors: cli.ignore_errors(),
        exclude_settings: cli
            .exclude_settings
            .as_deref()
            .map(|s| {
                s.split(',')
                    .map(|p| p.trim().to_string())
                    .filter(|p| !p.is_empty())
                    .collect()
            })
            .unwrap_or_default(),
        source_info,
    };
    let src = meta::MetaSource {
        client: src_client,
        index: src_index,
        file: file_path(input),
    };
    let doc = meta::read(&src, &opts).await?;
    let dst = meta::MetaTarget {
        client: dst_client,
        info: dst_info,
        index: dst_ep.and_then(|e| e.index.clone()),
        doc_type: dst_ep.and_then(|e| e.doc_type.clone()),
        prefix: cli.prefix.clone(),
        suffix: cli.suffix.clone(),
        file: file_options(cli, output)?,
    };
    meta::write(doc, &dst, &opts).await
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
        "Starting dump: {} -> {} (type {:?})",
        endpoint::redacted_endpoint(&cli.input),
        endpoint::redacted_endpoint(&cli.output),
        cli.r#type
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
        DumpType::Mapping | DumpType::Settings => {
            run_meta(&cli, &input, &output, retry, gate.clone()).await?
        }
    };

    if !cli.quiet() {
        let mut err = std::io::stderr().lock();
        let _ = writeln!(err, "Total Writes: {}", report.written);
        if report.failed > 0 {
            let _ = writeln!(err, "Failed Writes: {}", report.failed);
        }
        let _ = writeln!(err, "dump complete");
    }
    Ok(())
}

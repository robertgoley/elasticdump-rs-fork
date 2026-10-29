pub mod cli;
pub mod config;
pub mod endpoint;
pub mod http;
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

    let written = match (cli.r#type, &input, &output) {
        (DumpType::Data, _, Endpoint::Es(_)) => return Err(not_yet("Writing to Elasticsearch")),
        (DumpType::Data, Endpoint::Es(ep), out) => {
            let client = config::build_client(&cli, ep, Direction::Input, gate.clone())?;
            let search_type = match cli.effective_search() {
                EffectiveSearch::Scroll => SearchType::Scroll,
                EffectiveSearch::Pit => SearchType::PointInTime,
                EffectiveSearch::SearchAfter => {
                    return Err(not_yet("--searchAfter without --pit"));
                }
            };
            if cli.scroll_id.is_some() {
                return Err(not_yet("--scrollId"));
            }
            if cli.search_with_template() {
                return Err(not_yet("--searchWithTemplate"));
            }
            // Cluster detection (`GET /`) is only needed for PIT; scroll dumps
            // work the same on every flavor and version.
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
                    // `_doc` alone loses documents across PIT pages on
                    // OpenSearch; `_id` is the tiebreaker. `unmapped_type`
                    // keeps indices without any mapping searchable.
                    json!([{"_doc": "asc"}, {"_id": {"order": "asc", "unmapped_type": "keyword"}}])
                };
                (api, sort)
            } else {
                (crate::http::PitApi::Elastic, json!(["_shard_doc"]))
            };
            let size = (cli.size >= 0).then_some(cli.size as u64 + cli.offset);
            let search_body = retrieval::search_body::prepare_search_body(
                cli.search_body.as_deref(),
                cli.limit,
                size,
            )
            .await?;
            let sink = Sink::File(
                FileSink::create(file_path(out), cli.overwrite(), cli.source_only()).await?,
            );
            let opts = retrieval::EsSourceOptions {
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
            };
            retrieval::dump_data(opts, sink, window).await?
        }
        (DumpType::Data, inp, out) => {
            let sink = Sink::File(
                FileSink::create(file_path(out), cli.overwrite(), cli.source_only()).await?,
            );
            pipeline::file_to_sink(
                file_path(inp),
                cli.limit,
                window,
                sink,
                tokio_util::sync::CancellationToken::new(),
            )
            .await?
        }
        (DumpType::Mapping | DumpType::Settings, _, _) => {
            return Err(not_yet("--type mapping/settings"));
        }
    };

    if !cli.quiet() {
        let mut err = std::io::stderr().lock();
        let _ = writeln!(err, "Total Writes: {written}");
        let _ = writeln!(err, "dump complete");
    }
    Ok(())
}

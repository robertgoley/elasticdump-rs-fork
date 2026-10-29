//! `--type alias`: `GET /{index}/_alias/{name}` and one atomic
//! `POST /_aliases` with an `add` action per index and alias.

use anyhow::{Result, bail};
use reqwest::Method;
use serde_json::{Map, Value, json};

use super::http::{get_json, send_json};
use super::{MetaOptions, MetaTarget};
use crate::http::EsClient;
use crate::pipeline::WriteReport;

/// `index` defaults to `*` (elasticdump requests `/undefined/_alias/*` for a
/// host-only input).
pub async fn fetch(
    client: &EsClient,
    index: Option<&str>,
    alias: Option<&str>,
    opts: &MetaOptions,
) -> Result<Value> {
    let index = index.unwrap_or("*");
    let alias = alias.unwrap_or("*");
    get_json(
        client,
        opts.retry,
        &[index, "_alias", alias],
        &format!("GET {index}/_alias/{alias}"),
    )
    .await
}

/// `(target index, alias, options)` for every alias in the document.
pub fn actions(doc: &Value, dst: &MetaTarget, opts: &MetaOptions) -> Result<Vec<(String, Value)>> {
    let Value::Object(indices) = doc else {
        bail!("alias document must be a JSON object keyed by index");
    };
    let with_aliases: Vec<(&String, &Map<String, Value>)> = indices
        .iter()
        .filter_map(
            |(i, def)| match def.get("aliases").and_then(Value::as_object) {
                Some(a) if !a.is_empty() => Some((i, a)),
                _ => {
                    log::debug!("Index '{i}' has no aliases; skipping it");
                    None
                }
            },
        )
        .collect();
    if dst.index.is_some() && with_aliases.len() > 1 {
        bail!(
            "the alias document holds aliases of {} indices but a single output index was given; use a host-only --output",
            with_aliases.len()
        );
    }
    let mut out = Vec::new();
    for (index, aliases) in with_aliases {
        let target = dst.target_index(index)?;
        for (alias, options) in aliases {
            let alias = if opts.rename_aliases {
                dst.renamer.alias(alias)?
            } else {
                alias.clone()
            };
            let mut add = Map::new();
            add.insert("index".into(), Value::String(target.clone()));
            add.insert("alias".into(), Value::String(alias));
            if let Value::Object(o) = options {
                for (k, v) in o {
                    add.insert(k.clone(), v.clone());
                }
            }
            out.push((target.clone(), json!({ "add": add })));
        }
    }
    Ok(out)
}

pub async fn write(
    doc: Value,
    client: &EsClient,
    dst: &MetaTarget,
    opts: &MetaOptions,
) -> Result<WriteReport> {
    let acts = actions(&doc, dst, opts)?;
    if acts.is_empty() {
        log::warn!("No aliases to restore");
        return Ok(WriteReport::default());
    }
    let body = json!({"actions": acts.iter().map(|(_, a)| a.clone()).collect::<Vec<_>>()});
    match send_json(
        client,
        opts.retry,
        Method::POST,
        &["_aliases"],
        &body,
        "update aliases",
    )
    .await
    {
        Ok(_) => Ok(WriteReport {
            written: acts.len() as u64,
            failed: 0,
        }),
        Err(e) if opts.ignore_errors => {
            log::warn!("Atomic alias update failed; retrying per index (--ignore-errors): {e:#}");
            let mut report = WriteReport::default();
            let mut targets: Vec<&String> = acts.iter().map(|(t, _)| t).collect();
            targets.dedup();
            for t in targets {
                let mine: Vec<Value> = acts
                    .iter()
                    .filter(|(x, _)| x == t)
                    .map(|(_, a)| a.clone())
                    .collect();
                let n = mine.len() as u64;
                match send_json(
                    client,
                    opts.retry,
                    Method::POST,
                    &["_aliases"],
                    &json!({"actions": mine}),
                    "update aliases",
                )
                .await
                {
                    Ok(_) => report.written += n,
                    Err(e) => {
                        log::error!("aliases of '{t}' failed (--ignore-errors): {e:#}");
                        report.failed += n;
                    }
                }
            }
            Ok(report)
        }
        Err(e) => Err(e.context("restoring aliases")),
    }
}

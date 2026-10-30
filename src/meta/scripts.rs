//! `--type script`: stored scripts and search templates, read from the
//! cluster state (there is no list API) and restored with
//! `POST /_scripts/{id}`.

use anyhow::{Result, bail};
use reqwest::Method;
use serde_json::{Map, Value, json};

use super::MetaOptions;
use super::http::{get_json_opt, get_json_q, send_json};
use crate::http::EsClient;
use crate::pipeline::WriteReport;

pub async fn fetch(client: &EsClient, name: Option<&str>, opts: &MetaOptions) -> Result<Value> {
    let filter = format!("**.stored_scripts.{}", name.unwrap_or("*"));
    let state = get_json_q(
        client,
        opts.retry,
        &["_cluster", "state", "metadata"],
        &[("filter_path", filter.as_str())],
        "GET _cluster/state/metadata (stored scripts)",
    )
    .await?;
    Ok(transform_state(&state))
}

/// Cluster state → elasticdump's `{"stored_scripts":[{_id,found,script}]}`.
pub fn transform_state(state: &Value) -> Value {
    let scripts = state
        .pointer("/metadata/stored_scripts")
        .and_then(Value::as_object)
        .map(|m| {
            m.iter()
                .map(|(id, s)| json!({"_id": id, "found": true, "script": s}))
                .collect::<Vec<_>>()
        })
        .unwrap_or_default();
    json!({ "stored_scripts": scripts })
}

/// `(id, {"script": {lang, source, options?}})` from the list shape or an
/// `{id: script}` map.
pub fn normalize(doc: &Value) -> Result<Vec<(String, Value)>> {
    let mut out = Vec::new();
    if let Some(list) = doc.get("stored_scripts") {
        let Some(list) = list.as_array() else {
            bail!("stored_scripts must be an array");
        };
        for s in list {
            let Some(id) = s.get("_id").and_then(Value::as_str) else {
                bail!("stored script without _id: {s}");
            };
            out.push((
                id.to_string(),
                body(s.get("script").unwrap_or(&Value::Null))?,
            ));
        }
    } else if let Value::Object(m) = doc {
        for (id, s) in m {
            out.push((id.clone(), body(s.get("script").unwrap_or(s))?));
        }
    }
    Ok(out)
}

fn body(script: &Value) -> Result<Value> {
    let Value::Object(s) = script else {
        bail!("stored script is not an object: {script}");
    };
    let mut s: Map<String, Value> = s.clone();
    if s.get("options")
        .is_some_and(|o| o.as_object().is_some_and(Map::is_empty))
    {
        s.shift_remove("options");
    }
    Ok(json!({ "script": s }))
}

pub async fn write(doc: Value, client: &EsClient, opts: &MetaOptions) -> Result<WriteReport> {
    let mut report = WriteReport::default();
    for (id, body) in normalize(&doc)? {
        if opts.skip_existing
            && get_json_opt(client, opts.retry, &["_scripts", &id], &[], "get script")
                .await?
                .is_some()
        {
            log::warn!("Stored script '{id}' already exists; skipping it (--skip-existing)");
            continue;
        }
        match send_json(
            client,
            opts.retry,
            Method::POST,
            &["_scripts", &id],
            &body,
            "put script",
        )
        .await
        {
            Ok(_) => report.written += 1,
            Err(e) if opts.ignore_errors => {
                log::error!("stored script '{id}' failed (--ignore-errors): {e:#}");
                report.failed += 1;
            }
            Err(e) => return Err(e.context(format!("restoring stored script '{id}'"))),
        }
    }
    Ok(report)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn state_transform_and_normalize() {
        let state = json!({"metadata": {"stored_scripts": {"s1": {"lang": "painless", "source": "1", "options": {}}}}});
        let doc = transform_state(&state);
        assert_eq!(
            doc,
            json!({"stored_scripts": [{"_id": "s1", "found": true, "script": {"lang": "painless", "source": "1", "options": {}}}]})
        );
        assert_eq!(transform_state(&json!({})), json!({"stored_scripts": []}));
        let n = normalize(&doc).unwrap();
        assert_eq!(
            n,
            vec![(
                "s1".to_string(),
                json!({"script": {"lang": "painless", "source": "1"}})
            )]
        );
        let m = normalize(&json!({"s2": {"lang": "mustache", "source": "{}"}})).unwrap();
        assert_eq!(
            m[0].1,
            json!({"script": {"lang": "mustache", "source": "{}"}})
        );
    }
}

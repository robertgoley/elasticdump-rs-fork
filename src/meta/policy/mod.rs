//! `--type policy`: Elasticsearch ILM (`_ilm/policy`) and OpenSearch ISM
//! (`_plugins/_ism/policies`).
//!
//! * ILM dump: `GET /_ilm/policy[/{name}]`, elasticdump's shape
//!   `{name: {version, modified_date, policy, in_use_by}}`.
//! * ISM dump: the paginated list API, written as
//!   `{"policies":[{_id,_seq_no,_primary_term,policy}],"total_policies":N}`.
//! * Restores stay within one engine. ILM → ISM is translated only with
//!   `--convertIlmToIsm` (see [`ilm_to_ism`]); ISM → ILM is always refused.

pub mod ilm_to_ism;

use anyhow::{Result, bail};
use reqwest::Method;
use serde_json::{Value, json};

use super::http::{get_json_opt, get_json_q, send_json, send_json_q, status_of};
use super::{MetaOptions, MetaTarget};
use crate::http::{ClusterInfo, EsClient};
use crate::pipeline::WriteReport;

const ISM: &str = "_plugins";
const ISM_PAGE: usize = 1000;

/// `*`/`?` glob match (policy name filters with wildcards are applied
/// client-side on both engines).
pub fn glob(pattern: &str, name: &str) -> bool {
    pattern.split(',').any(|p| {
        let re = format!(
            "^{}$",
            regex::escape(p.trim())
                .replace(r"\*", ".*")
                .replace(r"\?", ".")
        );
        regex::Regex::new(&re).is_ok_and(|r| r.is_match(name))
    })
}

pub async fn fetch(
    client: &EsClient,
    info: &ClusterInfo,
    name: Option<&str>,
    opts: &MetaOptions,
) -> Result<Value> {
    let wildcard = name.is_some_and(|n| n.contains(['*', '?', ',']));
    if !info.is_opensearch() {
        let mut segs = vec!["_ilm", "policy"];
        if let Some(n) = name.filter(|_| !wildcard) {
            segs.push(n);
        }
        let mut v = get_json_q(client, opts.retry, &segs, &[], "GET _ilm/policy").await?;
        if let (true, Some(p), Value::Object(m)) = (wildcard, name, &mut v) {
            m.retain(|k, _| glob(p, k));
        }
        return Ok(v);
    }
    if let Some(n) = name.filter(|_| !wildcard) {
        let one = get_json_q(
            client,
            opts.retry,
            &[ISM, "_ism", "policies", n],
            &[],
            "GET ISM policy",
        )
        .await?;
        return Ok(json!({"policies": [one], "total_policies": 1}));
    }
    let mut all = Vec::new();
    let mut from = 0usize;
    loop {
        let size = ISM_PAGE.to_string();
        let f = from.to_string();
        let page = get_json_opt(
            client,
            opts.retry,
            &[ISM, "_ism", "policies"],
            &[("size", size.as_str()), ("from", f.as_str())],
            "GET ISM policies",
        )
        .await?
        .unwrap_or_else(|| json!({"policies": [], "total_policies": 0}));
        let batch = page["policies"].as_array().cloned().unwrap_or_default();
        let total = page["total_policies"].as_u64().unwrap_or(0) as usize;
        let n = batch.len();
        all.extend(batch);
        from += n;
        if n == 0 || from >= total {
            break;
        }
    }
    if let Some(p) = name {
        all.retain(|it| it["_id"].as_str().is_some_and(|id| glob(p, id)));
    }
    let total = all.len();
    Ok(json!({"policies": all, "total_policies": total}))
}

/// `--filterManaged` (ILM): drop policies with `_meta.managed: true`.
pub fn drop_managed(v: &mut Value) {
    let Value::Object(m) = v else { return };
    if m.contains_key("policies") {
        return;
    }
    let mut managed = Vec::new();
    m.retain(|k, p| {
        let d = p.pointer("/policy/_meta/managed").and_then(Value::as_bool) == Some(true);
        if d {
            managed.push(k.clone());
        }
        !d
    });
    if !managed.is_empty() {
        log::info!(
            "Skipping {} managed policies (--filterManaged; --filterManaged=false keeps them): {}",
            managed.len(),
            managed.join(", ")
        );
    }
}

#[derive(Debug, PartialEq)]
pub enum Policies {
    /// `(name, ILM policy object {phases, _meta?})`
    Ilm(Vec<(String, Value)>),
    /// `(id, ISM policy object {states, default_state, ...})`
    Ism(Vec<(String, Value)>),
}

pub fn normalize(doc: &Value) -> Result<Policies> {
    let Value::Object(m) = doc else {
        bail!("policy document must be a JSON object");
    };
    let ism_item = |it: &Value| -> Result<(String, Value)> {
        let id = it
            .get("_id")
            .or_else(|| it.pointer("/policy/policy_id"))
            .and_then(Value::as_str);
        match (id, it.get("policy")) {
            (Some(id), Some(p)) => Ok((id.to_string(), p.clone())),
            _ => bail!("ISM policy entry without _id/policy: {it}"),
        }
    };
    if let Some(list) = m.get("policies") {
        let Some(list) = list.as_array() else {
            bail!("'policies' must be an array");
        };
        return Ok(Policies::Ism(
            list.iter().map(ism_item).collect::<Result<_>>()?,
        ));
    }
    if m.contains_key("_id") && m.contains_key("policy") {
        return Ok(Policies::Ism(vec![ism_item(doc)?]));
    }
    let mut ilm = Vec::new();
    let mut ism = Vec::new();
    for (name, entry) in m {
        let p = entry.get("policy").unwrap_or(entry);
        if p.get("phases").is_some() {
            ilm.push((name.clone(), p.clone()));
        } else if p.get("states").is_some() {
            ism.push((name.clone(), p.clone()));
        } else {
            bail!("'{name}' is neither an ILM policy (phases) nor an ISM policy (states)");
        }
    }
    match (ilm.is_empty(), ism.is_empty()) {
        (_, true) => Ok(Policies::Ilm(ilm)),
        (true, false) => Ok(Policies::Ism(ism)),
        (false, false) => bail!("the policy document mixes ILM and ISM policies"),
    }
}

pub async fn write(
    doc: Value,
    client: &EsClient,
    info: &ClusterInfo,
    dst: &MetaTarget,
    opts: &MetaOptions,
) -> Result<WriteReport> {
    match (normalize(&doc)?, info.is_opensearch()) {
        (Policies::Ilm(list), false) => {
            put_all(list, opts, |n, p| put_ilm(client, n, p, opts)).await
        }
        (Policies::Ism(list), true) => {
            let list = list
                .into_iter()
                .map(|(n, p)| clean_ism(p, dst, opts).map(|p| (n, p)))
                .collect::<Result<Vec<_>>>()?;
            put_all(list, opts, |n, p| put_ism(client, n, p, opts)).await
        }
        (Policies::Ism(list), false) => {
            if list.is_empty() {
                return Ok(WriteReport::default());
            }
            bail!(
                "these are OpenSearch ISM policies and the target is Elasticsearch: ISM → ILM conversion is not supported (ISM state machines, cron/doc-count transitions, notifications, rollups, snapshots and aliases have no ILM form); recreate them as ILM policies by hand"
            )
        }
        (Policies::Ilm(list), true) => {
            if list.is_empty() {
                return Ok(WriteReport::default());
            }
            if !opts.convert_ilm_to_ism {
                bail!(
                    "these are Elasticsearch ILM policies ({}) and the target is OpenSearch (ISM); re-run with --convertIlmToIsm to translate them (see the README for what is mapped, approximated and refused)",
                    list.iter()
                        .map(|(n, _)| n.as_str())
                        .collect::<Vec<_>>()
                        .join(", ")
                );
            }
            let mut converted = Vec::new();
            let mut failed = 0u64;
            for (name, p) in list {
                let caps = ilm_to_ism::IsmCaps {
                    shrink: info.has_ism_shrink(),
                };
                match ilm_to_ism::convert_for(&name, &p, opts.lenient_policy_conversion, caps) {
                    Ok(c) => {
                        for n in &c.notes {
                            log::warn!("ILM → ISM '{name}': {n}");
                        }
                        converted.push((name, clean_ism(c.policy, dst, opts)?));
                    }
                    Err(e) if opts.ignore_errors => {
                        log::error!("{e:#} (skipped, --ignore-errors)");
                        failed += 1;
                    }
                    Err(e) => return Err(e),
                }
            }
            if !converted.is_empty() {
                log::warn!(
                    "Converted ILM policies are not attached to any index: add an ism_template or use POST _plugins/_ism/add/<index>"
                );
            }
            let mut r = put_all(converted, opts, |n, p| put_ism(client, n, p, opts)).await?;
            r.failed += failed;
            Ok(r)
        }
    }
}

async fn put_all<F, Fut>(
    list: Vec<(String, Value)>,
    opts: &MetaOptions,
    put: F,
) -> Result<WriteReport>
where
    F: Fn(String, Value) -> Fut,
    Fut: std::future::Future<Output = Result<bool>>,
{
    let mut report = WriteReport::default();
    for (name, p) in list {
        match put(name.clone(), p).await {
            Ok(true) => report.written += 1,
            Ok(false) => {}
            Err(e) if opts.ignore_errors => {
                log::error!("policy '{name}' failed (--ignore-errors): {e:#}");
                report.failed += 1;
            }
            Err(e) => return Err(e.context(format!("restoring policy '{name}'"))),
        }
    }
    Ok(report)
}

/// ILM PUT body: `phases`, `_meta` and `deprecated` only.
pub fn ilm_body(p: &Value) -> Value {
    let mut out = serde_json::Map::new();
    for k in ["phases", "_meta", "deprecated"] {
        if let Some(v) = p.get(k) {
            out.insert(k.into(), v.clone());
        }
    }
    json!({ "policy": out })
}

async fn put_ilm(client: &EsClient, name: String, p: Value, opts: &MetaOptions) -> Result<bool> {
    if opts.skip_existing
        && get_json_opt(
            client,
            opts.retry,
            &["_ilm", "policy", &name],
            &[],
            "GET ILM policy",
        )
        .await?
        .is_some()
    {
        log::warn!("ILM policy '{name}' already exists; skipping it (--skip-existing)");
        return Ok(false);
    }
    send_json(
        client,
        opts.retry,
        Method::PUT,
        &["_ilm", "policy", &name],
        &ilm_body(&p),
        "put ILM policy",
    )
    .await?;
    Ok(true)
}

/// Drop server-generated ISM fields; rename `ism_template` patterns with
/// `--renamePatterns`.
pub fn clean_ism(mut p: Value, dst: &MetaTarget, opts: &MetaOptions) -> Result<Value> {
    if let Value::Object(m) = &mut p {
        for k in ["policy_id", "last_updated_time", "schema_version"] {
            m.shift_remove(k);
        }
        let fix = |t: &mut Value| -> Result<()> {
            if let Value::Object(t) = t {
                t.shift_remove("last_updated_time");
                if opts.rename_patterns {
                    if let Some(Value::Array(ps)) = t.get_mut("index_patterns") {
                        for x in ps.iter_mut() {
                            if let Value::String(s) = x {
                                *s = dst.renamer.pattern(s)?;
                            }
                        }
                    }
                }
            }
            Ok(())
        };
        match m.get_mut("ism_template") {
            Some(Value::Array(ts)) => {
                for t in ts.iter_mut() {
                    fix(t)?;
                }
            }
            Some(t @ Value::Object(_)) => fix(t)?,
            Some(Value::Null) => {
                m.shift_remove("ism_template");
            }
            _ => {}
        }
    }
    Ok(p)
}

async fn put_ism(client: &EsClient, id: String, p: Value, opts: &MetaOptions) -> Result<bool> {
    let segs = [ISM, "_ism", "policies", id.as_str()];
    let body = json!({ "policy": p });
    match send_json(
        client,
        opts.retry,
        Method::PUT,
        &segs,
        &body,
        "put ISM policy",
    )
    .await
    {
        Ok(_) => Ok(true),
        Err(e) if status_of(&e) == Some(409) => {
            if opts.skip_existing {
                log::warn!("ISM policy '{id}' already exists; skipping it (--skip-existing)");
                return Ok(false);
            }
            // overwrite with the current sequence number (ISM refuses blind updates)
            let cur = get_json_q(client, opts.retry, &segs, &[], "GET ISM policy").await?;
            let seq = cur["_seq_no"].to_string();
            let term = cur["_primary_term"].to_string();
            log::info!("ISM policy '{id}' exists; overwriting it (if_seq_no={seq})");
            send_json_q(
                client,
                opts.retry,
                Method::PUT,
                &segs,
                &[
                    ("if_seq_no", seq.as_str()),
                    ("if_primary_term", term.as_str()),
                ],
                &body,
                "update ISM policy",
            )
            .await
            .map_err(|e| {
                if status_of(&e) == Some(409) {
                    e.context(format!("ISM policy '{id}' was changed concurrently"))
                } else {
                    e
                }
            })?;
            Ok(true)
        }
        Err(e) => Err(e),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn shapes() {
        let ilm = json!({"p": {"version": 1, "modified_date": "x", "policy": {"phases": {"hot": {}}, "_meta": {"managed": true}}, "in_use_by": {}}});
        assert_eq!(
            normalize(&ilm).unwrap(),
            Policies::Ilm(vec![(
                "p".into(),
                json!({"phases": {"hot": {}}, "_meta": {"managed": true}})
            )])
        );
        let mut f = ilm.clone();
        drop_managed(&mut f);
        assert_eq!(f, json!({}));
        let ism = json!({"policies": [{"_id": "a", "_seq_no": 1, "policy": {"policy_id": "a", "states": []}}], "total_policies": 1});
        assert!(matches!(normalize(&ism).unwrap(), Policies::Ism(v) if v[0].0 == "a"));
        let one = json!({"_id": "b", "_version": 1, "policy": {"states": []}});
        assert!(matches!(normalize(&one).unwrap(), Policies::Ism(v) if v[0].0 == "b"));
        assert!(normalize(&json!({"x": {"policy": {}}})).is_err());
        assert!(
            normalize(&json!({"x": {"policy": {"phases": {}}}, "y": {"policy": {"states": []}}}))
                .is_err()
        );
        assert_eq!(
            ilm_body(&json!({"phases": {}, "_meta": {"a": 1}, "in_use_by": {}})),
            json!({"policy": {"phases": {}, "_meta": {"a": 1}}})
        );
        assert!(glob("it_*", "it_1") && !glob("it_*", "x") && glob("a,b", "b"));
    }
}

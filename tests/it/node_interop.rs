//! L-433 (TST-API-060): Node interop for every type. For each of the 11 types: Node dump ->
//! Rust restore and Rust dump -> Node restore, each checked by "dump, reset, restore, dump
//! again" against the Rust baseline; the Node and Rust dumps themselves are compared
//! semantically (after removing documented differences).
//!
//! Node-produced dumps are committed under `tests/fixtures/node/<engine>-<version>/<type>.json`
//! with the run's unique prefix replaced by `__U__` (record with `IT_RECORD_NODE_FIXTURES=1`).
//! Without Node, the "Node dump" side is read from those fixtures, so Node -> Rust restores
//! still run on the recorded version.

use std::path::{Path, PathBuf};
use std::process::Output;

use serde_json::{Value, json};

use crate::harness::*;

pub(crate) const TYPES: [&str; 11] = [
    "data",
    "mapping",
    "settings",
    "analyzer",
    "alias",
    "index",
    "template",
    "component_template",
    "index_template",
    "policy",
    "script",
];

fn node_bin() -> Option<&'static str> {
    let p = "/tmp/ed/node_modules/.bin/elasticdump";
    // IT_NODE=0 forces the fixture path (as on a machine without Node)
    (env("IT_NODE").as_deref() != Some("0")
        && Path::new(p).exists()
        && Path::new("/usr/bin/node").exists())
    .then_some(p)
}

async fn run_node(args: &[String]) -> Output {
    tokio::process::Command::new(node_bin().unwrap())
        .args(args)
        .env("NODE_TLS_REJECT_UNAUTHORIZED", "0")
        .output()
        .await
        .unwrap()
}

async fn run_rs(c: &Cluster, input: &str, output: &str, ty: &str) -> Output {
    let mut a = s(&["--input", input, "--output", output, "--type", ty]);
    if input.starts_with("http") {
        a.extend(c.tls_args("input"));
    }
    if output.starts_with("http") {
        a.extend(c.tls_args("output"));
    } else {
        a.push("--overwrite".into());
    }
    run(a).await
}

fn fixture_dir(engine: &str, version: &str) -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("tests/fixtures/node")
        .join(format!("{engine}-{version}"))
}

pub(crate) struct Names {
    pub(crate) u: String,
    pub(crate) ix: String,
    pub(crate) al: String,
    pub(crate) lt: String,
    pub(crate) ct: String,
    pub(crate) it: String,
    pub(crate) pol: String,
    pub(crate) sc: String,
}

impl Names {
    pub(crate) fn new() -> Self {
        Self::from_u(unique("nx"))
    }
    /// The names a run with unique prefix `u` used (for restoring another run's export).
    pub(crate) fn from_u(u: String) -> Self {
        Self {
            ix: format!("{u}_ix"),
            al: format!("{u}_al"),
            lt: format!("{u}_lt"),
            ct: format!("{u}_ct"),
            it: format!("{u}_it"),
            pol: format!("{u}_pol"),
            sc: format!("{u}_sc"),
            u,
        }
    }
    /// Path that selects the object of `ty`.
    pub(crate) fn selector(&self, ty: &str) -> &str {
        match ty {
            "template" => &self.lt,
            "component_template" => &self.ct,
            "index_template" => &self.it,
            "policy" => &self.pol,
            "script" => &self.sc,
            _ => &self.ix,
        }
    }
}

pub(crate) async fn seed(c: &Cluster, n: &Names, es: bool) {
    c.delete_index(&n.ix).await;
    c.ok(
        "PUT",
        &n.ix,
        Some(json!({
            "settings": {"number_of_shards": 1, "number_of_replicas": 0,
                "analysis": {"analyzer": {"my_an": {"type": "custom", "tokenizer": "whitespace", "filter": ["lowercase"]}}}},
            "mappings": {"properties": {"k": {"type": "keyword"}, "t": {"type": "text", "analyzer": "my_an"}, "n": {"type": "long"}}},
            "aliases": {n.al.as_str(): {"filter": {"term": {"k": "a"}}, "routing": "1"}}
        })),
    )
    .await;
    let mut bulk = String::new();
    for i in 0..20 {
        bulk.push_str(&format!(
            "{{\"index\":{{\"_index\":\"{}\",\"_id\":\"d{i:02}\"}}}}\n{{\"k\":\"{}\",\"t\":\"Doc {i} ünï\",\"n\":{}}}\n",
            n.ix,
            if i % 2 == 0 { "a" } else { "b" },
            9_007_199_254_740_000u64 + i
        ));
    }
    c.bulk_raw(bulk).await;
    c.ok("POST", &format!("{}/_refresh", n.ix), None).await;
    c.ok(
        "PUT",
        &format!("_template/{}", n.lt),
        Some(json!({"index_patterns": [format!("{}-lt-*", n.u)], "order": 3, "settings": {"number_of_shards": 1}, "mappings": {"properties": {"x": {"type": "keyword"}}}})),
    )
    .await;
    c.ok(
        "PUT",
        &format!("_component_template/{}", n.ct),
        Some(json!({"template": {"mappings": {"properties": {"y": {"type": "long"}}}}})),
    )
    .await;
    c.ok(
        "PUT",
        &format!("_index_template/{}", n.it),
        Some(json!({"index_patterns": [format!("{}-it-*", n.u)], "priority": 77, "composed_of": [n.ct.clone()], "template": {"settings": {"number_of_replicas": 0}}})),
    )
    .await;
    // the ES OSS build has no ILM: no policy is seeded there
    if es && c.is_oss().await {
    } else if es {
        c.ok(
            "PUT",
            &format!("_ilm/policy/{}", n.pol),
            Some(json!({"policy": {"phases": {"hot": {"actions": {"rollover": {"max_docs": 1000}}}, "delete": {"min_age": "30d", "actions": {"delete": {}}}}}})),
        )
        .await;
    } else {
        c.ok(
            "PUT",
            &format!("_plugins/_ism/policies/{}", n.pol),
            Some(json!({"policy": {"description": "nx", "default_state": "hot", "states": [{"name": "hot", "actions": [], "transitions": [{"state_name": "delete", "conditions": {"min_index_age": "30d"}}]}, {"name": "delete", "actions": [{"delete": {}}], "transitions": []}]}})),
        )
        .await;
    }
    c.ok(
        "PUT",
        &format!("_scripts/{}", n.sc),
        Some(json!({"script": {"lang": "painless", "source": "doc['n'].value * params.f"}})),
    )
    .await;
}

pub(crate) async fn cleanup(c: &Cluster, n: &Names, es: bool) {
    c.delete_index(&n.ix).await;
    let _ = c.call("DELETE", &format!("_template/{}", n.lt), None).await;
    let _ = c
        .call("DELETE", &format!("_index_template/{}", n.it), None)
        .await;
    let _ = c
        .call("DELETE", &format!("_component_template/{}", n.ct), None)
        .await;
    let pol = if es {
        format!("_ilm/policy/{}", n.pol)
    } else {
        format!("_plugins/_ism/policies/{}", n.pol)
    };
    let _ = c.call("DELETE", &pol, None).await;
    let _ = c.call("DELETE", &format!("_scripts/{}", n.sc), None).await;
}

/// Parse every line; metadata lines that are JSON string literals are decoded. Data lines are
/// sorted by `_id` and lose `_score`. Volatile and documented-divergent keys are removed.
fn normalise(path: &Path, ty: &str) -> Value {
    let mut out: Vec<Value> = lines(path)
        .iter()
        .filter(|l| !l.trim().is_empty())
        .map(|l| {
            let v: Value = serde_json::from_str(l).unwrap_or_else(|e| panic!("{l}: {e}"));
            let mut v = match v {
                Value::String(s) => serde_json::from_str(&s).unwrap_or(Value::String(s)),
                o => o,
            };
            scrub(&mut v);
            v
        })
        .collect();
    if ty == "policy" {
        // ISM: Rust writes `{"policies":[...],"total_policies":N}` even for one named policy,
        // Node writes the single GET response; compare the entries.
        out = out
            .into_iter()
            .flat_map(|v| match v.get("policies").and_then(Value::as_array) {
                Some(a) => a.clone(),
                None => vec![v],
            })
            .collect();
    }
    if ty == "analyzer" {
        // only the analysis is the analyzer type's payload; a restore creates the index with
        // default replicas
        for v in &mut out {
            if let Some(o) = v.as_object_mut() {
                for (_, d) in o.iter_mut() {
                    *d = d["settings"]["index"]["analysis"].clone();
                }
            }
        }
    }
    if ty == "data" {
        for v in &mut out {
            if let Some(o) = v.as_object_mut() {
                o.remove("_score");
                o.remove("_type");
            }
        }
        out.sort_by_key(|v| v["_id"].as_str().unwrap_or("").to_string());
    }
    Value::Array(out)
}

/// Keys that differ between runs or that Rust strips on purpose (generated index settings,
/// NODE-DIFFERENCES.md "generated keys"; ILM/ISM bookkeeping).
fn scrub(v: &mut Value) {
    match v {
        Value::Object(o) => {
            for k in [
                "uuid",
                "creation_date",
                "provided_name",
                "version",
                "history",
                "routing",
                "replication",
                "modified_date",
                "last_updated_time",
                "in_use_by",
                "_seq_no",
                "_primary_term",
                "_version",
                "schema_version",
                "error_notification",
                "ism_template",
                "policy_id",
            ] {
                if let Some(Value::Object(_) | Value::String(_) | Value::Number(_) | Value::Null) =
                    o.get(k)
                    && (k != "routing" || o.get(k).is_some_and(Value::is_object))
                {
                    o.remove(k);
                }
            }
            for (_, x) in o.iter_mut() {
                scrub(x);
            }
        }
        Value::Array(a) => a.iter_mut().for_each(scrub),
        _ => {}
    }
}

/// Bring the cluster back to "seeded minus the object of `ty`", ready for a restore.
async fn reset(c: &Cluster, n: &Names, es: bool, ty: &str, base: &Path) {
    let host = c.url_with_auth("");
    let ix_url = c.url_with_auth(&n.ix);
    match ty {
        "data" => {
            c.delete_index(&n.ix).await;
            assert_success(
                &run_rs(
                    c,
                    base.join("index.json").to_str().unwrap(),
                    &ix_url,
                    "index",
                )
                .await,
            );
        }
        // analyzer: restore creates the index with the analysis (NODE-DIFFERENCES #14)
        "index" | "settings" | "analyzer" => c.delete_index(&n.ix).await,
        "mapping" => {
            c.delete_index(&n.ix).await;
            assert_success(
                &run_rs(
                    c,
                    base.join("settings.json").to_str().unwrap(),
                    &ix_url,
                    "settings",
                )
                .await,
            );
        }
        "alias" => {
            c.ok(
                "POST",
                "_aliases",
                Some(json!({"actions": [{"remove": {"index": n.ix, "alias": n.al}}]})),
            )
            .await;
        }
        "template" => {
            c.ok("DELETE", &format!("_template/{}", n.lt), None).await;
        }
        "component_template" => {
            // the index template uses it: remove that first, restore it after
            c.ok("DELETE", &format!("_index_template/{}", n.it), None)
                .await;
            c.ok("DELETE", &format!("_component_template/{}", n.ct), None)
                .await;
        }
        "index_template" => {
            c.ok("DELETE", &format!("_index_template/{}", n.it), None)
                .await;
        }
        "policy" => {
            let p = if es {
                format!("_ilm/policy/{}", n.pol)
            } else {
                format!("_plugins/_ism/policies/{}", n.pol)
            };
            c.ok("DELETE", &p, None).await;
        }
        "script" => {
            c.ok("DELETE", &format!("_scripts/{}", n.sc), None).await;
        }
        _ => unreachable!(),
    }
    let _ = host;
}

/// Undo what `reset` + restore may have left different from the seeded state for the next
/// type (the data index and the index template removed for component_template).
async fn reseed_after(c: &Cluster, n: &Names, ty: &str, base: &Path) {
    let ix_url = c.url_with_auth(&n.ix);
    match ty {
        "index" | "settings" | "analyzer" | "mapping" | "data" => {
            c.delete_index(&n.ix).await;
            assert_success(
                &run_rs(
                    c,
                    base.join("index.json").to_str().unwrap(),
                    &ix_url,
                    "index",
                )
                .await,
            );
            assert_success(
                &run_rs(c, base.join("data.json").to_str().unwrap(), &ix_url, "data").await,
            );
            c.ok("POST", &format!("{}/_refresh", n.ix), None).await;
        }
        "policy" => {
            assert_success(
                &run_rs(
                    c,
                    base.join("policy.json").to_str().unwrap(),
                    &c.url_with_auth(""),
                    "policy",
                )
                .await,
            );
        }
        "component_template" => {
            assert_success(
                &run_rs(
                    c,
                    base.join("index_template.json").to_str().unwrap(),
                    &c.url_with_auth(""),
                    "index_template",
                )
                .await,
            );
        }
        _ => {}
    }
}

pub(crate) fn restore_target(c: &Cluster, n: &Names, ty: &str) -> String {
    match ty {
        "data" | "mapping" | "settings" | "analyzer" | "alias" | "index" => c.url_with_auth(&n.ix),
        _ => c.url_with_auth(""),
    }
}

/// Dump the object of `ty` again and compare with the baseline.
async fn after_restore(
    c: &Cluster,
    n: &Names,
    ty: &str,
    again: &Path,
    want: &Value,
) -> Result<(), String> {
    let o = run_rs(
        c,
        &c.url_with_auth(n.selector(ty)),
        again.to_str().unwrap(),
        ty,
    )
    .await;
    if !o.status.success() {
        return Err(format!(
            "re-dump failed: {}",
            String::from_utf8_lossy(&o.stderr)
        ));
    }
    let got = normalise(again, ty);
    if &got != want {
        return Err(format!("object changed:\n  now:  {got}\n  want: {want}"));
    }
    Ok(())
}

#[tokio::test]
async fn node_interop_every_type() {
    let c = Cluster::from_env();
    let es = matches!(c.engine, Engine::Elasticsearch);
    if es && c.is_oss().await {
        println!("SKIP: no ILM on the OSS build; interop runs on the default distribution");
        return;
    }
    let version = c.version().await;
    let engine = if es { "elasticsearch" } else { "opensearch" };
    let fixtures = fixture_dir(engine, &version);
    let node = node_bin().is_some();
    if !node && !fixtures.is_dir() {
        println!("SKIP: no Node elasticdump and no fixtures for {engine}-{version}");
        return;
    }
    let record = node && env("IT_RECORD_NODE_FIXTURES").is_some_and(|v| v == "1");
    let n = Names::new();
    seed(&c, &n, es).await;
    // Rust baselines
    let base = tempfile::tempdir().unwrap();
    for ty in TYPES {
        let f = base.path().join(format!("{ty}.json"));
        assert_success(
            &run_rs(
                &c,
                &c.url_with_auth(n.selector(ty)),
                f.to_str().unwrap(),
                ty,
            )
            .await,
        );
    }
    let mut failures = vec![];
    let mut green = 0;
    for ty in TYPES {
        let rf = base.path().join(format!("{ty}.json"));
        let want = normalise(&rf, ty);
        // the Node dump (live, or the committed fixture with this run's names)
        let nf = base.path().join(format!("node-{ty}.json"));
        if node {
            let mut a = s(&[
                "--input",
                &c.url_with_auth(n.selector(ty)),
                "--output",
                nf.to_str().unwrap(),
                "--type",
                ty,
            ]);
            if ty == "data" {
                a.extend(s(&["--limit", "7"]));
            }
            let o = run_node(&a).await;
            if !o.status.success() {
                failures.push(format!(
                    "{ty}: node dump failed: {}",
                    String::from_utf8_lossy(&o.stderr)
                ));
                continue;
            }
            if record {
                std::fs::create_dir_all(&fixtures).unwrap();
                let text = std::fs::read_to_string(&nf).unwrap().replace(&n.u, "__U__");
                std::fs::write(fixtures.join(format!("{ty}.json")), text).unwrap();
            }
        } else {
            let fx = fixtures.join(format!("{ty}.json"));
            let Ok(text) = std::fs::read_to_string(&fx) else {
                failures.push(format!("{ty}: fixture {} missing", fx.display()));
                continue;
            };
            std::fs::write(&nf, text.replace("__U__", &n.u)).unwrap();
        }
        // 1. dumps agree
        let got = normalise(&nf, ty);
        if got != want {
            failures.push(format!(
                "{ty}: Node dump differs from Rust dump:\n  node: {got}\n  rust: {want}"
            ));
        }
        // 2. Node dump -> Rust restore
        reset(&c, &n, es, ty, base.path()).await;
        let o = run_rs(&c, nf.to_str().unwrap(), &restore_target(&c, &n, ty), ty).await;
        let again = base.path().join(format!("again-{ty}.json"));
        if !o.status.success() {
            failures.push(format!(
                "{ty}: node->rust restore failed: {}",
                String::from_utf8_lossy(&o.stderr)
            ));
        } else {
            if ty == "data" {
                c.ok("POST", &format!("{}/_refresh", n.ix), None).await;
            }
            match after_restore(&c, &n, ty, &again, &want).await {
                Ok(()) => green += 1,
                Err(e) => failures.push(format!("{ty}: node->rust restore: {e}")),
            }
        }
        reseed_after(&c, &n, ty, base.path()).await;
        // 3. Rust dump -> Node restore. Known difference #7 (NODE-DIFFERENCES.md): Node's ISM
        // restore never creates the policy; assert that instead.
        if node && ty == "policy" && !es {
            reset(&c, &n, es, ty, base.path()).await;
            let _ = run_node(&s(&[
                "--input",
                rf.to_str().unwrap(),
                "--output",
                &restore_target(&c, &n, ty),
                "--type",
                ty,
            ]))
            .await;
            let (st, _) = c
                .call("GET", &format!("_plugins/_ism/policies/{}", n.pol), None)
                .await;
            if st == 404 {
                println!(
                    "policy: rust->node on OpenSearch leaves the policy missing (known difference #7)"
                );
                green += 1;
            } else {
                failures.push(format!(
                    "policy: Node's ISM restore now works (status {st}); update NODE-DIFFERENCES #7"
                ));
            }
            reseed_after(&c, &n, ty, base.path()).await;
        } else if node {
            reset(&c, &n, es, ty, base.path()).await;
            let o = run_node(&s(&[
                "--input",
                rf.to_str().unwrap(),
                "--output",
                &restore_target(&c, &n, ty),
                "--type",
                ty,
            ]))
            .await;
            if !o.status.success() {
                failures.push(format!(
                    "{ty}: rust->node restore failed: {}",
                    String::from_utf8_lossy(&o.stderr)
                ));
            } else {
                if ty == "data" {
                    c.ok("POST", &format!("{}/_refresh", n.ix), None).await;
                }
                match after_restore(&c, &n, ty, &again, &want).await {
                    Ok(()) => green += 1,
                    Err(e) => failures.push(format!("{ty}: rust->node restore: {e}")),
                }
            }
            reseed_after(&c, &n, ty, base.path()).await;
        }
    }
    cleanup(&c, &n, es).await;
    println!(
        "node interop {engine}-{version}: {green} directions green (node={node}, record={record})"
    );
    assert!(failures.is_empty(), "{}", failures.join("\n"));
}

// L-434 (TST-API-061): compression interop on single files (split parts are covered by
// files::node_interop_split_names_and_gzip). Node gzip -> Rust (auto-detected, with and without
// --fsCompress); Rust gzip -> Node (--fsCompress); Rust zstd -> Node is refused: Node cannot
// read .zst (README "zstd"), so it must fail or write nothing, never import garbage.
#[tokio::test]
async fn compression_interop_gzip_both_ways_zstd_refused_by_node() {
    if node_bin().is_none() {
        println!("SKIP: node elasticdump not installed");
        return;
    }
    let c = Cluster::from_env();
    let src = unique("ncomp_src");
    seed_index(&c, &src, 500).await;
    // The big-integer documents of seed_index are left out of the comparison: Node's
    // --support-big-int writes integers >= 1e21 in exponent form (value-equal, not byte-equal;
    // byte equality is L-437's subject with Node's own fixtures).
    let digest = |m: std::collections::BTreeMap<String, (Option<String>, Value)>| {
        m.into_iter()
            .filter(|(_, (_, v))| v["k"] != "bigint")
            .collect::<std::collections::BTreeMap<_, _>>()
    };
    let want = digest(c.docs(&src).await);
    let dir = tempfile::tempdir().unwrap();
    // Node gzip -> Rust
    let ngz = dir.path().join("node.json.gz");
    let o = run_node(&s(&[
        "--input",
        &c.url_with_auth(&src),
        "--output",
        ngz.to_str().unwrap(),
        "--fsCompress",
        "--support-big-int",
    ]))
    .await;
    assert!(o.status.success(), "{}", String::from_utf8_lossy(&o.stderr));
    assert_eq!(
        &std::fs::read(&ngz).unwrap()[..2],
        &[0x1f, 0x8b],
        "Node wrote gzip"
    );
    for extra in [&[][..], &["--fsCompress"][..]] {
        let dst = unique("ncomp_r");
        c.copy_shape(&src, &dst).await;
        let mut a = s(&[
            "--input",
            ngz.to_str().unwrap(),
            "--output",
            &c.url_with_auth(&dst),
        ]);
        a.extend(c.tls_args("output"));
        a.extend(s(extra));
        assert_success(&run(a).await);
        let got = digest(c.docs(&dst).await);
        let diff: Vec<_> = want
            .iter()
            .filter(|(k, v)| got.get(*k) != Some(v))
            .map(|(k, v)| format!("{k}: want {v:?} got {:?}", got.get(k)))
            .take(3)
            .collect();
        assert!(
            diff.is_empty(),
            "node gzip -> rust {extra:?} ({} vs {} docs): {diff:?}",
            got.len(),
            want.len()
        );
        c.delete_index(&dst).await;
    }
    // (seed_index holds big integers: Node needs --support-big-int to keep them)
    // Rust gzip -> Node
    let rgz = dir.path().join("rust.json.gz");
    let mut a = s(&[
        "--input",
        &c.url_with_auth(&src),
        "--output",
        rgz.to_str().unwrap(),
        "--fsCompress",
        "--overwrite",
    ]);
    a.extend(c.tls_args("input"));
    assert_success(&run(a).await);
    let dst = unique("ncomp_n");
    c.copy_shape(&src, &dst).await;
    let o = run_node(&s(&[
        "--input",
        rgz.to_str().unwrap(),
        "--output",
        &c.url_with_auth(&dst),
        "--fsCompress",
        "--support-big-int",
    ]))
    .await;
    assert!(o.status.success(), "{}", String::from_utf8_lossy(&o.stderr));
    c.ok("POST", &format!("{dst}/_refresh"), None).await;
    assert_eq!(digest(c.docs(&dst).await), want, "rust gzip -> node");
    c.delete_index(&dst).await;
    // Rust zstd -> Node: refused (error or nothing written), with or without --fsCompress
    let zst = dir.path().join("rust.json.zst");
    let mut a = s(&[
        "--input",
        &c.url_with_auth(&src),
        "--output",
        zst.to_str().unwrap(),
        "--compressionType",
        "zstd",
    ]);
    a.extend(c.tls_args("input"));
    assert_success(&run(a).await);
    assert_eq!(
        &std::fs::read(&zst).unwrap()[..4],
        &[0x28, 0xb5, 0x2f, 0xfd],
        "zstd magic"
    );
    for extra in [&[][..], &["--fsCompress"][..]] {
        let dst = unique("ncomp_z");
        c.copy_shape(&src, &dst).await;
        let mut a = s(&[
            "--input",
            zst.to_str().unwrap(),
            "--output",
            &c.url_with_auth(&dst),
        ]);
        a.extend(s(extra));
        let o = run_node(&a).await;
        c.ok("POST", &format!("{dst}/_refresh"), None).await;
        let n = c.count(&dst).await;
        println!(
            "zstd -> node {extra:?}: exit {:?}, {n} docs",
            o.status.code()
        );
        assert!(
            !o.status.success() || n == 0,
            "Node imported a zstd file: exit 0 with {n} docs"
        );
        assert_eq!(n, 0, "Node must not import anything from a zstd file");
        c.delete_index(&dst).await;
    }
    c.delete_index(&src).await;
}

// L-435 (TST-API-062): split naming interop. `--fileSize` counts UTF-8 bytes in Rust and
// UTF-16 code units in Node (README "Intentional differences"): ASCII data gives identical part
// names (asserted in files::node_interop_split_names_and_gzip); for non-ASCII text Node's parts
// hold more bytes, so Rust writes more parts. Both sets restore to the same documents.
#[tokio::test]
async fn split_names_non_ascii_boundary_difference() {
    if node_bin().is_none() {
        println!("SKIP: node elasticdump not installed");
        return;
    }
    let c = Cluster::from_env();
    let src = unique("nsplit_u");
    c.delete_index(&src).await;
    c.ok(
        "PUT",
        &src,
        Some(json!({"settings": {"number_of_shards": 1, "number_of_replicas": 0}})),
    )
    .await;
    let mut nd = String::new();
    for i in 0..1500 {
        nd.push_str(&json!({"index": {"_index": src, "_id": format!("{i:06}")}}).to_string());
        nd.push('\n');
        nd.push_str(&json!({"t": "é".repeat(60 + i % 40), "k": format!("κλειδί{i}")}).to_string());
        nd.push('\n');
    }
    c.bulk_raw(nd).await;
    c.ok("POST", &format!("{src}/_refresh"), None).await;
    let parts = |d: &Path| -> Vec<(String, u64)> {
        let mut v: Vec<(String, u64)> = std::fs::read_dir(d)
            .unwrap()
            .flatten()
            .map(|e| {
                (
                    e.file_name().to_string_lossy().into_owned(),
                    e.metadata().unwrap().len(),
                )
            })
            .collect();
        v.sort();
        v
    };
    let (rdir, ndir) = (tempfile::tempdir().unwrap(), tempfile::tempdir().unwrap());
    let mut a = s(&[
        "--input",
        &c.url_with_auth(&src),
        "--output",
        rdir.path().join("out.json").to_str().unwrap(),
        "--fileSize",
        "64kb",
    ]);
    a.extend(c.tls_args("input"));
    assert_success(&run(a).await);
    let o = run_node(&s(&[
        "--input",
        &c.url_with_auth(&src),
        "--output",
        ndir.path().join("out.json").to_str().unwrap(),
        "--fileSize",
        "64kb",
    ]))
    .await;
    assert!(o.status.success(), "{}", String::from_utf8_lossy(&o.stderr));
    let (rp, np) = (parts(rdir.path()), parts(ndir.path()));
    println!("rust parts: {}, node parts: {}", rp.len(), np.len());
    let max_line = 600u64;
    // Rust: byte-bounded parts; Node: UTF-16-bounded parts, so more bytes each and fewer parts
    assert!(rp.iter().all(|(_, b)| *b <= 65_536 + max_line), "{rp:?}");
    assert!(np.iter().any(|(_, b)| *b > 65_536 + max_line), "{np:?}");
    assert!(
        rp.len() > np.len(),
        "rust {} vs node {}",
        rp.len(),
        np.len()
    );
    // same naming scheme: Node's names are a prefix of Rust's list
    let rn: Vec<&String> = rp.iter().map(|(n, _)| n).collect();
    let nn: Vec<&String> = np.iter().map(|(n, _)| n).collect();
    assert!(
        nn.iter().all(|n| rn.contains(n)),
        "node {nn:?} vs rust {rn:?}"
    );
    // both restore to the same documents
    let want = c.docs(&src).await;
    for (label, dir) in [("rust", rdir.path()), ("node", ndir.path())] {
        let dst = unique("nsplit_ud");
        c.copy_shape(&src, &dst).await;
        for (n, _) in parts(dir) {
            let mut a = s(&[
                "--input",
                dir.join(&n).to_str().unwrap(),
                "--output",
                &c.url_with_auth(&dst),
            ]);
            a.extend(c.tls_args("output"));
            assert_success(&run(a).await);
        }
        assert_eq!(c.docs(&dst).await, want, "{label} parts");
        c.delete_index(&dst).await;
    }
    c.delete_index(&src).await;
}

// L-436 (TST-API-063): data file variants accepted, each restoring to the same digest:
// `--sourceOnly` files, `_type`, `fields`, `_routing` / `routing` / `fields._routing`,
// `--format human` pretty files, concatenated JSON, CRLF and a UTF-8 BOM.
#[tokio::test]
async fn data_file_variants_restore_to_the_same_digest() {
    let c = Cluster::from_env();
    let src = unique("variants_src");
    c.delete_index(&src).await;
    c.ok(
        "PUT",
        &src,
        Some(json!({"settings": {"number_of_shards": 2, "number_of_replicas": 0}})),
    )
    .await;
    let docs: Vec<(String, Option<String>, Value)> = (0..12)
        .map(|i| {
            (
                format!("d{i:02}"),
                (i % 3 != 0).then(|| format!("r{}", i % 3)),
                json!({"n": i, "t": format!("text {i} ü"), "nested": {"a": [i, i + 1]}}),
            )
        })
        .collect();
    let line = |style: &str, (id, r, src_doc): &(String, Option<String>, Value)| -> Value {
        let mut v = json!({"_index": "x", "_id": id, "_source": src_doc});
        if let Some(r) = r {
            match style {
                "_routing" => v["_routing"] = json!(r),
                "routing" => v["routing"] = json!(r),
                "fields" => v["fields"] = json!({"_routing": r}),
                _ => unreachable!(),
            }
        }
        v
    };
    let ndjson = |vals: &[Value], sep: &str| {
        vals.iter()
            .map(|v| v.to_string())
            .collect::<Vec<_>>()
            .join(sep)
            + sep
    };
    let canonical: Vec<Value> = docs.iter().map(|d| line("_routing", d)).collect();
    let mut variants: Vec<(&str, String, bool)> = vec![
        ("_routing", ndjson(&canonical, "\n"), false),
        (
            "routing",
            ndjson(
                &docs.iter().map(|d| line("routing", d)).collect::<Vec<_>>(),
                "\n",
            ),
            false,
        ),
        (
            "fields._routing",
            ndjson(
                &docs.iter().map(|d| line("fields", d)).collect::<Vec<_>>(),
                "\n",
            ),
            false,
        ),
        (
            "_type",
            ndjson(
                &canonical
                    .iter()
                    .cloned()
                    .map(|mut v| {
                        v["_type"] = json!("_doc");
                        v
                    })
                    .collect::<Vec<_>>(),
                "\n",
            ),
            false,
        ),
        ("concatenated", ndjson(&canonical, ""), false),
        ("CRLF", ndjson(&canonical, "\r\n"), false),
        (
            "BOM",
            format!("\u{feff}{}", ndjson(&canonical, "\n")),
            false,
        ),
        (
            "human",
            canonical
                .iter()
                .map(|v| serde_json::to_string_pretty(v).unwrap())
                .collect::<Vec<_>>()
                .join("\n")
                + "\n",
            false,
        ),
        (
            "sourceOnly",
            ndjson(&docs.iter().map(|d| d.2.clone()).collect::<Vec<_>>(), "\n"),
            true,
        ),
    ];
    // a real --format human dump and a real --sourceOnly dump from the tool itself
    let mut bulk = String::new();
    for d in &docs {
        let mut meta = json!({"index": {"_index": src, "_id": d.0}});
        if let Some(r) = &d.1 {
            meta["index"]["routing"] = json!(r);
        }
        bulk.push_str(&format!("{meta}\n{}\n", d.2));
    }
    c.bulk_raw(bulk).await;
    c.ok("POST", &format!("{src}/_refresh"), None).await;
    let want = c.docs(&src).await;
    for (flag, label, source_only) in [
        ("--format", "tool human dump", false),
        ("--sourceOnly", "tool sourceOnly dump", true),
    ] {
        let f = tmp_path("variants_tool", "json");
        let mut a = s(&[
            "--input",
            &c.url_with_auth(&src),
            "--output",
            f.to_str().unwrap(),
        ]);
        a.extend(c.tls_args("input"));
        if flag == "--format" {
            a.extend(s(&["--format", "human"]));
        } else {
            a.push("--sourceOnly".into());
        }
        assert_success(&run(a).await);
        variants.push((label, std::fs::read_to_string(&f).unwrap(), source_only));
        let _ = std::fs::remove_file(f);
    }
    let sources = |m: &std::collections::BTreeMap<String, (Option<String>, Value)>| {
        let mut v: Vec<String> = m.values().map(|(_, s)| s.to_string()).collect();
        v.sort();
        v
    };
    let mut failures = vec![];
    for (name, body, source_only) in variants {
        let f = tmp_path("variant", "json");
        std::fs::write(&f, &body).unwrap();
        let dst = unique("variants_dst");
        c.copy_shape(&src, &dst).await;
        let mut a = s(&[
            "--input",
            f.to_str().unwrap(),
            "--output",
            &c.url_with_auth(&dst),
        ]);
        a.extend(c.tls_args("output"));
        let o = run(a).await;
        if !o.status.success() {
            failures.push(format!(
                "{name}: exit {:?}: {}",
                o.status.code(),
                String::from_utf8_lossy(&o.stderr)
            ));
        } else {
            let got = c.docs(&dst).await;
            let ok = if source_only {
                sources(&got) == sources(&want)
            } else {
                got == want
            };
            if !ok {
                failures.push(format!(
                    "{name}: digest differs:\n  got:  {got:?}\n  want: {want:?}"
                ));
            }
        }
        c.delete_index(&dst).await;
        let _ = std::fs::remove_file(f);
    }
    c.delete_index(&src).await;
    assert!(failures.is_empty(), "{}", failures.join("\n"));
}

// L-437 (TST-API-064): Node's bigint fixtures (tests/fixtures/node/bigint, from the Node repo's
// test/test-resources, Apache-2.0) restore and dump with `--support-big-int` byte for byte, and
// Node's own `--support-big-int` dump of the same index is byte-equal too.
#[tokio::test]
async fn node_bigint_fixtures_round_trip_byte_for_byte() {
    let c = Cluster::from_env();
    let dir = Path::new(env!("CARGO_MANIFEST_DIR")).join("tests/fixtures/node/bigint");
    for (data, mapping) in [
        ("bigint.json", "bigint_mapping.json"),
        ("bigint2.json", "bigint_mapping2.json"),
        ("bigint3.json", "bigint_mapping3.json"),
    ] {
        let dst = unique("bigint");
        c.delete_index(&dst).await;
        assert_success(
            &run_rs(
                &c,
                dir.join(mapping).to_str().unwrap(),
                &c.url_with_auth(&dst),
                "mapping",
            )
            .await,
        );
        assert_success(
            &run_rs(
                &c,
                dir.join(data).to_str().unwrap(),
                &c.url_with_auth(&dst),
                "data",
            )
            .await,
        );
        c.ok("POST", &format!("{dst}/_refresh"), None).await;
        // bigint2/3 hold one id; bigint.json has distinct ids
        let want = raw_sources_by_id(&dir.join(data));
        let out = tempfile::tempdir().unwrap();
        let rf = out.path().join("rust.json");
        let mut a = s(&[
            "--input",
            &c.url_with_auth(&dst),
            "--output",
            rf.to_str().unwrap(),
            "--support-big-int",
        ]);
        a.extend(c.tls_args("input"));
        assert_success(&run(a).await);
        assert_eq!(raw_sources_by_id(&rf), want, "{data}: rust dump");
        if node_bin().is_some() {
            let nf = out.path().join("node.json");
            let o = run_node(&s(&[
                "--input",
                &c.url_with_auth(&dst),
                "--output",
                nf.to_str().unwrap(),
                "--support-big-int",
            ]))
            .await;
            assert!(o.status.success(), "{}", String::from_utf8_lossy(&o.stderr));
            assert_eq!(raw_sources_by_id(&nf), want, "{data}: node dump");
        }
        c.delete_index(&dst).await;
    }
}

/// L-511 (TST-INJ-014): escaping corpus round trip, file -> engine -> file. Ids and values with
/// `"`, `\`, control escapes, U+2028/U+2029 (raw and escaped), astral characters (raw and as
/// surrogate pairs), lone surrogate escapes, 2^64+1, -0.0, 1e400, ... Every `_source` must come
/// back byte for byte (the multiset digest of sources is equal) and every `_id` decodes to the
/// same string. The index stores sources without indexing them (`enabled: false`), so values
/// out of any numeric range are legal. Node is run on the same file when installed and its
/// differences are printed (documented in NODE-DIFFERENCES.md), not asserted.
#[tokio::test]
async fn xv_escaping_corpus_round_trips_byte_for_byte() {
    use sonic_rs::JsonValueTrait;
    use std::hash::{Hash, Hasher};
    let c = Cluster::from_env();
    let ids: &[&str] = &[
        r#""plain""#,
        r#""q\"uote""#,
        r#""back\\slash""#,
        r#""ctl\u0001\u001f""#,
        r#""nl\n\t\r""#,
        "\"ls\u{2028}ps\u{2029}raw\"",
        r#""ls\u2028ps\u2029esc""#,
        "\"astral😀raw\"",
        r#""astral\ud83d\ude00esc""#,
        r#""slash\/esc""#,
        r#""é\u00e9mixed""#,
        r#""lone\ud800high""#,
        r#""lone\udc00low""#,
        "18446744073709551617",
        "-0",
    ];
    let values: &[&str] = &[
        r#""q\"\\\/\b\f\n\r\t""#,
        r#""\u0000\u0001\u007f\u0080""#,
        "\"\u{2028}\u{2029}\"",
        r#""\u2028\u2029""#,
        "\"😀𝄞\"",
        r#""\ud83d\ude00\uD834\uDD1E""#,
        r#""\ud800""#,
        r#""\udfff\ud800""#,
        "18446744073709551617",
        "-18446744073709551617",
        "-0.0",
        "-0",
        "1e400",
        "-1e-400",
        "1.0E+2",
        "0.1000000000000000055511151231257827",
        r#"{"k\u0000":1,"k\"":[true,null,{}],"":""}"#,
        r#"[ 1 , "a" ,{ } ]"#,
    ];
    let mut cases: Vec<(String, String, String)> = Vec::new(); // (case, id literal, source raw)
    for (i, id) in ids.iter().enumerate() {
        for (j, v) in values.iter().enumerate() {
            // string ids get a per-case suffix; a numeric id can only be used once
            let numeric = !id.starts_with('"');
            if (numeric && j != i % values.len())
                || (!numeric && (i + j) % 3 != 0 && j != i % values.len())
            {
                continue;
            }
            let case = format!("c{i:02}-{j:02}");
            let src = format!(r#"{{"case":"{case}","v":{v},"k\u2028\"\\":1}}"#);
            cases.push((case, id.to_string(), src));
        }
    }
    let mut body = String::new();
    for (case, id, src) in &cases {
        let id = if id.starts_with('"') {
            format!("{}-{case}\"", &id[..id.len() - 1])
        } else {
            id.clone()
        };
        body.push_str(&format!(r#"{{"_index":"x","_id":{id},"_source":{src}}}"#));
        body.push('\n');
    }
    let dir = tempfile::tempdir().unwrap();
    let input = dir.path().join("corpus.json");
    std::fs::write(&input, &body).unwrap();
    let numeric_ids = cases
        .iter()
        .filter(|(_, id, _)| !id.starts_with('"'))
        .count();
    let digest = |srcs: &mut Vec<String>| {
        srcs.sort();
        let mut h = std::collections::hash_map::DefaultHasher::new();
        srcs.hash(&mut h);
        h.finish()
    };
    let want: std::collections::BTreeMap<String, String> = cases
        .iter()
        .map(|(c, _, s)| (c.clone(), s.clone()))
        .collect();
    let decoded_ids = |path: &Path| -> std::collections::BTreeMap<String, (String, String)> {
        let mut m = std::collections::BTreeMap::new();
        for l in lines(path) {
            let src = sonic_rs::get(l.as_bytes(), ["_source"])
                .unwrap()
                .as_raw_str()
                .to_string();
            let case = sonic_rs::get(src.as_bytes(), ["case"])
                .unwrap()
                .as_str()
                .unwrap()
                .to_string();
            let id_raw = sonic_rs::get(l.as_bytes(), ["_id"])
                .unwrap()
                .as_raw_str()
                .to_string();
            m.insert(case, (id_raw, src));
        }
        m
    };
    let mut report = Vec::new();
    let mut id_diffs = 0;
    for tool in ["rust", "node"] {
        if tool == "node" && node_bin().is_none() {
            eprintln!("SKIP node side: Node elasticdump not installed");
            continue;
        }
        let idx = unique(&format!("esc-{tool}"));
        c.delete_index(&idx).await;
        c.ok(
            "PUT",
            &idx,
            Some(json!({"settings": {"number_of_shards": 1, "number_of_replicas": 0}, "mappings": {"enabled": false}})),
        )
        .await;
        let out = dir.path().join(format!("{tool}.json"));
        let restore = if tool == "rust" {
            run_rs(&c, input.to_str().unwrap(), &c.url_with_auth(&idx), "data").await
        } else {
            let to = c.url_with_auth(&idx);
            run_node(&s(&[
                "--input",
                input.to_str().unwrap(),
                "--output",
                &to,
                "--type",
                "data",
            ]))
            .await
        };
        let restore_err = String::from_utf8_lossy(&restore.stderr).to_string();
        c.ok("POST", &format!("{idx}/_refresh"), None).await;
        let o = if tool == "rust" {
            run_rs(&c, &c.url_with_auth(&idx), out.to_str().unwrap(), "data").await
        } else {
            run_node(&s(&[
                "--input",
                &c.url_with_auth(&idx),
                "--output",
                out.to_str().unwrap(),
                "--type",
                "data",
            ]))
            .await
        };
        assert!(
            o.status.success(),
            "{tool} dump: {}",
            String::from_utf8_lossy(&o.stderr)
        );
        let got = decoded_ids(&out);
        let mut diffs = Vec::new();
        for (case, src) in &want {
            match got.get(case) {
                None => diffs.push(format!("{case}: missing (restore: {})", restore.status)),
                Some((_, s)) if s != src => diffs.push(format!("{case}: source {src} -> {s}")),
                _ => {}
            }
        }
        let mut a: Vec<String> = want.values().cloned().collect();
        let mut b: Vec<String> = got.values().map(|(_, s)| s.clone()).collect();
        let equal = digest(&mut a) == digest(&mut b);
        report.push(format!(
            "{tool} on {}: {} docs ({numeric_ids} numeric ids), restore exit {:?}, {} returned, digest {}, {} differences",
            c.version().await,
            cases.len(),
            restore.status.code(),
            got.len(),
            if equal { "EQUAL" } else { "DIFFERENT" },
            diffs.len()
        ));
        for d in diffs.iter().take(if tool == "rust" { 40 } else { 12 }) {
            report.push(format!("  {d}"));
        }
        if tool == "rust" {
            // ids: what the engine returns decodes to what was sent (lone surrogates: reported)
            for (case, (id_raw, _)) in &got {
                let sent = cases.iter().find(|(c, _, _)| c == case).unwrap();
                let sent_id = if sent.1.starts_with('"') {
                    format!("{}-{case}\"", &sent.1[..sent.1.len() - 1])
                } else {
                    format!("\"{}\"", sent.1)
                };
                // engines store a lone surrogate escape in an _id as U+FFFD
                let lone_to_fffd = sent_id
                    .replace("\\ud800", "\u{fffd}")
                    .replace("\\udc00", "\u{fffd}");
                let a = serde_json::from_str::<String>(&lone_to_fffd).ok();
                let b = serde_json::from_str::<String>(id_raw).ok();
                if a.is_none() || a != b {
                    report.push(format!("  {case}: id {sent_id} -> {id_raw}"));
                    id_diffs += 1;
                }
            }
            if !restore.status.success() {
                report.push(format!(
                    "  restore stderr: {}",
                    restore_err
                        .lines()
                        .rev()
                        .take(3)
                        .collect::<Vec<_>>()
                        .join(" | ")
                ));
            }
        }
        c.delete_index(&idx).await;
        if tool == "rust" {
            eprintln!("{}", report.join("\n"));
            assert!(
                restore.status.success(),
                "rust restore failed: {restore_err}"
            );
            assert!(
                equal && diffs.is_empty() && id_diffs == 0,
                "rust round trip differs:\n{}",
                report.join("\n")
            );
        }
    }
    eprintln!("{}", report.join("\n"));
}

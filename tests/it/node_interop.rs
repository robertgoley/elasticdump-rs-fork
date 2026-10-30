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

const TYPES: [&str; 11] = [
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

struct Names {
    u: String,
    ix: String,
    al: String,
    lt: String,
    ct: String,
    it: String,
    pol: String,
    sc: String,
}

impl Names {
    fn new() -> Self {
        let u = unique("nx");
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
    fn selector(&self, ty: &str) -> &str {
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

async fn seed(c: &Cluster, n: &Names, es: bool) {
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
    if es {
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

async fn cleanup(c: &Cluster, n: &Names, es: bool) {
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
                {
                    if k != "routing" || o.get(k).is_some_and(Value::is_object) {
                        o.remove(k);
                    }
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

fn restore_target(c: &Cluster, n: &Names, ty: &str) -> String {
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

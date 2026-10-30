//! Phase A4 per-version API behaviour (TST-API-010 onwards). Every test runs on whatever
//! engine `scripts/it/up.sh` started; version-specific expectations are branched on
//! `Cluster::vnum()` / `engine()` so one source covers the whole matrix.

use std::collections::BTreeMap;
use std::path::{Path, PathBuf};

use serde_json::Value;

use crate::harness::*;

pub(crate) fn io(c: &Cluster, input: &str, output: &str, extra: &[&str]) -> Vec<String> {
    let mut a = s(&["--input", input, "--output", output]);
    if input.starts_with("http") {
        a.extend(c.tls_args("input"));
    }
    if output.starts_with("http") {
        a.extend(c.tls_args("output"));
    }
    a.extend(s(extra));
    a
}

/// Dump `index` to a fresh file with `extra` flags; returns the file and the process output.
pub(crate) async fn dump(
    c: &Cluster,
    index: &str,
    extra: &[&str],
) -> (PathBuf, std::process::Output) {
    let out = tmp_path("api", "json");
    let o = run(io(c, &c.url_with_auth(index), out.to_str().unwrap(), extra)).await;
    (out, o)
}

/// As `dump`, without `--quiet` (for warning assertions).
pub(crate) async fn dump_loud(
    c: &Cluster,
    index: &str,
    extra: &[&str],
) -> (PathBuf, std::process::Output) {
    let out = tmp_path("api", "json");
    let o = run_loud(io(c, &c.url_with_auth(index), out.to_str().unwrap(), extra)).await;
    (out, o)
}

/// Same shape as `Cluster::docs`: id -> (routing, _source).
pub(crate) fn file_docs(path: &Path) -> BTreeMap<String, (Option<String>, Value)> {
    let mut m = BTreeMap::new();
    for l in lines(path) {
        let v: Value = serde_json::from_str(&l).unwrap();
        let routing = v["_routing"]
            .as_str()
            .or_else(|| v["fields"]["_routing"].as_str())
            .map(str::to_string);
        let id = v["_id"].as_str().unwrap().to_string();
        assert!(
            m.insert(id.clone(), (routing, v["_source"].clone()))
                .is_none(),
            "duplicate _id {id}"
        );
    }
    m
}

/// Open search contexts on one index (index-level stats, so concurrent tests on other
/// indices do not interfere): (scroll_current, point_in_time_current if the stat exists).
pub(crate) async fn open_contexts(c: &Cluster, index: &str) -> (u64, Option<u64>) {
    let v = c.ok("GET", &format!("{index}/_stats/search"), None).await;
    let s = &v["_all"]["total"]["search"];
    (
        s["scroll_current"].as_u64().unwrap_or(0),
        s["point_in_time_current"].as_u64(),
    )
}

/// Contexts are released asynchronously on some versions; poll briefly.
pub(crate) async fn assert_no_open_contexts(c: &Cluster, index: &str, what: &str) {
    let mut last = (0, None);
    for _ in 0..20 {
        last = open_contexts(c, index).await;
        if last.0 == 0 && last.1.unwrap_or(0) == 0 {
            return;
        }
        tokio::time::sleep(std::time::Duration::from_millis(250)).await;
    }
    panic!(
        "{what}: open contexts left on {index} (scroll_current, point_in_time_current) = {last:?}"
    );
}

// L-405 (TST-API-010): scroll initial / continuation / clear, unsliced and sliced with
// slices = shards and > shards; slices > index.max_slices_per_scroll is refused with the
// server's reason and nothing is written; no scroll contexts are left in any case.
#[tokio::test]
async fn scroll_unsliced_and_sliced_digest_and_no_open_contexts() {
    let c = Cluster::from_env();
    let src = unique("api_scroll");
    seed_index(&c, &src, 3000).await;
    let want = c.docs(&src).await;
    for slices in ["0", "2", "4", "7"] {
        let (f, o) = dump(
            &c,
            &src,
            &["--slices", slices, "--limit", "250", "--scrollTime", "1m"],
        )
        .await;
        assert_success(&o);
        assert_eq!(file_docs(&f), want, "slices={slices}");
        let _ = std::fs::remove_file(f);
        assert_no_open_contexts(&c, &src, &format!("slices={slices}")).await;
    }
    // over the per-index limit (default 1024): a clear failure carrying the server's reason
    let (f, o) = dump(&c, &src, &["--slices", "1100"]).await;
    let err = String::from_utf8_lossy(&o.stderr);
    assert_eq!(o.status.code(), Some(1), "{err}");
    assert!(err.contains("max_slices_per_scroll"), "{err}");
    assert!(!f.exists(), "nothing written on failure");
    assert_no_open_contexts(&c, &src, "slices=1100").await;
    c.delete_index(&src).await;
}

// L-406 (TST-API-011): PIT unsliced and sliced (2, 4, 16) give the source digest and leave no
// PIT open; versions without a PIT API (ES OSS, OpenSearch < 2.4) fall back to scroll with a
// warning naming the version. Request shapes (tiebreaker per version) are in the goldens.
#[tokio::test]
async fn pit_unsliced_and_sliced_digest_and_no_open_pits() {
    let c = Cluster::from_env();
    let src = unique("api_pit");
    seed_index(&c, &src, 2500).await;
    let want = c.docs(&src).await;
    let has_pit = c.has_pit().await;
    for slices in ["0", "2", "4", "16"] {
        let (f, o) = dump_loud(
            &c,
            &src,
            &["--searchType", "pit", "--slices", slices, "--limit", "200"],
        )
        .await;
        assert_success(&o);
        let err = String::from_utf8_lossy(&o.stderr);
        assert_eq!(
            err.contains("does not support point-in-time search"),
            !has_pit,
            "slices={slices}: fallback warning iff no PIT API: {err}"
        );
        assert_eq!(file_docs(&f), want, "slices={slices}");
        let _ = std::fs::remove_file(f);
        assert_no_open_contexts(&c, &src, &format!("pit slices={slices}")).await;
    }
    c.delete_index(&src).await;
}

// L-407 (TST-API-012): `--searchAfter` without a PIT needs a sort (usage error, exit 2); with a
// non-unique sort (1,000 docs sharing one timestamp) the tool used to stop after the first page
// and exit 0 with 100 of 1,000 documents. It now fails (exit 1, nothing written) naming the
// cause; with a unique tiebreaker it returns every document.
#[tokio::test]
async fn search_after_needs_sort_and_refuses_silent_loss_on_ties() {
    let c = Cluster::from_env();
    let src = unique("api_sa_ties");
    c.delete_index(&src).await;
    c.ok(
        "PUT",
        &src,
        Some(serde_json::json!({
            "settings": {"number_of_shards": 2, "number_of_replicas": 0},
            "mappings": {"properties": {"d": {"type": "date"}, "k": {"type": "keyword"}}}
        })),
    )
    .await;
    let mut nd = String::new();
    for i in 0..1000 {
        nd.push_str(&format!(
            "{{\"index\":{{\"_index\":\"{src}\",\"_id\":\"{i}\"}}}}\n"
        ));
        nd.push_str(&format!(
            "{{\"d\":\"2026-01-01T00:00:00Z\",\"k\":\"k{i:04}\"}}\n"
        ));
    }
    c.bulk_raw(nd).await;
    let want = c.docs(&src).await;

    let (f, o) = dump(&c, &src, &["--searchAfter"]).await;
    assert_eq!(
        o.status.code(),
        Some(2),
        "{}",
        String::from_utf8_lossy(&o.stderr)
    );
    assert!(!f.exists());

    let (f, o) = dump(
        &c,
        &src,
        &[
            "--searchAfter",
            "--limit",
            "100",
            "--searchBody",
            r#"{"sort":[{"d":"asc"}]}"#,
        ],
    )
    .await;
    let err = String::from_utf8_lossy(&o.stderr);
    assert_eq!(o.status.code(), Some(1), "{err}");
    assert!(
        err.contains("retrieved 100 of 1000") && err.contains("not unique"),
        "{err}"
    );
    assert!(!f.exists(), "nothing committed on a shortfall");

    let (f, o) = dump(
        &c,
        &src,
        &[
            "--searchAfter",
            "--limit",
            "100",
            "--searchBody",
            r#"{"sort":[{"d":"asc"},{"k":"asc"}]}"#,
        ],
    )
    .await;
    assert_success(&o);
    assert_eq!(file_docs(&f), want);
    let _ = std::fs::remove_file(f);
    c.delete_index(&src).await;
}

// L-409 (TST-API-014): `--searchWithTemplate` with an inline and a stored (`id`) mustache
// template returns the same documents as the equivalent plain query.
#[tokio::test]
async fn search_templates_inline_and_stored_match_plain_query() {
    let c = Cluster::from_env();
    let src = unique("api_tpl");
    seed_index(&c, &src, 1200).await;
    let tid = unique("api_tpl_script");
    c.ok(
        "PUT",
        &format!("_scripts/{tid}"),
        Some(serde_json::json!({"script": {"lang": "mustache", "source": {"query": {"range": {"n": {"gte": "{{min}}"}}}}}})),
    )
    .await;
    let (plain, o) = dump(
        &c,
        &src,
        &["--searchBody", r#"{"query":{"range":{"n":{"gte":100}}}}"#],
    )
    .await;
    assert_success(&o);
    let want = file_docs(&plain);
    assert_eq!(want.len(), 1100);
    let inline = r#"{"source":{"query":{"range":{"n":{"gte":"{{min}}"}}}},"params":{"min":100}}"#;
    let stored = format!(r#"{{"id":"{tid}","params":{{"min":100}}}}"#);
    for (name, body) in [("inline", inline.to_string()), ("stored", stored)] {
        let (f, o) = dump(
            &c,
            &src,
            &[
                "--searchWithTemplate",
                "--searchBody",
                &body,
                "--limit",
                "300",
            ],
        )
        .await;
        assert_success(&o);
        assert_eq!(file_docs(&f), want, "{name}");
        let _ = std::fs::remove_file(f);
    }
    let _ = std::fs::remove_file(plain);
    let _ = c.call("DELETE", &format!("_scripts/{tid}"), None).await;
    c.delete_index(&src).await;
}

// L-410 (TST-API-015): `--searchBody` variants are passed through and their output shapes
// hold on every version; `size` is overridden by `--limit`; a user `sort` and a user `slice`
// are honoured; restore takes routing from `fields._routing` (meta order `X`, `_X`,
// `fields.X`, `fields._X`, unit-tested in pipeline::bulk).
#[tokio::test]
async fn search_body_variants_and_meta_from_fields() {
    let c = Cluster::from_env();
    let src = unique("api_sbv");
    c.delete_index(&src).await;
    c.ok(
        "PUT",
        &src,
        Some(serde_json::json!({
            "settings": {"number_of_shards": 2, "number_of_replicas": 0},
            "mappings": {"properties": {"n": {"type": "long"}, "k": {"type": "keyword"}, "x": {"type": "keyword", "store": true}}}
        })),
    )
    .await;
    let mut nd = String::new();
    for i in 0..300 {
        nd.push_str(&format!(
            "{{\"index\":{{\"_index\":\"{src}\",\"_id\":\"{i}\",\"routing\":\"r{}\"}}}}\n",
            i % 3
        ));
        nd.push_str(&format!(
            "{{\"n\":{i},\"k\":\"k{i}\",\"x\":\"x{i}\",\"y\":{{\"z\":{i}}}}}\n"
        ));
    }
    c.bulk_raw(nd).await;
    c.ok("POST", &format!("{src}/_refresh"), None).await;
    let parsed = |f: &Path| -> Vec<Value> {
        lines(f)
            .iter()
            .map(|l| serde_json::from_str(l).unwrap())
            .collect()
    };
    let check = |name: &str, docs: &[Value], pred: &dyn Fn(&Value) -> bool| {
        assert_eq!(docs.len(), 300, "{name}");
        for d in docs {
            assert!(pred(d), "{name}: {d}");
            assert!(d["_routing"].is_string(), "{name}: routing kept: {d}");
        }
    };
    type Pred = Box<dyn Fn(&Value) -> bool>;
    let variants: Vec<(&str, &str, Pred)> = vec![
        (
            "includes",
            r#"{"_source":{"includes":["n"]}}"#,
            Box::new(|d| d["_source"].as_object().unwrap().keys().eq(["n"].iter())),
        ),
        (
            "excludes",
            r#"{"_source":{"excludes":["y"]}}"#,
            Box::new(|d| d["_source"].get("y").is_none() && d["_source"]["k"].is_string()),
        ),
        (
            "stored_fields",
            r#"{"stored_fields":["x"]}"#,
            Box::new(|d| d["fields"]["x"][0].is_string() && d.get("_source").is_none()),
        ),
        (
            "docvalue_fields",
            r#"{"docvalue_fields":["k"]}"#,
            Box::new(|d| d["fields"]["k"][0] == d["_source"]["k"]),
        ),
        (
            "fields",
            r#"{"fields":["k"],"_source":false}"#,
            Box::new(|d| d["fields"]["k"][0].is_string() && d.get("_source").is_none()),
        ),
        (
            "size override",
            r#"{"query":{"match_all":{}},"size":5}"#,
            Box::new(|d| d["_source"]["n"].is_u64()),
        ),
    ];
    for (name, body, pred) in &variants {
        let (f, o) = dump(&c, &src, &["--limit", "50", "--searchBody", body]).await;
        assert_success(&o);
        check(name, &parsed(&f), pred.as_ref());
        let _ = std::fs::remove_file(f);
    }
    // user sort is kept (descending n)
    let (f, o) = dump(
        &c,
        &src,
        &[
            "--limit",
            "50",
            "--searchBody",
            r#"{"sort":[{"n":"desc"}]}"#,
        ],
    )
    .await;
    assert_success(&o);
    let ns: Vec<u64> = parsed(&f)
        .iter()
        .map(|d| d["_source"]["n"].as_u64().unwrap())
        .collect();
    assert_eq!(ns, (0..300).rev().collect::<Vec<_>>(), "user sort");
    let _ = std::fs::remove_file(f);
    // a user slice is honoured: same ids as the server's own sliced search
    let sliced = r#"{"query":{"match_all":{}},"slice":{"id":0,"max":2}}"#;
    let (f, o) = dump(&c, &src, &["--limit", "50", "--searchBody", sliced]).await;
    assert_success(&o);
    let got: std::collections::BTreeSet<String> = parsed(&f)
        .iter()
        .map(|d| d["_id"].as_str().unwrap().to_string())
        .collect();
    let v = c
        .ok(
            "POST",
            &format!("{src}/_search?scroll=1m"),
            Some(serde_json::json!({"size": 1000, "slice": {"id": 0, "max": 2}, "_source": false})),
        )
        .await;
    let want: std::collections::BTreeSet<String> = v["hits"]["hits"]
        .as_array()
        .unwrap()
        .iter()
        .map(|h| h["_id"].as_str().unwrap().to_string())
        .collect();
    if let Some(sid) = v["_scroll_id"].as_str() {
        let _ = c
            .call(
                "DELETE",
                "_search/scroll",
                Some(serde_json::json!({"scroll_id": [sid]})),
            )
            .await;
    }
    assert!(!got.is_empty() && got.len() < 300);
    assert_eq!(got, want, "user slice");
    let _ = std::fs::remove_file(f);
    // restore: routing only in fields._routing (array form) is applied
    let dst = unique("api_sbv_dst");
    c.delete_index(&dst).await;
    c.ok(
        "PUT",
        &dst,
        Some(serde_json::json!({"settings": {"number_of_shards": 2, "number_of_replicas": 0}})),
    )
    .await;
    let file = tmp_path("api_fields_routing", "json");
    std::fs::write(&file, "{\"_index\":\"x\",\"_id\":\"a\",\"fields\":{\"_routing\":[\"rq\"]},\"_source\":{\"v\":1}}\n").unwrap();
    let o = run(io(&c, file.to_str().unwrap(), &c.url_with_auth(&dst), &[])).await;
    assert_success(&o);
    let (st, d) = c
        .call("GET", &format!("{dst}/_doc/a?routing=rq"), None)
        .await;
    assert_eq!(st, 200, "{d}");
    assert_eq!(d["_routing"], "rq", "{d}");
    let _ = std::fs::remove_file(file);
    c.delete_index(&src).await;
    c.delete_index(&dst).await;
}

fn report_counts(o: &std::process::Output) -> (u64, u64) {
    let err = String::from_utf8_lossy(&o.stderr);
    let num = |key: &str| {
        err.lines()
            .find_map(|l| {
                l.trim()
                    .strip_prefix(key)
                    .map(|v| v.trim().parse::<u64>().unwrap())
            })
            .unwrap_or(0)
    };
    (num("Total Writes:"), num("Failed Writes:"))
}

// L-412 (TST-API-021): `Total Writes` / `Failed Writes` equal the server's truth (`_count`)
// for mixed bulk responses: mapper_parsing_exception, version_conflict_engine_exception
// (create on an existing id), document_missing_exception (update of a missing id) and
// cluster_block_exception (index.blocks.write).
#[tokio::test]
async fn bulk_item_errors_are_counted_like_the_server() {
    let c = Cluster::from_env();
    let dst = unique("api_bulkerr");
    c.delete_index(&dst).await;
    c.ok(
        "PUT",
        &dst,
        Some(serde_json::json!({"settings": {"number_of_shards": 1, "number_of_replicas": 0}, "mappings": {"properties": {"n": {"type": "long"}}}})),
    )
    .await;
    c.ok(
        "PUT",
        &format!("{dst}/_doc/e1?refresh=true"),
        Some(serde_json::json!({"n": 0})),
    )
    .await;
    let mut input = String::new();
    for i in 0..10 {
        input.push_str(&format!(
            "{{\"_index\":\"x\",\"_id\":\"g{i}\",\"_source\":{{\"n\":{i}}}}}\n"
        ));
    }
    for i in 0..3 {
        input.push_str(&format!(
            "{{\"_index\":\"x\",\"_id\":\"m{i}\",\"_source\":{{\"n\":\"abc\"}}}}\n"
        ));
    }
    input.push_str("{\"_index\":\"x\",\"_id\":\"e1\",\"_source\":{\"n\":5}}\n");
    let file = tmp_path("api_bulkerr", "json");
    std::fs::write(&file, &input).unwrap();
    let o = run_loud(io(
        &c,
        file.to_str().unwrap(),
        &c.url_with_auth(&dst),
        &["--bulkAction", "create", "--limit", "5"],
    ))
    .await;
    let (written, failed) = report_counts(&o);
    let err = String::from_utf8_lossy(&o.stderr);
    // ES >= 8.8 says document_parsing_exception where older versions say mapper_parsing_exception
    assert!(
        (err.contains("mapper_parsing_exception") || err.contains("document_parsing_exception"))
            && err.contains("version_conflict_engine_exception"),
        "{err}"
    );
    assert_eq!((written, failed), (10, 4), "{err}");
    assert_eq!(c.count(&dst).await, 1 + written);

    // update of missing ids: document_missing_exception, nothing written
    let missing = tmp_path("api_bulkerr_upd", "json");
    std::fs::write(&missing, "{\"_index\":\"x\",\"_id\":\"nope1\",\"_source\":{\"n\":1}}\n{\"_index\":\"x\",\"_id\":\"g1\",\"_source\":{\"n\":100}}\n").unwrap();
    let o = run_loud(io(
        &c,
        missing.to_str().unwrap(),
        &c.url_with_auth(&dst),
        &["--bulkAction", "update"],
    ))
    .await;
    let err = String::from_utf8_lossy(&o.stderr);
    assert!(err.contains("document_missing_exception"), "{err}");
    assert_eq!(report_counts(&o), (1, 1), "{err}");
    assert_eq!(c.count(&dst).await, 11);

    // write block: every item fails with cluster_block_exception
    c.ok(
        "PUT",
        &format!("{dst}/_settings"),
        Some(serde_json::json!({"index.blocks.write": true})),
    )
    .await;
    let o = run_loud(io(
        &c,
        file.to_str().unwrap(),
        &c.url_with_auth(&dst),
        &["--limit", "5"],
    ))
    .await;
    let err = String::from_utf8_lossy(&o.stderr);
    assert!(err.contains("cluster_block_exception"), "{err}");
    assert_eq!(report_counts(&o), (0, 14), "{err}");
    c.ok(
        "PUT",
        &format!("{dst}/_settings"),
        Some(serde_json::json!({"index.blocks.write": false})),
    )
    .await;
    assert_eq!(c.count(&dst).await, 11);
    let _ = std::fs::remove_file(file);
    let _ = std::fs::remove_file(missing);
    c.delete_index(&dst).await;
}

/// `GET <index>/_mapping` body without the index name.
async fn mapping_of(c: &Cluster, index: &str) -> Value {
    let v = c.ok("GET", &format!("{index}/_mapping"), None).await;
    v[index]["mappings"].clone()
}

// L-414 (TST-API-030): same-engine mapping round trips (dump `--type mapping` to a file,
// restore into a new index) are lossless for each mapping feature the version supports:
// a field literally named `properties`, runtime fields (ES >= 7.11), `flattened` (ES default
// distribution) / `flat_object` (OS >= 2.7), `dense_vector` / `knn_vector`, `_source`
// disabled, synthetic `_source` (ES >= 8.4 where the licence allows it), `subobjects: false`
// (ES >= 8.3). Cross-engine conversion is the expectation table (L-438/L-439).
#[tokio::test]
async fn mapping_round_trips_are_lossless_per_feature() {
    use serde_json::json;
    let c = Cluster::from_env();
    let v = c.vnum().await;
    let es = matches!(c.engine, Engine::Elasticsearch);
    let oss = c.is_oss().await;
    let mut variants: Vec<(&str, Value)> = vec![
        (
            "base",
            json!({"properties": {"k": {"type": "keyword"}, "n": {"type": "long"}, "obj": {"properties": {"properties": {"type": "keyword"}, "x": {"type": "integer"}}}}}),
        ),
        (
            "source_disabled",
            json!({"_source": {"enabled": false}, "properties": {"k": {"type": "keyword"}}}),
        ),
    ];
    if es && v >= (7, 11) {
        variants.push(("runtime", json!({"runtime": {"rt": {"type": "keyword", "script": {"source": "emit('x')"}}}, "properties": {"k": {"type": "keyword"}}})));
    }
    if es && !oss {
        variants.push((
            "flattened",
            json!({"properties": {"fl": {"type": "flattened"}}}),
        ));
        variants.push((
            "dense_vector",
            json!({"properties": {"dv": {"type": "dense_vector", "dims": 3}}}),
        ));
    }
    if !es && v >= (2, 7) {
        variants.push((
            "flat_object",
            json!({"properties": {"fo": {"type": "flat_object"}}}),
        ));
    }
    if !es {
        variants.push((
            "knn_vector",
            json!({"properties": {"kv": {"type": "knn_vector", "dimension": 3}}}),
        ));
    }
    if es && v >= (8, 3) {
        variants.push((
            "subobjects_false",
            json!({"subobjects": false, "properties": {"a.b": {"type": "keyword"}}}),
        ));
    }
    if es && v >= (8, 4) {
        variants.push((
            "synthetic_source",
            json!({"_source": {"mode": "synthetic"}, "properties": {"k": {"type": "keyword"}}}),
        ));
    }
    let mut done = vec![];
    for (name, mapping) in variants {
        let src = unique(&format!("api_map_{name}"));
        let dst = format!("{src}_dst");
        c.delete_index(&src).await;
        c.delete_index(&dst).await;
        let (st, body) = c
            .call(
                "PUT",
                &src,
                Some(json!({"settings": {"number_of_replicas": 0}, "mappings": mapping})),
            )
            .await;
        if st != 200 {
            // e.g. synthetic _source needs an enterprise licence on ES >= 8.17
            println!("SKIP: mapping variant {name} not accepted by this version: {body}");
            continue;
        }
        let file = tmp_path(&format!("api_map_{name}"), "json");
        let o = run(io(
            &c,
            &c.url_with_auth(&src),
            file.to_str().unwrap(),
            &["--type", "mapping"],
        ))
        .await;
        assert_success(&o);
        let o = run(io(
            &c,
            file.to_str().unwrap(),
            &c.url_with_auth(&dst),
            &["--type", "mapping"],
        ))
        .await;
        assert_success(&o);
        assert_eq!(
            mapping_of(&c, &dst).await,
            mapping_of(&c, &src).await,
            "{name}"
        );
        let _ = std::fs::remove_file(file);
        c.delete_index(&src).await;
        c.delete_index(&dst).await;
        done.push(name);
    }
    println!("mapping round trips: {done:?}");
    assert!(done.contains(&"base") && done.contains(&"source_disabled"));
}

/// `index` settings of `index` with the keys every new index generates removed.
async fn settings_of(c: &Cluster, index: &str) -> Value {
    let v = c.ok("GET", &format!("{index}/_settings"), None).await;
    let mut s = v[index]["settings"]["index"].clone();
    if let Some(m) = s.as_object_mut() {
        for k in [
            "uuid",
            "creation_date",
            "provided_name",
            "version",
            "history",
            "resize",
        ] {
            m.remove(k);
        }
    }
    s
}

// L-415 (TST-API-031): `--type settings` dumps strip generated keys; restoring them into a
// new index reproduces the source settings (analysis included) modulo the generated-key
// allow-list; `--excludeSettings` drops the named keys (with or without `index.`).
#[tokio::test]
async fn settings_round_trip_strips_generated_keys_and_honours_exclude() {
    use serde_json::json;
    let c = Cluster::from_env();
    let src = unique("api_set");
    c.delete_index(&src).await;
    c.ok(
        "PUT",
        &src,
        Some(json!({"settings": {
            "number_of_shards": 2, "number_of_replicas": 0, "refresh_interval": "5s", "max_result_window": 20000,
            "analysis": {"analyzer": {"a1": {"type": "custom", "tokenizer": "whitespace", "filter": ["lowercase"]}},
                         "normalizer": {"lc": {"type": "custom", "filter": ["lowercase"]}}}
        }})),
    )
    .await;
    let file = tmp_path("api_set", "json");
    assert_success(
        &run(io(
            &c,
            &c.url_with_auth(&src),
            file.to_str().unwrap(),
            &["--type", "settings"],
        ))
        .await,
    );
    let dumped = std::fs::read_to_string(&file).unwrap();
    for k in [
        "\\\"uuid\\\"",
        "creation_date",
        "provided_name",
        "\\\"version\\\"",
    ] {
        assert!(
            !dumped.contains(k),
            "generated key {k} in the dump: {dumped}"
        );
    }
    let dst = format!("{src}_dst");
    c.delete_index(&dst).await;
    assert_success(
        &run(io(
            &c,
            file.to_str().unwrap(),
            &c.url_with_auth(&dst),
            &["--type", "settings"],
        ))
        .await,
    );
    assert_eq!(settings_of(&c, &dst).await, settings_of(&c, &src).await);

    let dst2 = format!("{src}_dst2");
    c.delete_index(&dst2).await;
    let o = run(io(
        &c,
        file.to_str().unwrap(),
        &c.url_with_auth(&dst2),
        &[
            "--type",
            "settings",
            "--excludeSettings",
            "refresh_interval,index.max_result_window",
        ],
    ))
    .await;
    assert_success(&o);
    let mut want = settings_of(&c, &src).await;
    want.as_object_mut().unwrap().remove("refresh_interval");
    want.as_object_mut().unwrap().remove("max_result_window");
    assert_eq!(settings_of(&c, &dst2).await, want);
    let _ = std::fs::remove_file(file);
    for i in [&src, &dst, &dst2] {
        c.delete_index(i).await;
    }
}

/// Index names in a `--type index|settings|mapping` dump file (the document may be a
/// JSON-encoded string, as elasticdump writes it).
fn dumped_index_names(path: &Path) -> Vec<String> {
    let mut out = vec![];
    for l in lines(path) {
        let mut v: Value = serde_json::from_str(&l).unwrap();
        if let Some(s) = v.as_str() {
            v = serde_json::from_str(s).unwrap();
        }
        out.extend(v.as_object().into_iter().flat_map(|o| o.keys().cloned()));
    }
    out.sort();
    out
}

// L-416 (TST-API-032): `--type index` over a wildcard includes open visible indices with
// their aliases (filter, routing); hidden and closed indices are excluded (the server's
// default `expand_wildcards=open`), a hidden index named explicitly is included, and data
// stream backing indices are skipped with a warning naming the data stream.
#[tokio::test]
async fn index_type_inclusion_hidden_closed_and_data_streams() {
    use serde_json::json;
    let c = Cluster::from_env();
    let v = c.vnum().await;
    let es = matches!(c.engine, Engine::Elasticsearch);
    let p = unique("api_incl");
    let (vis, hid, clo, ds) = (
        format!("{p}_a"),
        format!("{p}_h"),
        format!("{p}_c"),
        format!("{p}_ds"),
    );
    c.ok("PUT", &vis, Some(json!({"settings": {"number_of_replicas": 0}, "aliases": {format!("{p}_al"): {"filter": {"term": {"k": "x"}}, "routing": "r1"}}}))).await;
    c.ok(
        "PUT",
        &hid,
        Some(json!({"settings": {"number_of_replicas": 0, "index.hidden": true}})),
    )
    .await;
    c.ok(
        "PUT",
        &clo,
        Some(json!({"settings": {"number_of_replicas": 0}})),
    )
    .await;
    c.ok("POST", &format!("{clo}/_close"), None).await;
    let tpl = format!("{p}_dstpl");
    let has_ds = if es {
        v >= (7, 9) && !c.is_oss().await
    } else {
        true
    };
    if has_ds {
        c.ok("PUT", &format!("_index_template/{tpl}"), Some(json!({"index_patterns": [format!("{ds}*")], "data_stream": {}, "priority": 500, "template": {"settings": {"number_of_replicas": 0}}}))).await;
        c.ok("PUT", &format!("_data_stream/{ds}"), None).await;
    }
    let file = tmp_path("api_incl", "json");
    let o = run_loud(io(
        &c,
        &c.url_with_auth(&format!("{p}_*")),
        file.to_str().unwrap(),
        &["--type", "index"],
    ))
    .await;
    assert_success(&o);
    let err = String::from_utf8_lossy(&o.stderr);
    assert_eq!(dumped_index_names(&file), vec![vis.clone()], "{err}");
    if has_ds {
        assert!(
            err.contains("skipping data stream backing index") && err.contains(&ds),
            "{err}"
        );
    }
    let doc: Value = {
        let l = &lines(&file)[0];
        let v: Value = serde_json::from_str(l).unwrap();
        v.as_str()
            .map(|s| serde_json::from_str(s).unwrap())
            .unwrap_or(v)
    };
    let al = &doc[&vis]["aliases"][format!("{p}_al")];
    assert_eq!(al["filter"]["term"]["k"], "x", "{doc}");
    assert_eq!(al["index_routing"], "r1", "{doc}");
    let _ = std::fs::remove_file(&file);
    // a hidden index named explicitly is dumped
    let o = run(io(
        &c,
        &c.url_with_auth(&hid),
        file.to_str().unwrap(),
        &["--type", "index"],
    ))
    .await;
    assert_success(&o);
    assert_eq!(dumped_index_names(&file), vec![hid.clone()]);
    let _ = std::fs::remove_file(&file);
    if has_ds {
        let _ = c.call("DELETE", &format!("_data_stream/{ds}"), None).await;
        let _ = c
            .call("DELETE", &format!("_index_template/{tpl}"), None)
            .await;
    }
    for i in [&vis, &hid, &clo] {
        c.delete_index(i).await;
    }
}


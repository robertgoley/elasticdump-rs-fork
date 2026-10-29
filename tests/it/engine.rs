//! Engine-specific expectations: the same suite runs against OpenSearch and
//! Elasticsearch; these tests assert what must differ between them.

use serde_json::Value;

use crate::harness::*;

fn input_args(c: &Cluster, index: &str, out: &std::path::Path) -> Vec<String> {
    let mut a = s(&[
        "--input",
        &c.url_with_auth(index),
        "--output",
        out.to_str().unwrap(),
    ]);
    a.extend(c.tls_args("input"));
    a
}

#[tokio::test]
async fn pit_request_shape_matches_engine() {
    let c = Cluster::from_env();
    let index = unique("eng_pit");
    seed_index(&c, &index, 300).await;
    let out = tmp_path("eng_pit", "json");
    let mut a = input_args(&c, &index, &out);
    a.extend(s(&["--searchAfter", "--pit", "--limit", "50", "--debug"]));
    let o = run_loud(a).await;
    assert_success(&o);
    assert_eq!(lines(&out).len() as u64, c.count(&index).await);
    let log = String::from_utf8_lossy(&o.stderr);
    match c.engine {
        Engine::Elasticsearch => {
            assert!(log.contains(&format!("/{index}/_pit?keep_alive=")), "{log}");
            assert!(log.contains(r#""sort":["_shard_doc"]"#), "{log}");
            assert!(log.contains("DELETE ") && log.contains("/_pit"), "{log}");
            assert!(!log.contains("point_in_time"), "{log}");
        }
        Engine::OpenSearch => {
            assert!(log.contains("/_search/point_in_time?keep_alive="), "{log}");
            // key order inside the sort clause is not significant
            assert!(
                log.contains(r#"{"_id":{"#) && log.contains(r#""unmapped_type":"keyword""#),
                "{log}"
            );
            assert!(!log.contains("_shard_doc"), "{log}");
        }
    }
    c.delete_index(&index).await;
}

#[tokio::test]
async fn typeless_dump_shapes() {
    let c = Cluster::from_env();
    let index = unique("eng_tl");
    seed_index(&c, &index, 20).await;
    // data: no _type on ES >= 8 / OpenSearch
    let out = tmp_path("eng_tl", "json");
    assert_success(&run(input_args(&c, &index, &out)).await);
    for l in lines(&out) {
        let v: Value = serde_json::from_str(&l).unwrap();
        assert!(v.get("_type").is_none(), "{l}");
    }
    // mapping: {index: {mappings: {properties: ..}}}, no type level
    let m = tmp_path("eng_tl_m", "json");
    let mut a = input_args(&c, &index, &m);
    a.extend(s(&["--type", "mapping"]));
    assert_success(&run(a).await);
    let v: Value = serde_json::from_str(&lines(&m)[0]).unwrap();
    assert!(v[&index]["mappings"]["properties"]["k"].is_object(), "{v}");
    c.delete_index(&index).await;
}

#[tokio::test]
async fn settings_dump_keeps_engine_keys_and_drops_generated_ones() {
    let c = Cluster::from_env();
    let index = unique("eng_set");
    seed_index(&c, &index, 5).await;
    let f = tmp_path("eng_set", "json");
    let mut a = input_args(&c, &index, &f);
    a.extend(s(&["--type", "settings"]));
    assert_success(&run(a).await);
    let line: Value = serde_json::from_str(&lines(&f)[0]).unwrap();
    let v: Value = serde_json::from_str(line.as_str().unwrap()).unwrap();
    let idx = &v[&index]["settings"]["index"];
    for k in ["uuid", "creation_date", "provided_name", "version"] {
        assert!(idx.get(k).is_none(), "{k}: {idx}");
    }
    let live = c.ok("GET", &format!("{index}/_settings"), None).await;
    let live = &live[&index]["settings"]["index"];
    match c.engine {
        Engine::Elasticsearch => {
            // ES-only allocation hint is kept in the file (stripped only on
            // cross-engine restores)
            assert_eq!(
                idx["routing"]["allocation"]["include"]["_tier_preference"],
                live["routing"]["allocation"]["include"]["_tier_preference"]
            );
            assert!(idx.get("replication").is_none());
        }
        Engine::OpenSearch => {
            assert_eq!(idx.get("replication"), live.get("replication"), "{idx}");
            assert!(
                idx["routing"]["allocation"]["include"]
                    .get("_tier_preference")
                    .is_none()
            );
        }
    }
    // same-engine restore accepts every key in the file
    let dst = unique("eng_set_dst");
    c.delete_index(&dst).await;
    let mut a = s(&[
        "--input",
        f.to_str().unwrap(),
        "--output",
        &c.url_with_auth(&dst),
        "--type",
        "settings",
    ]);
    a.extend(c.tls_args("output"));
    assert_success(&run(a).await);
    c.delete_index(&index).await;
    c.delete_index(&dst).await;
}

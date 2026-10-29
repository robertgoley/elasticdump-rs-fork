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
    if !c.has_pit().await {
        // ES < 7.10 / OpenSearch < 2.4: clear warning, then scroll
        assert!(
            log.contains("does not support point-in-time search"),
            "{log}"
        );
        assert!(log.contains("using scroll instead"), "{log}");
        assert!(log.contains("scroll="), "{log}");
        assert!(
            !log.contains("/_pit?") && !log.contains("point_in_time?"),
            "{log}"
        );
        c.delete_index(&index).await;
        return;
    }
    match c.engine {
        Engine::Elasticsearch => {
            assert!(log.contains(&format!("/{index}/_pit?keep_alive=")), "{log}");
            if c.vnum().await >= (7, 12) {
                assert!(log.contains(r#""sort":["_shard_doc"]"#), "{log}");
            } else {
                // `_shard_doc` arrived in 7.12: `_doc` + `_id` tiebreaker
                assert!(
                    log.contains(r#"{"_id":{"#) && !log.contains("_shard_doc"),
                    "{log}"
                );
            }
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
    // data: no _type on ES >= 8 / OpenSearch >= 2; ES 7 hits still carry
    // `"_type":"_doc"`, which is kept in the file (elasticdump does the same)
    // and never sent back to a 7+/OpenSearch target.
    // ES 7 and OpenSearch 1.x hits still carry `"_type":"_doc"`
    let es7 = c.major().await
        == if c.engine == Engine::Elasticsearch {
            7
        } else {
            1
        };
    let out = tmp_path("eng_tl", "json");
    assert_success(&run(input_args(&c, &index, &out)).await);
    for l in lines(&out) {
        let v: Value = serde_json::from_str(&l).unwrap();
        match v.get("_type") {
            None => {}
            Some(t) => assert!(es7 && t == "_doc", "{l}"),
        }
    }
    // a file with `_type` restores into a typeless cluster
    let back = unique("eng_tl_back");
    c.delete_index(&back).await;
    // explicit mapping first: ES 7 dynamically maps the seed's out-of-range
    // integers as `long` and rejects them (ES 8+ maps them as float)
    c.copy_shape(&index, &back).await;
    let mut a = s(&[
        "--input",
        out.to_str().unwrap(),
        "--output",
        &c.url_with_auth(&back),
    ]);
    a.extend(c.tls_args("output"));
    assert_success(&run(a).await);
    assert_eq!(c.docs(&back).await, c.docs(&index).await);
    c.delete_index(&back).await;
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

//! L-657 (TST-BRK-XV-006): ES 6.x-shaped search responses (`hits.total` as a bare integer,
//! as with `rest_total_hits_as_int`, and `_type` in every hit, as on 7.x for indices created
//! on 6.x) dump every document end to end, by scroll and by search_after; the total check
//! still works with an integer total; `_type` is written as served (Node writes hits as
//! served) and is sent back in the bulk action only to a target that still has types (< 7).

mod support;

use std::process::Output;

use support::router_mock::{Recorded, Reply, Route, RouterMock};

const ES717: &str =
    r#"{"version":{"number":"7.17.29","build_flavor":"default"},"tagline":"You Know, for Search"}"#;
const ES68: &str =
    r#"{"version":{"number":"6.8.23","build_flavor":"default"},"tagline":"You Know, for Search"}"#;
const ES819: &str =
    r#"{"version":{"number":"8.19.0","build_flavor":"default"},"tagline":"You Know, for Search"}"#;
const CLEARED: &str = r#"{"succeeded":true,"num_freed":1}"#;

async fn run(args: &[&str]) -> Output {
    let mut c = tokio::process::Command::new(env!("CARGO_BIN_EXE_searchdump"));
    for v in [
        "SEARCHDUMP_INPUT_USERNAME",
        "SEARCHDUMP_INPUT_PASSWORD",
        "SEARCHDUMP_OUTPUT_USERNAME",
        "SEARCHDUMP_OUTPUT_PASSWORD",
        "RUST_LOG",
    ] {
        c.env_remove(v);
    }
    c.args(args).output().await.unwrap()
}

fn stderr(o: &Output) -> String {
    String::from_utf8_lossy(&o.stderr).into_owned()
}

/// A 6.x-shaped hit: `_type` and an integer sort value.
fn hit(id: u64, ty: &str) -> String {
    format!(
        r#"{{"_index":"idx","_type":"{ty}","_id":"{id}","_score":null,"_source":{{"n":{id}}},"sort":[{id}]}}"#
    )
}

/// A page with `hits.total` as a bare integer.
fn page(hits: &[String], total: u64) -> String {
    format!(
        r#"{{"_scroll_id":"s1","took":1,"timed_out":false,"_shards":{{"total":1,"successful":1,"skipped":0,"failed":0}},"hits":{{"total":{total},"max_score":null,"hits":[{}]}}}}"#,
        hits.join(",")
    )
}

fn first() -> Vec<String> {
    (1..=3).map(|i| hit(i, "_doc")).collect()
}

fn second() -> Vec<String> {
    (4..=5).map(|i| hit(i, "doc")).collect()
}

async fn scroll_mock(info: &str, total: u64) -> RouterMock {
    RouterMock::builder()
        .route(Route::exact("GET", "/").reply(Reply::ok(info)))
        .route(Route::contains("POST", "/idx/_search").reply(Reply::ok(page(&first(), total))))
        .route(Route::exact("POST", "/_search/scroll").replies([
            Reply::ok(page(&second(), total)),
            Reply::ok(page(&[], total)),
        ]))
        .fallback(Reply::ok(CLEARED))
        .start()
        .await
}

fn docs(path: &std::path::Path) -> Vec<serde_json::Value> {
    std::fs::read_to_string(path)
        .unwrap()
        .lines()
        .map(|l| serde_json::from_str(l).unwrap())
        .collect()
}

fn assert_all_five(label: &str, o: &Output, out: &std::path::Path) {
    let err = stderr(o);
    assert_eq!(o.status.code(), Some(0), "{label}: {err}");
    assert!(!err.contains("invalid type"), "{label}: {err}");
    assert!(!err.contains("retrieved"), "{label}: {err}");
    let d = docs(out);
    let ids: Vec<&str> = d.iter().map(|d| d["_id"].as_str().unwrap()).collect();
    assert_eq!(ids, ["1", "2", "3", "4", "5"], "{label}");
    // `_type` is written as served (Node writes the hit as the server sent it)
    let types: Vec<&str> = d.iter().map(|d| d["_type"].as_str().unwrap()).collect();
    assert_eq!(types, ["_doc", "_doc", "_doc", "doc", "doc"], "{label}");
    for (i, d) in d.iter().enumerate() {
        assert_eq!(d["_source"]["n"], i as u64 + 1, "{label}");
    }
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn l657_integer_totals_and_types_dump_every_document_by_scroll() {
    let dir = tempfile::tempdir().unwrap();
    for (label, info) in [("7.17", ES717), ("6.8", ES68)] {
        let m = scroll_mock(info, 5).await;
        let out = dir.path().join(format!("scroll-{label}.json"));
        let o = run(&[
            "--input",
            &format!("{}/idx", m.url()),
            "--output",
            out.to_str().unwrap(),
            "--limit",
            "3",
        ])
        .await;
        assert_all_five(label, &o, &out);
        let reqs: Vec<Recorded> = m.requests();
        assert_eq!(
            reqs.iter()
                .filter(|r| r.path == "/_search/scroll" && r.method == "POST")
                .count(),
            2,
            "{label}"
        );
    }
}

// The total check works with an integer total: 6 promised, 5 served, is not a complete dump.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn l657_an_integer_total_the_pages_do_not_reach_fails_the_dump() {
    let dir = tempfile::tempdir().unwrap();
    let m = scroll_mock(ES717, 6).await;
    let out = dir.path().join("short.json");
    let o = run(&[
        "--input",
        &format!("{}/idx", m.url()),
        "--output",
        out.to_str().unwrap(),
        "--limit",
        "3",
    ])
    .await;
    let err = stderr(&o);
    assert_ne!(o.status.code(), Some(0), "{err}");
    assert!(err.contains("retrieved 5 of 6 documents"), "{err}");
    assert!(!err.contains("invalid type"), "{err}");
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn l657_integer_totals_and_types_dump_every_document_by_search_after() {
    let dir = tempfile::tempdir().unwrap();
    let m = RouterMock::builder()
        .route(Route::exact("GET", "/").reply(Reply::ok(ES717)))
        .route(
            Route::contains("POST", "/idx/_search")
                .body_contains("search_after")
                .replies([Reply::ok(page(&second(), 5)), Reply::ok(page(&[], 5))]),
        )
        .route(Route::contains("POST", "/idx/_search").reply(Reply::ok(page(&first(), 5))))
        .fallback(Reply::ok(CLEARED))
        .start()
        .await;
    let out = dir.path().join("sa.json");
    let o = run(&[
        "--input",
        &format!("{}/idx", m.url()),
        "--output",
        out.to_str().unwrap(),
        "--limit",
        "3",
        "--searchAfter",
        "--searchBody",
        r#"{"query":{"match_all":{}},"sort":[{"n":"asc"}]}"#,
    ])
    .await;
    assert_all_five("searchAfter", &o, &out);
    let afters: Vec<serde_json::Value> = m
        .requests()
        .iter()
        .filter(|r| r.path.contains("/idx/_search"))
        .filter_map(|r| serde_json::from_slice::<serde_json::Value>(&r.body).ok())
        .filter_map(|b| b.get("search_after").cloned())
        .collect();
    // the second page (2 < --limit 3) may end the dump; a further request asks after 5
    assert_eq!(afters.first(), Some(&serde_json::json!([3])), "{afters:?}");
    assert!(
        afters.iter().skip(1).all(|a| a == &serde_json::json!([5])),
        "{afters:?}"
    );
}

// The dumped lines go back: `_type` is kept in the bulk action for a 6.x target and left out
// for 7.x and 8.x (which reject or deprecate it).
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn l657_typed_lines_restore_with_type_only_where_types_exist() {
    let dir = tempfile::tempdir().unwrap();
    let input = dir.path().join("typed.json");
    let lines: String = first()
        .iter()
        .chain(second().iter())
        .map(|h| {
            let mut v: serde_json::Value = serde_json::from_str(h).unwrap();
            v.as_object_mut().unwrap().remove("sort");
            v.to_string() + "\n"
        })
        .collect();
    std::fs::write(&input, lines).unwrap();
    for (label, info, typed) in [
        ("6.8", ES68, true),
        ("7.17", ES717, false),
        ("8.19", ES819, false),
    ] {
        let m = RouterMock::builder()
            .route(Route::exact("GET", "/").reply(Reply::ok(info)))
            .route(Route::contains("POST", "_bulk").reply(Reply::ok(format!(
                r#"{{"took":1,"errors":false,"items":[{}]}}"#,
                [r#"{"index":{"status":201}}"#; 5].join(",")
            ))))
            .fallback(Reply::ok("{}"))
            .start()
            .await;
        let o = run(&[
            "--input",
            input.to_str().unwrap(),
            "--output",
            &format!("{}/dst", m.url()),
        ])
        .await;
        let err = stderr(&o);
        assert_eq!(o.status.code(), Some(0), "{label}: {err}");
        let bulk: String = m
            .requests()
            .iter()
            .filter(|r| r.path.contains("_bulk"))
            .map(|r| r.body_str())
            .collect();
        let actions: Vec<serde_json::Value> = bulk
            .lines()
            .step_by(2)
            .map(|l| serde_json::from_str(l).unwrap())
            .collect();
        assert_eq!(actions.len(), 5, "{label}: {bulk}");
        for (a, want) in actions.iter().zip(["_doc", "_doc", "_doc", "doc", "doc"]) {
            let meta = &a["index"];
            assert_eq!(meta["_index"], "dst", "{label}: {a}");
            if typed {
                assert_eq!(meta["_type"], want, "{label}: {a}");
            } else {
                assert!(meta.get("_type").is_none(), "{label}: {a}");
            }
        }
    }
}

//! Real HTTP regression tests: no live engine needed.
use super::*;
use crate::router_mock::{Fault, Reply, Route, RouterMock};
use serde_json::{Value, json};
use std::time::Duration;

fn common(slices: usize) -> Common {
    let cli = Cli::try_parse_from([
        "edverify",
        "digest",
        "--slices",
        &slices.to_string(),
        "http://localhost/index",
    ])
    .unwrap();
    match cli.cmd {
        Cmd::Digest { c, .. } => c,
        _ => unreachable!(),
    }
}

fn page(id: &str, docs: bool) -> Reply {
    let hits = if docs {
        json!([{"_index":"index", "_id":"1", "_source":{"x":1}}])
    } else {
        json!([])
    };
    Reply::ok(json!({"_scroll_id":id, "_shards":{"failed":0}, "hits":{"hits":hits}}).to_string())
}

fn count(n: u64) -> Route {
    Route::exact("GET", "/index/_count").reply(Reply::ok(json!({"count":n}).to_string()))
}

fn clear() -> Route {
    Route::exact("DELETE", "/_search/scroll").reply(Reply::ok(r#"{"succeeded":true}"#))
}

async fn scan_mock(mock: &RouterMock, slices: usize) -> Result<SliceResult> {
    let c = common(slices);
    scan_cluster(&format!("{}/index", mock.url()), &c, &opts(&c), None).await
}

fn deleted(mock: &RouterMock) -> Vec<String> {
    let reqs = mock.requests();
    let r = reqs
        .iter()
        .find(|r| r.method == "DELETE")
        .expect("cleanup requested");
    assert_eq!(r.path, "/_search/scroll");
    let v: Value = serde_json::from_slice(&r.body).unwrap();
    let mut ids: Vec<_> = v["scroll_id"]
        .as_array()
        .unwrap()
        .iter()
        .map(|v| v.as_str().unwrap().to_string())
        .collect();
    assert!(ids.iter().all(|id| !id.is_empty() && id != "_all"));
    ids.sort();
    ids
}

#[test]
fn defaults_and_retry_bounds_are_unchanged_for_large_tiers() {
    let cli = Cli::try_parse_from(["edverify", "digest", "input.ndjson"]).unwrap();
    let Cmd::Digest { c, .. } = cli.cmd else {
        unreachable!()
    };
    assert_eq!((c.slices, c.size), (4, 1000));
    let delays: Vec<_> = (0..HTTP_RETRIES)
        .map(|i| retry_delay(i).as_millis())
        .collect();
    assert_eq!(delays, [250, 500, 1000, 2000, 4000, 4000]);
    assert_eq!(retry_delay(usize::MAX), Duration::from_secs(4));
}

#[tokio::test]
async fn success_cleans_rotated_ids_before_count() {
    let mock = RouterMock::builder()
        .route(Route::exact("POST", "/index/_search").reply(page("first", true)))
        .route(Route::exact("POST", "/_search/scroll").reply(page("rotated", false)))
        .route(clear())
        .route(count(1))
        .start()
        .await;
    assert_eq!(scan_mock(&mock, 1).await.unwrap().0.count, 1);
    assert_eq!(deleted(&mock), ["first", "rotated"]);
    let rs = mock.requests();
    assert!(
        rs.iter().position(|r| r.method == "DELETE").unwrap()
            < rs.iter().position(|r| r.path.ends_with("_count")).unwrap()
    );
    assert!(
        !rs.iter()
            .any(|r| r.path.contains("point_in_time") || r.path.contains("_pit"))
    );
}

#[tokio::test]
async fn explicit_429_retries_initial_continuation_count_and_cleanup() {
    let busy = Reply::json(429, r#"{"error":"breaker"}"#);
    let mock = RouterMock::builder()
        .route(Route::exact("POST", "/index/_search").replies([busy.clone(), page("own", true)]))
        .route(Route::exact("POST", "/_search/scroll").replies([busy.clone(), page("own", false)]))
        .route(
            Route::exact("DELETE", "/_search/scroll")
                .replies([busy.clone(), Reply::ok(r#"{"succeeded":true}"#)]),
        )
        .route(Route::exact("GET", "/index/_count").replies([busy, Reply::ok(r#"{"count":1}"#)]))
        .start()
        .await;
    assert_eq!(scan_mock(&mock, 1).await.unwrap().0.count, 1);
    for method_path in [
        ("POST", "/index/_search"),
        ("POST", "/_search/scroll"),
        ("DELETE", "/_search/scroll"),
        ("GET", "/index/_count"),
    ] {
        let rs: Vec<_> = mock
            .requests()
            .into_iter()
            .filter(|r| (r.method.as_str(), r.path.as_str()) == method_path)
            .collect();
        assert_eq!(rs.len(), 2);
        assert_eq!(rs[0].body, rs[1].body);
    }
}

#[tokio::test]
async fn exhausted_429_still_cleans_and_has_a_finite_attempt_count() {
    let mock = RouterMock::builder()
        .route(Route::exact("POST", "/index/_search").reply(page("own", true)))
        .route(Route::exact("POST", "/_search/scroll").reply(Reply::json(429, "{}")))
        .route(clear())
        .start()
        .await;
    assert!(
        scan_mock(&mock, 1)
            .await
            .err()
            .expect("expected error")
            .to_string()
            .contains("429")
    );
    assert_eq!(
        mock.requests()
            .iter()
            .filter(|r| r.method == "POST" && r.path == "/_search/scroll")
            .count(),
        HTTP_RETRIES + 1
    );
    assert_eq!(deleted(&mock), ["own"]);
}

#[tokio::test]
async fn non_429_http_failure_is_not_retried_but_cleans() {
    let mock = RouterMock::builder()
        .route(Route::exact("POST", "/index/_search").reply(page("own", true)))
        .route(Route::exact("POST", "/_search/scroll").reply(Reply::json(500, "{}")))
        .route(clear())
        .start()
        .await;
    assert!(
        scan_mock(&mock, 1)
            .await
            .err()
            .expect("expected error")
            .to_string()
            .contains("500")
    );
    assert_eq!(
        mock.requests()
            .iter()
            .filter(|r| r.method == "POST" && r.path == "/_search/scroll")
            .count(),
        1
    );
    assert_eq!(deleted(&mock), ["own"]);
}

#[tokio::test]
async fn malformed_page_still_cleans_known_id() {
    let mock = RouterMock::builder()
        .route(
            Route::exact("POST", "/index/_search")
                .reply(Reply::ok(r#"{"_scroll_id":"own","hits":{}}"#)),
        )
        .route(clear())
        .start()
        .await;
    assert!(
        scan_mock(&mock, 1)
            .await
            .err()
            .expect("expected error")
            .to_string()
            .contains("hits.hits")
    );
    assert_eq!(deleted(&mock), ["own"]);
}

#[tokio::test]
async fn shard_failure_still_cleans_known_id() {
    let mock = RouterMock::builder()
        .route(
            Route::exact("POST", "/index/_search")
                .reply(Reply::ok(r#"{"_scroll_id":"own","_shards":{"failed":1}}"#)),
        )
        .route(clear())
        .start()
        .await;
    assert!(
        scan_mock(&mock, 1)
            .await
            .err()
            .expect("expected error")
            .to_string()
            .contains("shard(s) failed")
    );
    assert_eq!(deleted(&mock), ["own"]);
}

#[tokio::test]
async fn an_early_slice_error_does_not_abort_the_slow_sibling() {
    let mock = RouterMock::builder()
        .route(
            Route::exact("POST", "/index/_search")
                .body(|b| serde_json::from_slice::<Value>(b).unwrap()["slice"]["id"] == 0)
                .reply(Reply::ok(
                    r#"{"_scroll_id":"failed","_shards":{"failed":1}}"#,
                )),
        )
        .route(
            Route::exact("POST", "/index/_search")
                .reply(page("late", true).with_fault(Fault::Delay(Duration::from_millis(100)))),
        )
        .route(Route::exact("POST", "/_search/scroll").reply(page("late-rotated", false)))
        .route(clear())
        .start()
        .await;
    assert!(scan_mock(&mock, 2).await.is_err());
    assert_eq!(deleted(&mock), ["failed", "late", "late-rotated"]);
}

#[tokio::test]
async fn a_slice_panic_is_joined_and_its_context_is_cleaned() {
    let mock = RouterMock::builder().route(clear()).start().await;
    let c = common(1);
    let t = target(&format!("{}/index", mock.url())).unwrap();
    let cl = client(&c).unwrap();
    let seen: ScrollIds = Arc::new(Mutex::new(HashSet::new()));
    let mut set = tokio::task::JoinSet::new();
    let own = Arc::clone(&seen);
    set.spawn(async move {
        own.lock().unwrap().insert("before-panic".into());
        panic!("synthetic slice panic");
    });
    let err = join_slices(set, &cl, &t, seen, None, DEFAULT_BUCKETS)
        .await
        .err()
        .expect("expected error");
    assert!(err.to_string().contains("panic"));
    assert_eq!(deleted(&mock), ["before-panic"]);
}

#[tokio::test]
async fn count_error_happens_after_cleanup() {
    let mock = RouterMock::builder()
        .route(Route::exact("POST", "/index/_search").reply(page("own", false)))
        .route(clear())
        .route(Route::exact("GET", "/index/_count").reply(Reply::json(500, "{}")))
        .start()
        .await;
    assert!(
        scan_mock(&mock, 1)
            .await
            .err()
            .expect("expected error")
            .to_string()
            .contains("500")
    );
    assert_eq!(deleted(&mock), ["own"]);
}

#[tokio::test]
async fn cleanup_failure_is_not_hidden_on_success_or_error() {
    for first in [
        page("own", false),
        Reply::ok(r#"{"_scroll_id":"own","hits":{}}"#),
    ] {
        let mock = RouterMock::builder()
            .route(Route::exact("POST", "/index/_search").reply(first))
            .route(Route::exact("DELETE", "/_search/scroll").reply(Reply::json(500, "{}")))
            .start()
            .await;
        let err = scan_mock(&mock, 1).await.err().expect("expected error");
        let msg = format!("{err:#}");
        assert!(msg.contains("cleanup"), "{msg}");
        assert!(msg.contains("500"), "{msg}");
        assert_eq!(deleted(&mock), ["own"]);
    }
}

#[tokio::test]
async fn cleanup_false_fails_but_already_expired_ids_are_ok() {
    for (reply, ok) in [
        (Reply::ok(r#"{"succeeded":false}"#), false),
        (Reply::json(404, "{}"), true),
    ] {
        let mock = RouterMock::builder()
            .route(Route::exact("POST", "/index/_search").reply(page("own", false)))
            .route(Route::exact("DELETE", "/_search/scroll").reply(reply))
            .route(count(0))
            .start()
            .await;
        assert_eq!(scan_mock(&mock, 1).await.is_ok(), ok);
        assert_eq!(deleted(&mock), ["own"]);
    }
}

#[tokio::test]
async fn wildcard_id_from_server_never_becomes_cluster_wide_deletion() {
    let mock = RouterMock::builder()
        .route(Route::exact("POST", "/index/_search").reply(page("_all", false)))
        .start()
        .await;
    assert!(
        scan_mock(&mock, 1)
            .await
            .err()
            .expect("expected error")
            .to_string()
            .contains("invalid scroll ID")
    );
    assert!(!mock.requests().iter().any(|r| r.method == "DELETE"));
}

//! Phase A6 chaos against a live engine (TST-BRK, L-6xx). The fault-only cases run against
//! in-process mocks in `tests/chaos_brk.rs`.

use std::time::Duration;

use crate::harness::*;

fn io_args(c: &Cluster, input: &str, output: &str) -> Vec<String> {
    let mut a = s(&["--input", input, "--output", output]);
    if input.starts_with("http") {
        a.extend(c.tls_args("input"));
    }
    if output.starts_with("http") {
        a.extend(c.tls_args("output"));
    }
    a
}

/// Start a slow dump (5 docs per page, one page every 1.5 s), drop the cluster's search
/// contexts after `after`, and return the run's output.
async fn dump_and_lose_context(
    c: &Cluster,
    index: &str,
    extra: &[&str],
    after: Duration,
    drop: (&str, &str, Option<serde_json::Value>),
) -> (std::process::Output, std::path::PathBuf) {
    let out = tmp_path("expiry", "json");
    let mut a = io_args(c, &c.url_with_auth(index), out.to_str().unwrap());
    a.extend(s(&[
        "--limit",
        "5",
        "--throttleInterval",
        "1500",
        "--bufferSize",
        "1",
        "--retryDelay",
        "1",
    ]));
    a.extend(s(extra));
    let run = tokio::spawn(async move { run_loud(a).await });
    tokio::time::sleep(after).await;
    let (st, body) = c.call(drop.0, drop.1, drop.2).await;
    assert!(st < 300, "dropping contexts: {st} {body}");
    (run.await.unwrap(), out)
}

// L-618 (TST-BRK-SRV-007): the scroll or point-in-time context disappears mid-dump (what a
// --scrollTime/--pitKeepAlive shorter than a slow page does). The contexts are dropped through
// the API (clear-scroll `_all`, delete-PIT `_all`) because engines reap expired contexts only
// every `search.keep_alive_interval` (1 min): the next page then gets the same 404
// search_context_missing an expiry gives. The run fails with exit 1, commits nothing, and says
// the context no longer exists, naming --scrollTime / --pitKeepAlive.
#[tokio::test]
async fn l618_lost_search_context_names_the_keep_alive_flag() {
    let c = Cluster::from_env();
    let index = unique("expiry");
    seed_index(&c, &index, 60).await;
    let (o, out) = dump_and_lose_context(
        &c,
        &index,
        &["--scrollTime", "1m"],
        Duration::from_millis(2500),
        ("DELETE", "_search/scroll/_all", None),
    )
    .await;
    let err = String::from_utf8_lossy(&o.stderr).into_owned();
    assert_eq!(o.status.code(), Some(1), "scroll: {err}");
    assert!(
        err.contains("the scroll context no longer exists") && err.contains("--scrollTime"),
        "scroll: {err}"
    );
    assert!(!out.exists(), "scroll: nothing committed");

    let opensearch = matches!(engine(), Engine::OpenSearch);
    if c.has_pit().await && opensearch {
        let (o, out) = dump_and_lose_context(
            &c,
            &index,
            &["--searchType", "pit", "--pitKeepAlive", "1m"],
            Duration::from_millis(2500),
            ("DELETE", "_search/point_in_time/_all", None),
        )
        .await;
        let err = String::from_utf8_lossy(&o.stderr).into_owned();
        assert_eq!(o.status.code(), Some(1), "pit: {err}");
        assert!(
            err.contains("the point-in-time context no longer exists")
                && err.contains("--pitKeepAlive"),
            "pit: {err}"
        );
        assert!(!out.exists(), "pit: nothing committed");
    } else {
        println!(
            "SKIP (PIT part): needs OpenSearch >= 2.4 (DELETE _search/point_in_time/_all); Elasticsearch has no delete-all for PITs"
        );
    }
    c.delete_index(&index).await;
}

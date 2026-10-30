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

// L-619 (TST-BRK-SRV-008): the index is deleted mid-dump, and the target index is deleted
// mid-restore (auto-create then re-creates it with dynamic mappings). Dump: exit 1, nothing
// committed, a message that names the deleted index as a cause. Restore (~13 s; the target's
// uuid is noted after 5 s and re-read at the end): auto-create re-creates the index, the uuid
// differs, and the tool exits 1 naming it as deleted and re-created.
#[tokio::test]
async fn l619_index_deleted_mid_dump_or_mid_restore_is_reported() {
    let c = Cluster::from_env();
    let src = unique("del_src");
    seed_index(&c, &src, 60).await;
    let (o, out) = dump_and_lose_context(
        &c,
        &src,
        &[],
        Duration::from_millis(2500),
        ("DELETE", &src, None),
    )
    .await;
    let err = String::from_utf8_lossy(&o.stderr).into_owned();
    assert_eq!(o.status.code(), Some(1), "dump: {err}");
    assert!(
        err.contains("index was deleted") || err.contains("index_not_found"),
        "dump: {err}"
    );
    assert!(!out.exists(), "dump: nothing committed");

    // restore: a slow restore into an existing index that is deleted part-way
    let src = unique("del_src2");
    let dst = unique("del_dst");
    seed_index(&c, &src, 60).await;
    let file = tmp_path("del", "json");
    assert_success(&run(io_args(&c, &c.url_with_auth(&src), file.to_str().unwrap())).await);
    c.copy_shape(&src, &dst).await;
    let mut a = io_args(&c, file.to_str().unwrap(), &c.url_with_auth(&dst));
    a.extend(s(&[
        "--limit",
        "5",
        "--throttleInterval",
        "1000",
        "--retryDelay",
        "1",
    ]));
    let run_h = tokio::spawn(async move { run_loud(a).await });
    tokio::time::sleep(Duration::from_millis(8000)).await;
    c.delete_index(&dst).await;
    let o = run_h.await.unwrap();
    let err = String::from_utf8_lossy(&o.stderr).into_owned();
    assert_eq!(o.status.code(), Some(1), "restore: {err}");
    assert!(
        err.contains(&format!(
            "the target index '{dst}' was deleted during the restore and re-created"
        )),
        "restore: {err}"
    );
    for i in [&src, &dst] {
        c.delete_index(i).await;
    }
}

fn stat(err: &str, key: &str) -> u64 {
    err.lines()
        .find_map(|l| l.strip_prefix(key))
        .and_then(|v| v.trim().parse().ok())
        .unwrap_or(0)
}

// L-621 (TST-BRK-SRV-011): two restores of the same file into the same index in parallel with
// `--bulkAction create`. Every document is created exactly once: the written counts add up to
// the file, the failed counts to the rest, each run with conflicts says why once (409, another
// writer) and does not exit 0.
#[tokio::test]
async fn l621_parallel_create_restores_count_conflicts_correctly() {
    let c = Cluster::from_env();
    let src = unique("cc_src");
    let dst = unique("cc_dst");
    seed_index(&c, &src, 200).await;
    let file = tmp_path("cc", "json");
    assert_success(&run(io_args(&c, &c.url_with_auth(&src), file.to_str().unwrap())).await);
    let n = lines(&file).len() as u64;
    c.copy_shape(&src, &dst).await;
    let mk = || {
        let mut a = io_args(&c, file.to_str().unwrap(), &c.url_with_auth(&dst));
        a.extend(s(&["--limit", "10", "--bulkAction", "create"]));
        a
    };
    let (a1, a2) = (mk(), mk());
    let (o1, o2) = tokio::join!(run_loud(a1), run_loud(a2));
    let mut written = 0;
    let mut failed = 0;
    for o in [&o1, &o2] {
        let err = String::from_utf8_lossy(&o.stderr).into_owned();
        let (w, f) = (stat(&err, "Total Writes:"), stat(&err, "Failed Writes:"));
        written += w;
        failed += f;
        if f > 0 {
            assert_ne!(o.status.code(), Some(0), "{err}");
            assert_eq!(err.matches("version conflict (409)").count(), 1, "{err}");
        } else {
            assert_success(o);
        }
    }
    assert_eq!(written, n, "each document created exactly once");
    assert_eq!(failed, n, "every other attempt is a counted conflict");
    c.ok("POST", &format!("{dst}/_refresh"), None).await;
    assert_eq!(c.count(&dst).await, n);
    for i in [&src, &dst] {
        c.delete_index(i).await;
    }
}

// L-622 (TST-BRK-SRV-012): a red index (its primary cannot be allocated: allocation filtering
// requires a node that does not exist). A dump fails clearly (exit 1) naming the index and
// that shards are unavailable, within the retry budget, and writes nothing.
#[tokio::test]
async fn l622_red_index_fails_clearly() {
    let c = Cluster::from_env();
    let index = unique("red");
    c.delete_index(&index).await;
    c.ok(
        "PUT",
        &index,
        Some(serde_json::json!({"settings": {
            "number_of_shards": 1, "number_of_replicas": 0,
            "index.routing.allocation.require._name": "no-such-node"
        }})),
    )
    .await;
    let (_, h) = c
        .call("GET", &format!("_cluster/health/{index}?timeout=2s"), None)
        .await;
    assert_eq!(h["status"], "red", "{h}");
    let out = tmp_path("red", "json");
    let mut a = io_args(&c, &c.url_with_auth(&index), out.to_str().unwrap());
    a.extend(s(&["--retryAttempts", "2", "--retryDelay", "100"]));
    let started = std::time::Instant::now();
    let o = run_loud(a).await;
    let err = String::from_utf8_lossy(&o.stderr).into_owned();
    assert_eq!(o.status.code(), Some(1), "{err}");
    assert!(err.contains("the index is red"), "{err}");
    assert!(started.elapsed() < Duration::from_secs(60), "bounded");
    assert!(!out.exists(), "nothing committed");
    c.delete_index(&index).await;
}

// L-638 (TST-BRK-RES-005), live half: --limit above index.max_result_window and --slices above
// index.max_slices_per_scroll are refused by the engine; the error names the setting and the
// flag to lower, exit 1, nothing committed.
#[tokio::test]
async fn l638_page_and_slice_limits_are_explained() {
    let c = Cluster::from_env();
    let index = unique("limits");
    seed_index(&c, &index, 20).await;
    for (flags, want) in [
        (vec!["--limit", "20000"], "lower --limit"),
        (vec!["--slices", "2000"], "lower --slices"),
    ] {
        let out = tmp_path("limits", "json");
        let mut a = io_args(&c, &c.url_with_auth(&index), out.to_str().unwrap());
        a.extend(s(&flags));
        let o = run_loud(a).await;
        let err = String::from_utf8_lossy(&o.stderr).into_owned();
        assert_eq!(o.status.code(), Some(1), "{flags:?}: {err}");
        assert!(err.contains(want), "{flags:?}: {err}");
        assert!(!out.exists(), "{flags:?}");
    }
    c.delete_index(&index).await;
}

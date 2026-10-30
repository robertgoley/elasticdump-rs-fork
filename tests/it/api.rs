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

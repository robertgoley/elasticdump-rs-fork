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

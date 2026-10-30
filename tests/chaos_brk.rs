//! Phase A6 chaos tests (TST-BRK, L-6xx) that need no cluster: an in-process router mock
//! plays the misbehaving server or proxy. Live-engine variants live in `tests/it`.

mod support;

use std::process::{Command, Output};

use support::router_mock::{Fault, Recorded, Reply, Route, RouterMock};

const INFO: &str = r#"{"version":{"distribution":"opensearch","number":"2.19.0"}}"#;
const CLEARED: &str = r#"{"succeeded":true,"num_freed":1}"#;

fn bin() -> Command {
    let mut c = Command::new(env!("CARGO_BIN_EXE_elasticdump-rs"));
    for v in [
        "ELASTICDUMP_INPUT_USERNAME",
        "ELASTICDUMP_INPUT_PASSWORD",
        "ELASTICDUMP_OUTPUT_USERNAME",
        "ELASTICDUMP_OUTPUT_PASSWORD",
        "NODE_TLS_REJECT_UNAUTHORIZED",
        "RUST_LOG",
        "SSL_CERT_FILE",
        "SSL_CERT_DIR",
    ] {
        c.env_remove(v);
    }
    c
}

async fn run_async(args: Vec<String>, envs: Vec<(String, String)>) -> Output {
    tokio::task::spawn_blocking(move || bin().args(args).envs(envs).output().expect("spawn"))
        .await
        .unwrap()
}

fn s(v: &[&str]) -> Vec<String> {
    v.iter().map(|x| x.to_string()).collect()
}

fn stderr(o: &Output) -> String {
    String::from_utf8_lossy(&o.stderr).into_owned()
}

fn hit(id: &str, src: &str) -> String {
    format!(r#"{{"_index":"idx","_id":"{id}","_score":1.0,"_source":{src}}}"#)
}

fn page(scroll_id: &str, hits: &[String], total: usize) -> String {
    format!(
        r#"{{"_scroll_id":"{scroll_id}","timed_out":false,"_shards":{{"total":1,"successful":1,"skipped":0,"failed":0}},"hits":{{"total":{{"value":{total},"relation":"eq"}},"hits":[{}]}}}}"#,
        hits.join(",")
    )
}

fn count(reqs: &[Recorded], method: &str, prefix: &str) -> usize {
    reqs.iter()
        .filter(|r| r.method == method && r.path.starts_with(prefix))
        .count()
}

fn dir_names(d: &std::path::Path) -> Vec<String> {
    std::fs::read_dir(d)
        .unwrap()
        .flatten()
        .map(|e| e.file_name().to_string_lossy().into_owned())
        .collect()
}

// L-601 (TST-BRK-NET-001): the connection is reset (RST) after N bytes of a scroll
// continuation's body. The continuation is not retried (the cursor may have advanced), the
// run exits 1, nothing is committed, the scroll is cleared, and the message says a larger
// --retryAttempts cannot help and how to recover. A rerun against a healthy server
// produces the full dump.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn l601_reset_mid_scroll_continuation_is_not_retried() {
    const N: usize = 4;
    let full: Vec<String> = (1..=N)
        .map(|i| hit(&i.to_string(), &format!("{{\"n\":{i}}}")))
        .collect();
    let mock = |cut: Option<usize>| {
        let full = full.clone();
        async move {
            let second = Reply::ok(page("s1", &full[1..2], N));
            let second = match cut {
                Some(n) => second.with_fault(Fault::ResetAt(n)),
                None => second,
            };
            RouterMock::builder()
                .route(Route::exact("GET", "/").reply(Reply::ok(INFO)))
                .route(
                    Route::contains("POST", "/idx/_search").reply(Reply::ok(page(
                        "s1",
                        &full[..1],
                        N,
                    ))),
                )
                .route(
                    Route::exact("POST", "/_search/scroll").replies(
                        [second]
                            .into_iter()
                            .chain((2..N).map(|i| Reply::ok(page("s1", &full[i..i + 1], N))))
                            .chain([Reply::ok(page("s1", &[], N))]),
                    ),
                )
                .fallback(Reply::ok(CLEARED))
                .start()
                .await
        }
    };
    for cut in [150usize, 260] {
        let m = mock(Some(cut)).await;
        let dir = tempfile::tempdir().unwrap();
        let out = dir.path().join("d.json");
        let o = run_async(
            s(&[
                "--input",
                &format!("{}/idx", m.url()),
                "--output",
                out.to_str().unwrap(),
                "--limit",
                "1",
                "--retryAttempts",
                "5",
                "--retryDelay",
                "1",
            ]),
            vec![],
        )
        .await;
        let err = stderr(&o);
        assert!(
            err.contains("reset by peer"),
            "cut {cut}: a mid-body RST: {err}"
        );
        let reqs = m.requests();
        assert_eq!(o.status.code(), Some(1), "cut {cut}: {err}");
        assert_eq!(
            count(&reqs, "POST", "/_search/scroll"),
            1,
            "cut {cut}: {err}"
        );
        assert!(
            err.contains("not retried") && err.contains("--retryAttempts cannot help"),
            "cut {cut}: {err}"
        );
        assert!(
            err.contains("--scrollId") && err.contains("Re-run"),
            "{err}"
        );
        assert_eq!(
            count(&reqs, "DELETE", "/_search/scroll"),
            1,
            "cut {cut}: scroll cleared: {err}"
        );
        assert!(!out.exists(), "cut {cut}: nothing committed");
        assert!(dir_names(dir.path()).is_empty(), "staging left");

        // the rerun the message recommends
        let m = mock(None).await;
        let o = run_async(
            s(&[
                "--input",
                &format!("{}/idx", m.url()),
                "--output",
                out.to_str().unwrap(),
                "--limit",
                "1",
            ]),
            vec![],
        )
        .await;
        assert!(o.status.success(), "{}", stderr(&o));
        let got = std::fs::read_to_string(&out).unwrap();
        let ids: Vec<String> = got
            .lines()
            .map(|l| serde_json::from_str::<serde_json::Value>(l).unwrap()["_id"].to_string())
            .collect();
        assert_eq!(ids, ["\"1\"", "\"2\"", "\"3\"", "\"4\""], "{got}");
    }
}

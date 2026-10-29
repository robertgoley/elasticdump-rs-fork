//! Injection regressions (TEST-REQUIREMENTS.md §8, TST-INJ-*) driven through
//! the binary against in-process mock clusters. No containers needed.

mod support;

use std::process::Output;

use support::mock_es::{CannedResponse, MockEs};

fn bin() -> std::process::Command {
    let mut c = std::process::Command::new(env!("CARGO_BIN_EXE_elasticdump-rs"));
    for k in [
        "ELASTICDUMP_INPUT_USERNAME",
        "ELASTICDUMP_INPUT_PASSWORD",
        "ELASTICDUMP_OUTPUT_USERNAME",
        "ELASTICDUMP_OUTPUT_PASSWORD",
        "RUST_LOG",
    ] {
        c.env_remove(k);
    }
    c
}

async fn run(args: Vec<String>) -> Output {
    tokio::task::spawn_blocking(move || bin().args(args).output().expect("spawn"))
        .await
        .unwrap()
}

fn stderr(o: &Output) -> String {
    String::from_utf8_lossy(&o.stderr).into_owned()
}

/// The INJ-F1 PoC hit as the cluster returns it: `_source` byte for byte as
/// indexed (pretty-printed, so it contains raw newlines between tokens).
/// `crlf` uses CRLF line breaks and a tab instead of LF.
fn poc_hit(crlf: bool) -> String {
    let nl = if crlf { "\r\n\t" } else { "\n" };
    format!(
        r#"{{"_index":"src","_type":"_doc","_id":"x","_score":1.0,"_source":{{"a":{nl}{{"_index":"victim","_id":"admin","_source":{{"role":"superuser"}}}}}}}}"#
    )
}

const POC_SOURCE: &str =
    r#"{"a":{"_index":"victim","_id":"admin","_source":{"role":"superuser"}}}"#;

fn page(hits: &[String]) -> String {
    format!(
        r#"{{"_scroll_id":"s1","timed_out":false,"_shards":{{"total":1,"successful":1,"skipped":0,"failed":0}},"hits":{{"total":{{"value":{n},"relation":"eq"}},"hits":[{h}]}}}}"#,
        n = hits.len(),
        h = hits.join(",")
    )
}

fn scroll(hits: &[String]) -> Vec<CannedResponse> {
    vec![
        CannedResponse::ok(page(hits)),
        CannedResponse::ok(page(&[])),
        CannedResponse::ok(r#"{"succeeded":true,"num_freed":1}"#),
    ]
}

const ES7: &str = r#"{"version":{"number":"7.17.29"}}"#;

fn bulk_ok(action: &str, n: usize) -> CannedResponse {
    let items: Vec<String> = (0..n)
        .map(|_| format!(r#"{{"{action}":{{"status":200}}}}"#))
        .collect();
    CannedResponse::ok(format!(
        r#"{{"took":1,"errors":false,"items":[{}]}}"#,
        items.join(",")
    ))
}

/// Body of the first `_bulk` request the output mock received.
fn bulk_body(mock: &MockEs) -> String {
    let reqs = mock.requests();
    let r = reqs
        .iter()
        .find(|r| r.contains("/_bulk"))
        .unwrap_or_else(|| panic!("no _bulk request: {reqs:#?}"));
    r.split_once("\r\n\r\n").unwrap().1.to_string()
}

fn ndjson(body: &str) -> Vec<serde_json::Value> {
    body.split('\n')
        .filter(|l| !l.is_empty())
        .map(|l| serde_json::from_str(l).unwrap_or_else(|e| panic!("bad NDJSON line {l:?}: {e}")))
        .collect()
}

async fn copy(extra: &[&str], input_index: &str, action: &str, crlf: bool) -> (Output, String) {
    let input = MockEs::serve(scroll(&[poc_hit(crlf)])).await;
    let output = MockEs::serve(vec![
        CannedResponse::ok(ES7),
        bulk_ok(action, 1),
        CannedResponse::ok(r#"{"_shards":{"total":1,"successful":1,"failed":0}}"#),
    ])
    .await;
    let mut args = vec![
        "--input".to_string(),
        input.input_url(input_index),
        "--output".to_string(),
        output.input_url("").trim_end_matches('/').to_string(),
        "--type".into(),
        "data".into(),
        "--quiet".into(),
        "--noImplicitRetry".into(),
    ];
    args.extend(extra.iter().map(|s| s.to_string()));
    let o = run(args).await;
    let body = bulk_body(&output);
    (o, body)
}

/// TST-INJ-002 (INJ-F1): host-only copy. The pretty-printed source must be
/// written back as ONE document to its own index; nothing reaches `victim`.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn inj_f1_host_only_copy_writes_one_doc_and_no_victim() {
    for crlf in [false, true] {
        let (o, body) = copy(&[], "", "index", crlf).await;
        let lines = ndjson(&body);
        assert_eq!(lines.len(), 2, "exactly action + source, got {body:?}");
        assert_eq!(lines[0]["index"]["_index"], "src", "{body}");
        assert_eq!(lines[0]["index"]["_id"], "x", "{body}");
        assert!(
            !body.contains(r#""_index":"victim","_id":"admin"}"#),
            "{body}"
        );
        assert_eq!(
            lines[1],
            serde_json::from_str::<serde_json::Value>(POC_SOURCE).unwrap(),
            "x keeps its source"
        );
        assert!(o.status.success(), "{}", stderr(&o));
    }
}

/// TST-INJ-002 with --output-index: the injected `_id` must not be written.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn inj_f1_output_index_does_not_overwrite_other_ids() {
    let (o, body) = copy(&["--output-index", "dst"], "src", "index", false).await;
    let lines = ndjson(&body);
    assert_eq!(lines.len(), 2, "{body:?}");
    assert_eq!(lines[0]["index"]["_id"], "x", "{body}");
    assert_eq!(lines[0]["index"]["_index"], "dst", "{body}");
    assert_eq!(
        lines[1],
        serde_json::from_str::<serde_json::Value>(POC_SOURCE).unwrap()
    );
    assert!(o.status.success(), "{}", stderr(&o));
}

/// TST-INJ-002 with --bulkAction delete: only `x` is deleted, never `admin`.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn inj_f1_bulk_delete_touches_only_the_hit() {
    let (o, body) = copy(
        &["--output-index", "dst", "--bulkAction", "delete"],
        "src",
        "delete",
        false,
    )
    .await;
    let lines = ndjson(&body);
    assert_eq!(lines.len(), 1, "{body:?}");
    assert_eq!(lines[0]["delete"]["_id"], "x", "{body}");
    assert!(o.status.success(), "{}", stderr(&o));
}

/// TST-INJ-001: one line per hit in file/stdout and --sourceOnly output.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn inj_f1_dump_is_one_line_per_hit() {
    for source_only in [false, true] {
        for crlf in [false, true] {
            let hits = vec![poc_hit(crlf), poc_hit(!crlf)];
            let input = MockEs::serve(scroll(&hits)).await;
            let mut args = vec![
                "--input".to_string(),
                input.input_url("src"),
                "--output".into(),
                "$".into(),
            ];
            if source_only {
                args.push("--sourceOnly".into());
            }
            let o = run(args).await;
            assert!(o.status.success(), "{}", stderr(&o));
            let out = String::from_utf8(o.stdout.clone()).unwrap();
            assert!(!out.contains('\r'), "{out:?}");
            let lines: Vec<&str> = out.lines().collect();
            assert_eq!(lines.len(), 2, "lines == hits: {out:?}");
            for l in lines {
                let v: serde_json::Value = serde_json::from_str(l).unwrap();
                let src = if source_only { &v } else { &v["_source"] };
                assert_eq!(
                    *src,
                    serde_json::from_str::<serde_json::Value>(POC_SOURCE).unwrap()
                );
            }
            assert!(stderr(&o).contains("Total Writes: 2"), "{}", stderr(&o));
        }
    }
}

/// TST-INJ-010: --fileSize parts never split a record.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn inj_f1_file_size_parts_hold_whole_records() {
    let hits: Vec<String> = (0..20).map(|i| poc_hit(i % 2 == 0)).collect();
    let input = MockEs::serve(scroll(&hits)).await;
    let dir = tempfile::tempdir().unwrap();
    let out = dir.path().join("dump.json");
    let o = run(vec![
        "--input".into(),
        input.input_url("src"),
        "--output".into(),
        out.to_str().unwrap().into(),
        "--fileSize".into(),
        "300b".into(),
        "--quiet".into(),
    ])
    .await;
    assert!(o.status.success(), "{}", stderr(&o));
    let mut records = 0;
    let mut parts = 0;
    for e in std::fs::read_dir(dir.path()).unwrap() {
        let text = std::fs::read_to_string(e.unwrap().path()).unwrap();
        parts += 1;
        for l in text.lines() {
            let v: serde_json::Value =
                serde_json::from_str(l).unwrap_or_else(|e| panic!("split record {l:?}: {e}"));
            assert_eq!(v["_id"], "x");
            records += 1;
        }
    }
    assert!(parts > 1, "expected several parts");
    assert_eq!(records, 20);
}

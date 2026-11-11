//! Injection regressions (TEST-REQUIREMENTS.md §8, TST-INJ-*) driven through
//! the binary against in-process mock clusters. No containers needed.

mod support;

use std::process::Output;

use support::mock_es::{CannedResponse, MockEs};

fn bin() -> std::process::Command {
    let mut c = std::process::Command::new(env!("CARGO_BIN_EXE_searchdump"));
    for k in [
        "SEARCHDUMP_INPUT_USERNAME",
        "SEARCHDUMP_INPUT_PASSWORD",
        "SEARCHDUMP_OUTPUT_USERNAME",
        "SEARCHDUMP_OUTPUT_PASSWORD",
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
    // a copy from a named index first asks the source for its engine (`GET /`, L-655)
    let mut canned = if input_index.is_empty() {
        vec![]
    } else {
        vec![CannedResponse::ok(ES7)]
    };
    canned.extend(scroll(&[poc_hit(crlf)]));
    let input = MockEs::serve(canned).await;
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

// ------------------------------------------------------------ INJ-F2

const BAD_NAMES: &[&str] = &[
    "*", "?", "a*", "a,b", ",", "_all", "", ".", "..", "-a", "_a", "+a", "Upper",
];

fn long_name() -> String {
    "a".repeat(256)
}

fn bad_names() -> Vec<String> {
    let mut v: Vec<String> = BAD_NAMES.iter().map(|s| s.to_string()).collect();
    v.push(long_name());
    v
}

/// Run a restore of `content` (one file) into a fresh mock and return the
/// output and every request line the mock saw.
async fn restore(content: &str, kind: &str, extra: &[&str]) -> (Output, Vec<String>) {
    let dir = tempfile::tempdir().unwrap();
    let f = dir.path().join("in.json");
    std::fs::write(&f, content).unwrap();
    // Plenty of OK answers: anything past the version probe is a violation.
    let mock = MockEs::serve(
        std::iter::once(CannedResponse::ok(ES7))
            .chain((0..8).map(|_| CannedResponse::ok(r#"{"acknowledged":true}"#)))
            .collect(),
    )
    .await;
    let mut args = vec![
        "--input".to_string(),
        f.to_str().unwrap().into(),
        "--output".into(),
        mock.input_url("").trim_end_matches('/').to_string(),
        "--type".into(),
        kind.into(),
        "--noImplicitRetry".into(),
    ];
    args.extend(extra.iter().map(|s| s.to_string()));
    let o = run(args).await;
    let lines = mock
        .requests()
        .iter()
        .map(|r| r.lines().next().unwrap_or("").to_string())
        .collect();
    (o, lines)
}

fn only_probe(reqs: &[String]) -> bool {
    reqs.iter().all(|r| r.starts_with("GET / "))
}

/// TST-INJ-006: the §8.1 analyzer PoC never sends `POST /*/_close`.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn inj_f2_analyzer_wildcard_never_closes_indices() {
    let (o, reqs) = restore(
        r#"{"*":{"settings":{"index":{"analysis":{"analyzer":{"evil":{"type":"standard"}}}}}}}"#,
        "analyzer",
        &["--closeIndex"],
    )
    .await;
    assert!(!o.status.success(), "{}", stderr(&o));
    assert!(only_probe(&reqs), "{reqs:#?}");
    assert!(reqs.iter().all(|r| !r.contains("_close")), "{reqs:#?}");
    assert!(stderr(&o).contains("invalid index name"), "{}", stderr(&o));
}

/// TST-INJ-006: `{"*":{"aliases":...}}` never adds an alias to every index.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn inj_f2_alias_wildcard_is_rejected() {
    let (o, reqs) = restore(r#"{"*":{"aliases":{"logs-read":{}}}}"#, "alias", &[]).await;
    assert!(!o.status.success(), "{}", stderr(&o));
    assert!(only_probe(&reqs), "{reqs:#?}");
}

/// TST-INJ-006: every bad name in every meta type and in data `_index`, with
/// and without a rename, is refused before any request is sent.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn inj_f2_bad_names_from_files_send_nothing() {
    for name in bad_names() {
        let key = serde_json::to_string(&name).unwrap();
        let cases: Vec<(String, &str)> = vec![
            (
                format!(r#"{{{key}:{{"settings":{{"index":{{"number_of_replicas":"0"}}}}}}}}"#),
                "settings",
            ),
            (
                format!(
                    r#"{{{key}:{{"mappings":{{"properties":{{"a":{{"type":"keyword"}}}}}}}}}}"#
                ),
                "mapping",
            ),
            (
                format!(
                    r#"{{{key}:{{"settings":{{"index":{{"analysis":{{}}}}}},"mappings":{{}}}}}}"#
                ),
                "index",
            ),
            (
                format!(
                    r#"{{{key}:{{"settings":{{"index":{{"analysis":{{"analyzer":{{"x":{{"type":"standard"}}}}}}}}}}}}}}"#
                ),
                "analyzer",
            ),
            (format!(r#"{{{key}:{{"aliases":{{"al":{{}}}}}}}}"#), "alias"),
            (
                format!(r#"{{"_index":{key},"_id":"1","_source":{{"a":1}}}}"#),
                "data",
            ),
        ];
        for (content, kind) in cases {
            for extra in [&[][..], &["--suffix=-x"][..]] {
                // With a suffix "." and ".." become ".-x"/"..-x", which are
                // valid names; every other bad name stays bad.
                if !extra.is_empty() && (name == "." || name == "..") {
                    continue;
                }
                let (o, reqs) = restore(&content, kind, extra).await;
                assert!(
                    !stderr(&o).contains("unexpected argument"),
                    "{}",
                    stderr(&o)
                );
                let bulk_or_write = reqs
                    .iter()
                    .any(|r| !r.starts_with("GET / ") && !r.starts_with("HEAD "));
                assert!(
                    !bulk_or_write,
                    "name {name:?} kind {kind} extra {extra:?}: {reqs:#?}\n{}",
                    stderr(&o)
                );
                assert!(
                    !o.status.success(),
                    "name {name:?} kind {kind} extra {extra:?} must fail\n{}",
                    stderr(&o)
                );
            }
        }
    }
}

// ------------------------------------------------------------ INJ-F3

/// TST-INJ-005: `_id`/`_routing`/`_parent` must be strings (numbers are
/// converted) and `version` an integer within u64; a bad document is rejected
/// on its own and never makes the whole bulk request fail.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn inj_f3_bad_action_fields_are_rejected_per_document() {
    let docs = [
        r#"{"_index":"i","_id":"good1","_source":{"n":1}}"#,
        r#"{"_index":"i","_id":{"$x":1},"_routing":[1,2],"_version":"9e99","_source":{}}"#,
        r#"{"_index":"i","_id":5,"_routing":7,"_version":3,"_source":{"n":2}}"#,
        r#"{"_index":"i","_id":"v","_version":"9e99","_source":{}}"#,
        r#"{"_index":"i","_id":"v2","_version":18446744073709551616,"_source":{}}"#,
        r#"{"_index":"i","_id":"r","_routing":{"a":1},"_source":{}}"#,
        r#"{"_index":"i","_id":true,"_source":{}}"#,
        r#"{"_index":"i","_id":"good2","_source":{"n":3}}"#,
    ];
    let dir = tempfile::tempdir().unwrap();
    let f = dir.path().join("in.json");
    std::fs::write(&f, docs.join("\n") + "\n").unwrap();
    let mock = MockEs::serve(vec![
        CannedResponse::ok(ES7),
        bulk_ok("index", 3),
        CannedResponse::ok(r#"{"_shards":{"total":1,"successful":1,"failed":0}}"#),
    ])
    .await;
    let o = run(vec![
        "--input".into(),
        f.to_str().unwrap().into(),
        "--output".into(),
        mock.input_url("").trim_end_matches('/').to_string(),
        "--handleVersion".into(),
        "--noImplicitRetry".into(),
    ])
    .await;
    let body = bulk_body(&mock);
    let lines = ndjson(&body);
    assert_eq!(lines.len(), 6, "only the 3 valid docs are sent: {body}");
    assert_eq!(lines[0]["index"]["_id"], "good1");
    assert_eq!(lines[2]["index"]["_id"], "5", "numbers become strings");
    assert_eq!(lines[2]["index"]["routing"], "7");
    assert_eq!(lines[2]["index"]["version"], 3);
    assert_eq!(lines[4]["index"]["_id"], "good2");
    // rejections are reported and make the run fail
    assert!(!o.status.success(), "{}", stderr(&o));
    let err = stderr(&o);
    assert!(err.contains("5 document line(s) were rejected"), "{err}");
    assert!(err.contains("_id must be a string or a number"), "{err}");
    assert!(err.contains("version must be an integer"), "{err}");
}

/// Without --ignore-es-write-errors the first bad document stops the run
/// before any bulk request is sent.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn inj_f3_strict_mode_sends_nothing() {
    let dir = tempfile::tempdir().unwrap();
    let f = dir.path().join("in.json");
    std::fs::write(
        &f,
        "{\"_index\":\"i\",\"_id\":\"ok\",\"_source\":{}}\n{\"_index\":\"i\",\"_id\":[1],\"_source\":{}}\n",
    )
    .unwrap();
    let mock = MockEs::serve(vec![CannedResponse::ok(ES7), bulk_ok("index", 1)]).await;
    let o = run(vec![
        "--input".into(),
        f.to_str().unwrap().into(),
        "--output".into(),
        mock.input_url("").trim_end_matches('/').to_string(),
        "--ignore-es-write-errors=false".into(),
        "--noImplicitRetry".into(),
    ])
    .await;
    assert!(!o.status.success());
    assert!(
        mock.requests().iter().all(|r| !r.contains("/_bulk")),
        "{:#?}",
        mock.requests()
    );
}

// ------------------------------------------------------------ INJ-F4

/// `--parseExtraFields` names are restricted to `^[a-z][a-z0-9_]*$`.
#[test]
fn inj_f4_extra_field_names_are_validated() {
    for bad in [
        r#"x":1,"_index":"other"#,
        "Pipeline",
        "a-b",
        "a.b",
        "9x",
        "a b",
        "\u{e9}",
    ] {
        let o = bin()
            .args([
                "--input",
                "/dev/null",
                "--output",
                "http://127.0.0.1:1/i",
                "--parseExtraFields",
                bad,
            ])
            .output()
            .unwrap();
        assert_eq!(o.status.code(), Some(2), "{bad:?}: {}", stderr(&o));
        assert!(stderr(&o).contains("--parseExtraFields"), "{}", stderr(&o));
    }
}

/// Valid names are de-duplicated against the defaults and each other.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn inj_f4_extra_fields_are_deduplicated() {
    let dir = tempfile::tempdir().unwrap();
    let f = dir.path().join("in.json");
    std::fs::write(
        &f,
        "{\"_index\":\"i\",\"_id\":\"1\",\"_routing\":\"r\",\"_pipeline\":\"p\",\"_source\":{}}\n",
    )
    .unwrap();
    let mock = MockEs::serve(vec![
        CannedResponse::ok(ES7),
        bulk_ok("index", 1),
        CannedResponse::ok(r#"{"_shards":{"total":1,"successful":1,"failed":0}}"#),
    ])
    .await;
    let o = run(vec![
        "--input".into(),
        f.to_str().unwrap().into(),
        "--output".into(),
        mock.input_url("").trim_end_matches('/').to_string(),
        "--parseExtraFields".into(),
        "routing,_pipeline,pipeline".into(),
        "--noImplicitRetry".into(),
    ])
    .await;
    assert!(o.status.success(), "{}", stderr(&o));
    let body = bulk_body(&mock);
    let action = body.lines().next().unwrap();
    assert_eq!(
        action,
        r#"{"index":{"_index":"i","_id":"1","routing":"r","pipeline":"p"}}"#
    );
}

// ------------------------------------------------------------ L-105 guards

/// TST-INJ-008 / TST-INJ-009: duplicate top-level keys and a non-object
/// `_source` are rejected per document; the valid one is still written.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn guards_duplicate_keys_and_non_object_source() {
    let docs = [
        r#"{"_index":"i","_id":"a","_index":"victim","_source":{}}"#,
        r#"{"_index":"i","_id":"b","_id":"admin","_source":{}}"#,
        r#"{"_index":"i","_id":"c","_source":{"x":1},"_source":{"role":"superuser"}}"#,
        r#"{"_index":"i","_id":"d","_routing":"r","_routing":"s","_source":{}}"#,
        r#"{"_index":"i","_id":"e","_source":[1,2]}"#,
        r#"{"_index":"i","_id":"f","_source":"x"}"#,
        r#"{"_index":"i","_id":"ok","_source":{"n":1}}"#,
    ];
    let dir = tempfile::tempdir().unwrap();
    let f = dir.path().join("in.json");
    std::fs::write(&f, docs.join("\n") + "\n").unwrap();
    let mock = MockEs::serve(vec![
        CannedResponse::ok(ES7),
        bulk_ok("index", 1),
        CannedResponse::ok(r#"{"_shards":{"total":1,"successful":1,"failed":0}}"#),
    ])
    .await;
    let o = run(vec![
        "--input".into(),
        f.to_str().unwrap().into(),
        "--output".into(),
        mock.input_url("").trim_end_matches('/').to_string(),
        "--noImplicitRetry".into(),
    ])
    .await;
    let lines = ndjson(&bulk_body(&mock));
    assert_eq!(lines.len(), 2, "{lines:?}");
    assert_eq!(lines[0]["index"]["_id"], "ok");
    assert!(!o.status.success());
    let err = stderr(&o);
    assert!(err.contains("6 document line(s) were rejected"), "{err}");
    assert!(err.contains("duplicate key"), "{err}");
    assert!(err.contains("_source must be a JSON object"), "{err}");
}

/// TST-INJ-011: CR/LF/NUL or invisible bytes in --headers are a usage error,
/// nothing is sent, and the value (maybe a secret) is not echoed.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn guards_header_injection_is_a_usage_error() {
    let mock = MockEs::serve(scroll(&[])).await;
    for (flag, h) in [
        ("--headers", r#"{"X-A":"s3cret\r\nX-Evil: 1"}"#),
        ("--headers", r#"{"X-A":"s3cret\n"}"#),
        ("--headers", r#"{"X-A":"s3cret\u0000"}"#),
        ("--input-headers", r#"{"X-A":"s3cret\u0007"}"#),
        ("--output-headers", r#"{"X-A\r\nX-Evil":"s3cret"}"#),
        ("--headers", r#"{"X A":"s3cret"}"#),
        ("--headers", r#"{"X-A":"s3cr\u00e9t"}"#),
    ] {
        let o = run(vec![
            "--input".into(),
            mock.input_url("idx"),
            "--output".into(),
            "$".into(),
            flag.into(),
            h.into(),
        ])
        .await;
        assert_eq!(o.status.code(), Some(2), "{h}: {}", stderr(&o));
        assert!(
            !stderr(&o).contains("s3cr"),
            "secret echoed: {}",
            stderr(&o)
        );
    }
    assert_eq!(mock.request_count(), 0, "{:#?}", mock.requests());
}

/// TST-INJ-012 (Grok Bot, 2026-11-02): CR/LF in --params / --input-params keys or values
/// is percent-encoded into the query string as data (the mock keeps the raw encoded query); no extra header or request line.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn guards_params_crlf_is_encoded_not_injected() {
    for (flag, p, k, v) in [
        (
            "--params",
            r#"{"a":"b\r\nX-Evil: 1"}"#,
            "a",
            "b%0D%0AX-Evil%3A+1",
        ),
        (
            "--input-params",
            r#"{"a\r\nX-Evil: 2":"b"}"#,
            "a%0D%0AX-Evil%3A+2",
            "b",
        ),
    ] {
        let mock = MockEs::serve(scroll(&[])).await;
        let _ = run(vec![
            "--input".into(),
            mock.input_url("idx"),
            "--output".into(),
            "$".into(),
            flag.into(),
            p.into(),
        ])
        .await;
        let reqs = mock.recorded();
        assert!(!reqs.is_empty(), "{p}: nothing sent");
        for r in &reqs {
            assert!(
                !r.headers
                    .iter()
                    .any(|(h, _)| h.eq_ignore_ascii_case("x-evil")),
                "{p}: injected header: {:?}",
                r.headers
            );
        }
        let search = reqs
            .iter()
            .find(|r| r.path.ends_with("/_search"))
            .expect("search");
        assert!(
            search.query.iter().any(|(qk, qv)| qk == k && qv == v),
            "{p}: payload not carried as query data: {:?}",
            search.query
        );
    }
}

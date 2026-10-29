//! Secret-handling regressions (TEST-REQUIREMENTS.md §7, F-05..F-08).

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
        "HTTP_PROXY",
        "http_proxy",
        "HTTPS_PROXY",
        "https_proxy",
        "ALL_PROXY",
        "all_proxy",
        "NO_PROXY",
        "no_proxy",
    ] {
        c.env_remove(k);
    }
    c
}

async fn run(args: Vec<String>, env: Vec<(&'static str, String)>) -> Output {
    tokio::task::spawn_blocking(move || bin().args(args).envs(env).output().expect("spawn"))
        .await
        .unwrap()
}

fn stderr(o: &Output) -> String {
    String::from_utf8_lossy(&o.stderr).into_owned()
}

fn scroll() -> Vec<CannedResponse> {
    let page = |hits: &str| {
        format!(
            r#"{{"_scroll_id":"s1","timed_out":false,"_shards":{{"total":1,"successful":1,"skipped":0,"failed":0}},"hits":{{"total":{{"value":1,"relation":"eq"}},"hits":[{hits}]}}}}"#
        )
    };
    vec![
        CannedResponse::ok(page(r#"{"_index":"i","_id":"1","_source":{}}"#)),
        CannedResponse::ok(page("")),
        CannedResponse::ok(r#"{"succeeded":true,"num_freed":1}"#),
    ]
}

/// F-05: --debug logs requests without --params values or passwords.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn debug_log_masks_params_and_credentials() {
    let mock = MockEs::serve(scroll()).await;
    let url = mock
        .input_url("i")
        .replace("http://", "http://user:PW-SECRET@");
    let o = run(
        vec![
            "--input".into(),
            url,
            "--output".into(),
            "$".into(),
            "--debug".into(),
            "--input-params".into(),
            r#"{"api_key":"Q-SECRET"}"#.into(),
        ],
        vec![],
    )
    .await;
    assert!(o.status.success(), "{}", stderr(&o));
    let err = stderr(&o);
    assert!(err.contains("api_key=***"), "{err}");
    assert!(!err.contains("Q-SECRET"), "{err}");
    assert!(!err.contains("PW-SECRET"), "{err}");
    // the server still got the real value
    assert!(mock.requests()[0].contains("api_key=Q-SECRET"));
}

/// F-06: a scheme-less `user:pw@host` endpoint is a file path; it is masked
/// in every message and a warning says it is treated as a file.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn schemeless_url_is_masked_and_warned() {
    let o = run(
        vec![
            "--input".into(),
            "elastic:PW-SECRET@localhost:9200/idx".into(),
            "--output".into(),
            "$".into(),
        ],
        vec![],
    )
    .await;
    let err = stderr(&o);
    assert!(!err.contains("PW-SECRET"), "{err}");
    assert!(err.contains("treated as a FILE path"), "{err}");
}

/// F-07: an environment proxy is announced; --noProxy connects directly.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn env_proxy_is_visible_and_can_be_disabled() {
    let mock = MockEs::serve(scroll()).await;
    let proxy = vec![(
        "HTTP_PROXY",
        "http://pu:PROXY-SECRET@127.0.0.1:9".to_string(),
    )];
    let args = |extra: Option<&str>| {
        let mut v = vec![
            "--input".to_string(),
            mock.input_url("i"),
            "--output".into(),
            "$".into(),
            "--noImplicitRetry".into(),
        ];
        v.extend(extra.map(String::from));
        v
    };
    let o = run(args(None), proxy.clone()).await;
    let err = stderr(&o);
    assert!(err.contains("go through proxy"), "{err}");
    assert!(err.contains("HTTP_PROXY"), "{err}");
    assert!(!err.contains("PROXY-SECRET"), "{err}");
    let o = run(args(Some("--noProxy")), proxy).await;
    assert!(o.status.success(), "{}", stderr(&o));
    assert!(!stderr(&o).contains("go through proxy"), "{}", stderr(&o));
}

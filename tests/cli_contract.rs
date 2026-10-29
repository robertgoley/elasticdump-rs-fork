//! CLI contract tests that need no cluster: flag handling, exit codes, file
//! transfers and request shaping against an in-process mock server.

mod support;

use std::process::{Command, Output, Stdio};

use support::mock_es::{CannedResponse, MockEs};

fn bin() -> Command {
    let mut c = Command::new(env!("CARGO_BIN_EXE_elasticdump-rs"));
    c.env_remove("ELASTICDUMP_INPUT_USERNAME")
        .env_remove("ELASTICDUMP_INPUT_PASSWORD")
        .env_remove("ELASTICDUMP_OUTPUT_USERNAME")
        .env_remove("ELASTICDUMP_OUTPUT_PASSWORD")
        .env_remove("NODE_TLS_REJECT_UNAUTHORIZED")
        .env_remove("RUST_LOG");
    c
}

fn run(args: &[&str]) -> Output {
    bin().args(args).output().expect("spawn")
}

async fn run_async(args: Vec<String>, envs: Vec<(String, String)>) -> Output {
    tokio::task::spawn_blocking(move || bin().args(args).envs(envs).output().expect("spawn"))
        .await
        .unwrap()
}

fn stderr(o: &Output) -> String {
    String::from_utf8_lossy(&o.stderr).into_owned()
}

fn stdout(o: &Output) -> String {
    String::from_utf8_lossy(&o.stdout).into_owned()
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

/// initial page with `hits`, then an empty continuation, then clear-scroll.
fn scroll_script(hits: &[String]) -> Vec<CannedResponse> {
    vec![
        CannedResponse::ok(page("s1", hits, hits.len())),
        CannedResponse::ok(page("s1", &[], hits.len())),
        CannedResponse::ok(r#"{"succeeded":true,"num_freed":1}"#),
    ]
}

#[test]
fn out_of_scope_type_is_a_usage_error() {
    let o = run(&["--input", "in.json", "--output", "$", "--type", "analyzer"]);
    assert_eq!(o.status.code(), Some(2), "{}", stderr(&o));
    assert!(stderr(&o).contains("not supported"), "{}", stderr(&o));
}

#[test]
fn out_of_scope_data_changing_flags_are_errors() {
    for flag in [
        "--transform=doc._source.x=1",
        "--awsChain",
        "--searchBodyTemplate=x.js",
        "--delete",
    ] {
        let o = run(&["--input", "in.json", "--output", "$", flag]);
        assert_eq!(o.status.code(), Some(2), "{flag}: {}", stderr(&o));
        assert!(
            stderr(&o).contains("not supported"),
            "{flag}: {}",
            stderr(&o)
        );
    }
}

#[test]
fn out_of_scope_tuning_flags_warn_and_are_ignored() {
    let dir = tempfile::tempdir().unwrap();
    let input = dir.path().join("in.json");
    std::fs::write(&input, "{\"a\":1}\n").unwrap();
    let o = run(&[
        "--input",
        input.to_str().unwrap(),
        "--output",
        "$",
        "--s3Region",
        "us-east-1",
        "--csvDelimiter=;",
    ]);
    assert!(o.status.success(), "{}", stderr(&o));
    assert_eq!(stdout(&o), "{\"a\":1}\n");
    assert!(stderr(&o).contains("Ignoring --s3Region"), "{}", stderr(&o));
    assert!(
        stderr(&o).contains("Ignoring --csvDelimiter"),
        "{}",
        stderr(&o)
    );
}

#[test]
fn split_flags_need_a_file_output() {
    let o = run(&["--input", "in.json", "--output", "$", "--fileSize", "1mb"]);
    assert_eq!(o.status.code(), Some(2), "{}", stderr(&o));
    let o = run(&[
        "--input",
        "in.json",
        "--output",
        "out.json",
        "--fileSize",
        "1mb",
        "--maxRows",
        "5",
    ]);
    assert_eq!(o.status.code(), Some(2), "{}", stderr(&o));
}

#[test]
fn existing_output_without_overwrite_fails() {
    let dir = tempfile::tempdir().unwrap();
    let input = dir.path().join("in.json");
    let out = dir.path().join("out.json");
    std::fs::write(&input, "{\"a\":1}\n").unwrap();
    std::fs::write(&out, "keep\n").unwrap();
    let o = run(&[
        "--input",
        input.to_str().unwrap(),
        "--output",
        out.to_str().unwrap(),
    ]);
    assert_eq!(o.status.code(), Some(1));
    assert!(stderr(&o).contains("already exists"), "{}", stderr(&o));
    assert_eq!(std::fs::read_to_string(&out).unwrap(), "keep\n");
    let o = run(&[
        "--input",
        input.to_str().unwrap(),
        "--output",
        out.to_str().unwrap(),
        "--overwrite",
    ]);
    assert!(o.status.success(), "{}", stderr(&o));
    assert_eq!(std::fs::read_to_string(&out).unwrap(), "{\"a\":1}\n");
}

#[test]
fn file_to_file_normalises_concatenated_and_pretty_json_with_offset_and_size() {
    let dir = tempfile::tempdir().unwrap();
    let input = dir.path().join("in.json");
    std::fs::write(
        &input,
        "{\"_id\":\"1\",\"_source\":{\"n\":1}}{\"_id\":\"2\",\"_source\":{\"n\":2}}\n{\n  \"_id\": \"3\",\n  \"_source\": {\"n\": 123456789012345678901234567890}\n}\n{\"_id\":\"4\",\"_source\":{}}\n",
    )
    .unwrap();
    let out = dir.path().join("out.json");
    let o = run(&[
        "--input",
        input.to_str().unwrap(),
        "--output",
        out.to_str().unwrap(),
        "--limit",
        "1",
        "--offset",
        "1",
        "--size",
        "2",
    ]);
    assert!(o.status.success(), "{}", stderr(&o));
    assert_eq!(
        std::fs::read_to_string(&out).unwrap(),
        "{\"_id\":\"2\",\"_source\":{\"n\":2}}\n{\"_id\":\"3\",\"_source\":{\"n\":123456789012345678901234567890}}\n"
    );
    assert!(stderr(&o).contains("Total Writes: 2"), "{}", stderr(&o));
}

#[test]
fn stdin_to_stdout() {
    let mut child = bin()
        .args(["--input", "$", "--output", "$", "--quiet"])
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .spawn()
        .unwrap();
    {
        use std::io::Write;
        let mut stdin = child.stdin.take().unwrap();
        stdin.write_all(b"{\"a\":1}\n{\"b\":2}\n").unwrap();
    }
    let o = child.wait_with_output().unwrap();
    assert!(o.status.success());
    assert_eq!(stdout(&o), "{\"a\":1}\n{\"b\":2}\n");
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn headers_and_params_reach_every_request() {
    let mock = MockEs::serve(scroll_script(&[hit("1", "{\"a\":1}")])).await;
    let o = run_async(
        vec![
            "--input".into(),
            mock.input_url("idx"),
            "--output".into(),
            "$".into(),
            "--quiet".into(),
            "--headers".into(),
            r#"{"X-Global":"g","X-Over":"global"}"#.into(),
            "--input-headers".into(),
            r#"{"X-Over":"input"}"#.into(),
            "--input-params".into(),
            r#"{"preference":"_local"}"#.into(),
        ],
        vec![],
    )
    .await;
    assert!(o.status.success(), "{}", stderr(&o));
    let reqs = mock.requests();
    assert_eq!(reqs.len(), 3, "{reqs:#?}");
    for r in &reqs {
        let lower = r.to_ascii_lowercase();
        assert!(lower.contains("\r\nx-global: g\r\n"), "{r}");
        assert!(lower.contains("\r\nx-over: input\r\n"), "{r}");
        assert!(lower.contains("\r\nuser-agent: elasticdump\r\n"), "{r}");
    }
    assert!(reqs[0].starts_with("POST /idx/_search?"), "{}", reqs[0]);
    assert!(reqs[0].contains("scroll=10m"), "{}", reqs[0]);
    assert!(reqs[0].contains("preference=_local"), "{}", reqs[0]);
    assert!(reqs[1].starts_with("POST /_search/scroll "), "{}", reqs[1]);
    assert!(
        reqs[2].starts_with("DELETE /_search/scroll "),
        "{}",
        reqs[2]
    );
    assert_eq!(stdout(&o), format!("{}\n", hit("1", "{\"a\":1}")));
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn source_only_writes_raw_sources() {
    let mock = MockEs::serve(scroll_script(&[
        hit("1", "{\"n\":12345678901234567890}"),
        hit("2", "{\"s\":\"\\u00e9\"}"),
    ]))
    .await;
    let o = run_async(
        vec![
            "--input".into(),
            mock.input_url("idx"),
            "--output".into(),
            "$".into(),
            "--quiet".into(),
            "--sourceOnly".into(),
        ],
        vec![],
    )
    .await;
    assert!(o.status.success(), "{}", stderr(&o));
    assert_eq!(
        stdout(&o),
        "{\"n\":12345678901234567890}\n{\"s\":\"\\u00e9\"}\n"
    );
}

fn basic(user: &str, pass: &str) -> String {
    use base64::prelude::*;
    format!(
        "authorization: basic {}",
        BASE64_STANDARD.encode(format!("{user}:{pass}"))
    )
    .to_ascii_lowercase()
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn credentials_from_auth_file_env_and_url() {
    let dir = tempfile::tempdir().unwrap();
    let auth = dir.path().join("auth.ini");
    std::fs::write(&auth, "user=fileuser\npassword=filepass\n").unwrap();

    // auth file
    let mock = MockEs::serve(scroll_script(&[])).await;
    let o = run_async(
        vec![
            "--input".into(),
            mock.input_url("idx"),
            "--output".into(),
            "$".into(),
            "--quiet".into(),
            "--httpAuthFile".into(),
            auth.to_str().unwrap().into(),
        ],
        vec![
            ("ELASTICDUMP_INPUT_USERNAME".into(), "envuser".into()),
            ("ELASTICDUMP_INPUT_PASSWORD".into(), "envpass".into()),
        ],
    )
    .await;
    assert!(o.status.success(), "{}", stderr(&o));
    assert!(
        mock.requests()[0]
            .to_ascii_lowercase()
            .contains(&basic("fileuser", "filepass"))
    );

    // env vars
    let mock = MockEs::serve(scroll_script(&[])).await;
    let o = run_async(
        vec![
            "--input".into(),
            mock.input_url("idx"),
            "--output".into(),
            "$".into(),
            "--quiet".into(),
        ],
        vec![
            ("ELASTICDUMP_INPUT_USERNAME".into(), "envuser".into()),
            ("ELASTICDUMP_INPUT_PASSWORD".into(), "envpass".into()),
        ],
    )
    .await;
    assert!(o.status.success(), "{}", stderr(&o));
    assert!(
        mock.requests()[0]
            .to_ascii_lowercase()
            .contains(&basic("envuser", "envpass"))
    );

    // URL credentials win over env
    let mock = MockEs::serve(scroll_script(&[])).await;
    let url = mock
        .input_url("idx")
        .replace("http://", "http://urluser:url%40pass@");
    let o = run_async(
        vec![
            "--input".into(),
            url,
            "--output".into(),
            "$".into(),
            "--quiet".into(),
        ],
        vec![("ELASTICDUMP_INPUT_USERNAME".into(), "envuser".into())],
    )
    .await;
    assert!(o.status.success(), "{}", stderr(&o));
    assert!(
        mock.requests()[0]
            .to_ascii_lowercase()
            .contains(&basic("urluser", "url@pass"))
    );
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn malformed_auth_file_is_an_error() {
    let dir = tempfile::tempdir().unwrap();
    let auth = dir.path().join("auth.ini");
    std::fs::write(&auth, "user=only\n").unwrap();
    let o = run_async(
        vec![
            "--input".into(),
            "http://127.0.0.1:9/idx".into(),
            "--output".into(),
            "$".into(),
            "--httpAuthFile".into(),
            auth.to_str().unwrap().into(),
        ],
        vec![],
    )
    .await;
    assert_eq!(o.status.code(), Some(1));
    assert!(stderr(&o).contains("Malformed Auth File"), "{}", stderr(&o));
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn size_stops_early_and_clears_scroll() {
    let hits: Vec<String> = (1..=5).map(|i| hit(&i.to_string(), "{}")).collect();
    // The reader may or may not issue one continuation before it observes
    // the --size cancellation; an empty page keeps both orders valid.
    let mock = MockEs::serve(vec![
        CannedResponse::ok(page("s1", &hits, 50)),
        CannedResponse::ok(page("s1", &[], 50)),
        CannedResponse::ok(r#"{"succeeded":true,"num_freed":1}"#),
    ])
    .await;
    let o = run_async(
        vec![
            "--input".into(),
            mock.input_url("idx"),
            "--output".into(),
            "$".into(),
            "--offset".into(),
            "1".into(),
            "--size".into(),
            "3".into(),
            "--limit".into(),
            "5".into(),
        ],
        vec![],
    )
    .await;
    assert!(o.status.success(), "{}", stderr(&o));
    let ids: Vec<String> = stdout(&o)
        .lines()
        .map(|l| {
            serde_json::from_str::<serde_json::Value>(l).unwrap()["_id"]
                .as_str()
                .unwrap()
                .to_string()
        })
        .collect();
    assert_eq!(ids, vec!["2", "3", "4"]);
    assert!(stderr(&o).contains("Total Writes: 3"), "{}", stderr(&o));
    let reqs = mock.requests();
    assert!(
        reqs.iter().any(|r| r.starts_with("DELETE /_search/scroll")),
        "{reqs:#?}"
    );
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn bulk_requests_are_shaped_like_elasticdump() {
    let dir = tempfile::tempdir().unwrap();
    let input = dir.path().join("in.json");
    std::fs::write(
        &input,
        format!("{}\n{}\n", hit("1", "{\"a\":1}"), hit("2", "{\"a\":2}")),
    )
    .unwrap();
    let mock = MockEs::serve(vec![
        CannedResponse::ok(r#"{"version":{"distribution":"opensearch","number":"2.19.0"}}"#),
        CannedResponse::ok(r#"{"took":1,"errors":false,"items":[{"index":{"status":201}},{"index":{"status":201}}]}"#),
        CannedResponse::ok(r#"{"_shards":{"total":1,"successful":1,"failed":0}}"#),
    ])
    .await;
    let o = run_async(
        vec![
            "--input".into(),
            input.to_str().unwrap().into(),
            "--output".into(),
            mock.input_url("dst"),
            "--esCompress".into(),
            "--output-params".into(),
            r#"{"pipeline":"p1"}"#.into(),
        ],
        vec![],
    )
    .await;
    assert!(o.status.success(), "{}", stderr(&o));
    let reqs = mock.requests();
    assert_eq!(reqs.len(), 3, "{reqs:#?}");
    assert!(reqs[0].starts_with("GET / "), "{}", reqs[0]);
    assert!(
        reqs[1].starts_with("POST /dst/_bulk?pipeline=p1 "),
        "{}",
        reqs[1]
    );
    let bulk = reqs[1].to_ascii_lowercase();
    assert!(
        bulk.contains("content-type: application/x-ndjson"),
        "{bulk}"
    );
    assert!(bulk.contains("content-encoding: gzip"), "{bulk}");
    assert!(reqs[2].starts_with("POST /dst/_refresh "), "{}", reqs[2]);
    assert!(stderr(&o).contains("Total Writes: 2"), "{}", stderr(&o));
}

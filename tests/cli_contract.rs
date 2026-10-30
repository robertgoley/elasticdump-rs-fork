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
fn unknown_type_and_file_to_file_metadata_are_usage_errors() {
    let o = run(&["--input", "in.json", "--output", "$", "--type", "bogus"]);
    assert_eq!(o.status.code(), Some(2), "{}", stderr(&o));
    assert!(stderr(&o).contains("index_template"), "{}", stderr(&o));
    let o = run(&[
        "--input", "in.json", "--output", "out.json", "--type", "alias",
    ]);
    assert_eq!(o.status.code(), Some(2), "{}", stderr(&o));
    assert!(stderr(&o).contains("at least one side"), "{}", stderr(&o));
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

// ------------------------------------------------ metadata types (M7-M12)

const ES8: &str = r#"{"version":{"number":"8.6.1"}}"#;
const OS2: &str = r#"{"version":{"distribution":"opensearch","number":"2.19.0"}}"#;

fn write_tmp(name: &str, content: &str) -> std::path::PathBuf {
    let p = std::env::temp_dir().join(format!("edrs-contract-{}-{name}", std::process::id()));
    std::fs::write(&p, content).unwrap();
    p
}

fn host(mock: &MockEs) -> String {
    mock.input_url("").trim_end_matches('/').to_string()
}

async fn meta_run(args: &[&str]) -> Output {
    let mut v: Vec<String> = args.iter().map(|s| s.to_string()).collect();
    v.extend(["--noImplicitRetry".to_string()]);
    run_async(v, vec![]).await
}

#[tokio::test]
async fn alias_restore_is_one_atomic_post_with_prefixed_targets() {
    let mock = MockEs::serve(vec![
        CannedResponse::ok(ES8),
        CannedResponse::ok(r#"{"acknowledged":true}"#),
    ])
    .await;
    // elasticdump's own fixture (test/test-resources/alias.json) plus an index without aliases
    let f = write_tmp(
        "alias.json",
        r#""{\"source_index\":{\"aliases\":{\"the-source\":{},\"test-2018\":{\"is_write_index\":true}}},\"bare\":{\"aliases\":{}}}""#,
    );
    let o = meta_run(&[
        "--input",
        f.to_str().unwrap(),
        "--output",
        &host(&mock),
        "--type",
        "alias",
        "--prefix",
        "p_",
    ])
    .await;
    assert!(o.status.success(), "{}", stderr(&o));
    let reqs = mock.requests();
    assert_eq!(reqs.len(), 2);
    assert!(reqs[1].starts_with("POST /_aliases "), "{}", reqs[1]);
    assert!(
        reqs[1].contains(r#"{"actions":[{"add":{"index":"p_source_index","alias":"the-source"}},{"add":{"index":"p_source_index","alias":"test-2018","is_write_index":true}}]}"#),
        "{}",
        reqs[1]
    );
}

#[tokio::test]
async fn ism_overwrite_uses_sequence_numbers() {
    let mock = MockEs::serve(vec![
        CannedResponse::ok(OS2),
        CannedResponse::new(
            409,
            r#"{"error":{"type":"version_conflict_engine_exception"},"status":409}"#,
        ),
        CannedResponse::ok(r#"{"_id":"p1","_seq_no":7,"_primary_term":3,"policy":{"states":[]}}"#),
        CannedResponse::ok(r#"{"_id":"p1"}"#),
    ])
    .await;
    let f = write_tmp(
        "ism.json",
        r#"{"policies":[{"_id":"p1","_seq_no":1,"_primary_term":1,"policy":{"policy_id":"p1","last_updated_time":1,"schema_version":21,"default_state":"a","states":[{"name":"a","actions":[],"transitions":[]}],"ism_template":[{"index_patterns":["x-*"],"priority":1,"last_updated_time":2}]}}],"total_policies":1}"#,
    );
    let o = meta_run(&[
        "--input",
        f.to_str().unwrap(),
        "--output",
        &host(&mock),
        "--type",
        "policy",
    ])
    .await;
    assert!(o.status.success(), "{}", stderr(&o));
    let reqs = mock.requests();
    assert!(
        reqs[1].starts_with("PUT /_plugins/_ism/policies/p1 "),
        "{}",
        reqs[1]
    );
    assert!(
        !reqs[1].contains("last_updated_time") && !reqs[1].contains("policy_id"),
        "{}",
        reqs[1]
    );
    assert!(
        reqs[2].starts_with("GET /_plugins/_ism/policies/p1 "),
        "{}",
        reqs[2]
    );
    assert!(
        reqs[3].starts_with("PUT /_plugins/_ism/policies/p1?if_seq_no=7&if_primary_term=3 "),
        "{}",
        reqs[3]
    );
}

#[tokio::test]
async fn ism_list_dump_pages_through_all_policies() {
    let mock = MockEs::serve(vec![
        CannedResponse::ok(OS2),
        CannedResponse::ok(
            r#"{"policies":[{"_id":"a","policy":{"states":[]}}],"total_policies":1}"#,
        ),
    ])
    .await;
    let out =
        std::env::temp_dir().join(format!("edrs-contract-{}-ism-out.json", std::process::id()));
    let o = meta_run(&[
        "--input",
        &host(&mock),
        "--output",
        out.to_str().unwrap(),
        "--type",
        "policy",
        "--overwrite",
    ])
    .await;
    assert!(o.status.success(), "{}", stderr(&o));
    assert!(
        mock.requests()[1].starts_with("GET /_plugins/_ism/policies?size=1000&from=0 "),
        "{}",
        mock.requests()[1]
    );
    let line = std::fs::read_to_string(&out).unwrap();
    assert!(line.starts_with("\"{\\\"policies\\\""), "{line}");
}

#[tokio::test]
async fn cross_engine_policy_restores_are_refused_without_the_flag() {
    let ilm = write_tmp(
        "ilm.json",
        r#"{"p":{"policy":{"phases":{"delete":{"min_age":"1d","actions":{"delete":{}}}}}}}"#,
    );
    let mock = MockEs::serve(vec![CannedResponse::ok(OS2)]).await;
    let o = meta_run(&[
        "--input",
        ilm.to_str().unwrap(),
        "--output",
        &host(&mock),
        "--type",
        "policy",
    ])
    .await;
    assert_eq!(o.status.code(), Some(1));
    assert!(stderr(&o).contains("--convertIlmToIsm"), "{}", stderr(&o));
    assert_eq!(mock.request_count(), 1);

    // with the flag: translated and PUT as ISM
    let mock = MockEs::serve(vec![
        CannedResponse::ok(OS2),
        CannedResponse::ok(r#"{"_id":"p"}"#),
    ])
    .await;
    let o = meta_run(&[
        "--input",
        ilm.to_str().unwrap(),
        "--output",
        &host(&mock),
        "--type",
        "policy",
        "--convertIlmToIsm",
    ])
    .await;
    assert!(o.status.success(), "{}", stderr(&o));
    let put = &mock.requests()[1];
    assert!(put.starts_with("PUT /_plugins/_ism/policies/p "), "{put}");
    assert!(
        put.contains(r#""states":[{"name":"pending""#) && put.contains(r#""delete":{}"#),
        "{put}"
    );

    let ism = write_tmp(
        "ism2.json",
        r#"{"policies":[{"_id":"a","policy":{"states":[]}}]}"#,
    );
    let mock = MockEs::serve(vec![CannedResponse::ok(ES8)]).await;
    let o = meta_run(&[
        "--input",
        ism.to_str().unwrap(),
        "--output",
        &host(&mock),
        "--type",
        "policy",
        "--convertIlmToIsm",
    ])
    .await;
    assert_eq!(o.status.code(), Some(1));
    assert!(
        stderr(&o).contains("ISM → ILM conversion is not supported"),
        "{}",
        stderr(&o)
    );
}

#[tokio::test]
async fn template_skip_existing_uses_create_and_index_template_needs_components() {
    let mock = MockEs::serve(vec![
        CannedResponse::ok(ES8),
        CannedResponse::new(400, r#"{"error":{"type":"illegal_argument_exception","reason":"index template [t] already exists"},"status":400}"#),
    ])
    .await;
    let f = write_tmp(
        "tpl.json",
        r#""{\"t\":{\"order\":0,\"index_patterns\":[\"te*\"],\"settings\":{},\"mappings\":{\"type1\":{\"_source\":{\"enabled\":false}}},\"aliases\":{}}}""#,
    );
    let o = meta_run(&[
        "--input",
        f.to_str().unwrap(),
        "--output",
        &host(&mock),
        "--type",
        "template",
        "--skip-existing",
    ])
    .await;
    assert!(o.status.success(), "{}", stderr(&o));
    let put = &mock.requests()[1];
    assert!(put.starts_with("PUT /_template/t?create=true "), "{put}");
    assert!(
        put.contains(r#""mappings":{"_source":{"enabled":false}}"#),
        "{put}"
    );

    let mock = MockEs::serve(vec![
        CannedResponse::ok(ES8),
        CannedResponse::new(
            404,
            r#"{"error":{"type":"resource_not_found_exception"},"status":404}"#,
        ),
    ])
    .await;
    let f = write_tmp(
        "itpl.json",
        r#""{\"index_templates\":[{\"name\":\"it\",\"index_template\":{\"index_patterns\":[\"x*\"],\"composed_of\":[\"c1\"]}}]}""#,
    );
    let o = meta_run(&[
        "--input",
        f.to_str().unwrap(),
        "--output",
        &host(&mock),
        "--type",
        "index_template",
    ])
    .await;
    assert_eq!(o.status.code(), Some(1));
    assert!(
        stderr(&o).contains("restore component templates first"),
        "{}",
        stderr(&o)
    );
    assert!(mock.requests()[1].starts_with("GET /_component_template/c1 "));
    assert_eq!(mock.request_count(), 2);

    // wrong --type for the file: refused before the target is contacted
    let o = meta_run(&[
        "--input",
        f.to_str().unwrap(),
        "--output",
        "http://127.0.0.1:1",
        "--type",
        "template",
    ])
    .await;
    assert_eq!(o.status.code(), Some(1));
    assert!(
        stderr(&o).contains("use --type index_template"),
        "{}",
        stderr(&o)
    );
}

#[tokio::test]
async fn analyzer_close_index_reopens_even_when_the_update_fails() {
    let mock = MockEs::serve(vec![
        CannedResponse::ok(ES8),
        CannedResponse::ok("{}"), // HEAD t
        CannedResponse::ok(r#"{"acknowledged":true}"#), // _close
        CannedResponse::new(400, r#"{"error":{"type":"illegal_argument_exception","reason":"bad analyzer"},"status":400}"#),
        CannedResponse::ok(r#"{"acknowledged":true}"#), // _open
        CannedResponse::ok(r#"{"status":"yellow"}"#),
    ])
    .await;
    let f = write_tmp(
        "an.json",
        r#""{\"s\":{\"settings\":{\"index\":{\"analysis\":{\"analyzer\":{\"a\":{\"type\":\"custom\",\"tokenizer\":\"nope\"}}}}}}}""#,
    );
    let out = format!("{}/t", host(&mock));
    let o = meta_run(&[
        "--input",
        f.to_str().unwrap(),
        "--output",
        &out,
        "--type",
        "analyzer",
        "--closeIndex",
    ])
    .await;
    assert_eq!(o.status.code(), Some(1), "{}", stderr(&o));
    let reqs = mock.requests();
    let lines: Vec<&str> = reqs.iter().map(|r| r.lines().next().unwrap()).collect();
    assert!(lines[1].starts_with("HEAD /t "), "{lines:?}");
    assert!(lines[2].starts_with("POST /t/_close "), "{lines:?}");
    assert!(lines[3].starts_with("PUT /t/_settings "), "{lines:?}");
    assert!(lines[4].starts_with("POST /t/_open "), "{lines:?}");
    assert!(stderr(&o).contains("was reopened"), "{}", stderr(&o));

    // without --closeIndex an existing index is an error naming both flags
    let mock = MockEs::serve(vec![CannedResponse::ok(ES8), CannedResponse::ok("{}")]).await;
    let out = format!("{}/t", host(&mock));
    let o = meta_run(&[
        "--input",
        f.to_str().unwrap(),
        "--output",
        &out,
        "--type",
        "analyzer",
    ])
    .await;
    assert_eq!(o.status.code(), Some(1));
    assert!(
        stderr(&o).contains("--closeIndex") && stderr(&o).contains("--skip-existing"),
        "{}",
        stderr(&o)
    );
}

#[tokio::test]
async fn script_dump_reads_cluster_state() {
    let mock = MockEs::serve(vec![
        CannedResponse::ok(ES8),
        CannedResponse::ok(
            r#"{"metadata":{"stored_scripts":{"s1":{"lang":"mustache","source":"{}"}}}}"#,
        ),
    ])
    .await;
    let o = meta_run(&[
        "--input",
        &host(&mock),
        "--output",
        "$",
        "--type",
        "script",
        "--quiet",
    ])
    .await;
    assert!(o.status.success(), "{}", stderr(&o));
    assert!(
        mock.requests()[1]
            .starts_with("GET /_cluster/state/metadata?filter_path=**.stored_scripts.* "),
        "{}",
        mock.requests()[1]
    );
    assert_eq!(
        stdout(&o).trim(),
        r#""{\"stored_scripts\":[{\"_id\":\"s1\",\"found\":true,\"script\":{\"lang\":\"mustache\",\"source\":\"{}\"}}]}""#
    );
}

#[tokio::test]
async fn host_only_index_and_alias_dumps_use_wildcards_and_node_alias_url() {
    let mock = MockEs::serve(vec![CannedResponse::ok(ES8), CannedResponse::ok("{}")]).await;
    let o = meta_run(&[
        "--input",
        &host(&mock),
        "--output",
        "$",
        "--type",
        "index",
        "--quiet",
    ])
    .await;
    assert!(o.status.success(), "{}", stderr(&o));
    assert!(
        mock.requests()[1].starts_with("GET /* "),
        "{}",
        mock.requests()[1]
    );

    let mock = MockEs::serve(vec![CannedResponse::ok(ES8), CannedResponse::ok("{}")]).await;
    let url = format!("{}/idx/al", host(&mock));
    let o = meta_run(&["--input", &url, "--output", "$", "--type", "alias"]).await;
    assert!(o.status.success(), "{}", stderr(&o));
    assert!(
        mock.requests()[1].starts_with("GET /idx/_alias/al "),
        "{}",
        mock.requests()[1]
    );
    assert!(stderr(&o).contains("<index>/<alias>"), "{}", stderr(&o));
}

#[tokio::test]
async fn rename_rules_apply_to_data_and_conflict_with_an_output_index() {
    let o = run(&[
        "--input",
        "in.json",
        "--output",
        "http://127.0.0.1:1/x",
        "--renamePrefix",
        "a-:b-",
    ]);
    assert_eq!(o.status.code(), Some(2));
    assert!(
        stderr(&o).contains("explicit output index"),
        "{}",
        stderr(&o)
    );
    let o = run(&[
        "--input",
        "in.json",
        "--output",
        "http://127.0.0.1:1",
        "--renameIndex",
        "(?=a):b",
    ]);
    assert_eq!(o.status.code(), Some(2));
    assert!(stderr(&o).contains("look-around"), "{}", stderr(&o));

    let mock = MockEs::serve(vec![
        CannedResponse::ok(ES8),
        CannedResponse::ok(
            r#"{"errors":false,"items":[{"index":{"status":201}},{"index":{"status":201}}]}"#,
        ),
        CannedResponse::ok("{}"),
    ])
    .await;
    let f = write_tmp(
        "data.json",
        "{\"_index\":\"logstash-2026-01-01\",\"_id\":\"1\",\"_source\":{}}\n{\"_index\":\"other\",\"_id\":\"2\",\"_source\":{}}\n",
    );
    let o = meta_run(&[
        "--input",
        f.to_str().unwrap(),
        "--output",
        &host(&mock),
        "--renamePrefix",
        "logstash-:logstash-srctag-",
        "--suffix",
        "_r",
    ])
    .await;
    assert!(o.status.success(), "{}", stderr(&o));
    let bulk = &mock.requests()[1];
    assert!(
        bulk.contains(r#""_index":"logstash-srctag-2026-01-01_r""#),
        "{bulk}"
    );
    assert!(bulk.contains(r#""_index":"other_r""#), "{bulk}");
}

// L-405 (TST-API-010): the scroll id is rotated from every page, and a clear that returns
// 404 (contexts already expired) is not an error.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn scroll_id_rotates_and_clear_404_after_expiry_is_ok() {
    let mock = MockEs::serve(vec![
        CannedResponse::ok(page("s1", &[hit("1", "{\"a\":1}")], 2)),
        CannedResponse::ok(page("s2", &[hit("2", "{\"a\":2}")], 2)),
        CannedResponse::ok(page("s3", &[], 2)),
        CannedResponse::new(404, r#"{"succeeded":true,"num_freed":0}"#),
    ])
    .await;
    let o = run_async(
        vec![
            "--input".into(),
            mock.input_url("idx"),
            "--output".into(),
            "$".into(),
            "--quiet".into(),
        ],
        vec![],
    )
    .await;
    assert!(o.status.success(), "{}", stderr(&o));
    assert_eq!(stdout(&o).lines().count(), 2, "{}", stdout(&o));
    let reqs = mock.requests();
    assert_eq!(reqs.len(), 4, "{reqs:#?}");
    assert!(
        reqs[1].contains("\"s1\""),
        "continuation 1 uses s1: {}",
        reqs[1]
    );
    assert!(
        reqs[2].contains("\"s2\""),
        "continuation 2 uses the rotated s2: {}",
        reqs[2]
    );
    assert!(
        reqs[3].starts_with("DELETE") && reqs[3].contains("s3"),
        "clear the latest id: {}",
        reqs[3]
    );
}

// L-408 (TST-API-013): `hits.total` variants. An int total (`rest_total_hits_as_int`, ES 6
// style) and `{relation: eq}` are exact and reconciled; `gte`, a missing total and a user
// `track_total_hits: false` are estimates and never fail the run.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn hits_total_variants_are_reconciled_only_when_exact() {
    let sa_hits = r#"[{"_index":"idx","_id":"1","_source":{"a":1},"sort":[1]},{"_index":"idx","_id":"2","_source":{"a":2},"sort":[2]}]"#;
    let cases: [(&str, &str, Option<i32>, &str); 6] = [
        ("int exact", r#""total":2,"#, Some(0), ""),
        ("int short", r#""total":5,"#, Some(1), "retrieved 2 of 5"),
        (
            "eq short",
            r#""total":{"value":3,"relation":"eq"},"#,
            Some(1),
            "retrieved 2 of 3",
        ),
        (
            "gte",
            r#""total":{"value":10000,"relation":"gte"},"#,
            Some(0),
            "",
        ),
        ("missing", "", Some(0), ""),
        ("track_total_hits false", "", Some(0), ""),
    ];
    for (name, total, code, msg) in cases {
        let body = format!(
            r#"{{"took":1,"timed_out":false,"_shards":{{"total":1,"successful":1,"skipped":0,"failed":0}},"hits":{{{total}"hits":{sa_hits}}}}}"#
        );
        let mock = MockEs::serve(vec![CannedResponse::ok(body)]).await;
        let search_body = if name.starts_with("track") {
            r#"{"query":{"match_all":{}},"sort":[{"a":"asc"}],"track_total_hits":false}"#
        } else {
            r#"{"query":{"match_all":{}},"sort":[{"a":"asc"}]}"#
        };
        let o = run_async(
            vec![
                "--input".into(),
                mock.input_url("idx"),
                "--output".into(),
                "$".into(),
                "--quiet".into(),
                "--searchAfter".into(),
                "--searchBody".into(),
                search_body.into(),
            ],
            vec![],
        )
        .await;
        assert_eq!(o.status.code(), code, "{name}: {}", stderr(&o));
        assert!(stderr(&o).contains(msg), "{name}: {}", stderr(&o));
        if name.starts_with("track") {
            assert!(
                mock.requests()[0].contains("\"track_total_hits\":false"),
                "user flag kept: {}",
                mock.requests()[0]
            );
        }
    }
}

// L-412 (TST-API-021): bulk responses that the live engines cannot be made to produce on
// demand: per-item 429 (`es_rejected_execution_exception`) is retried for those items only;
// a 413 for the whole request and an item-count mismatch fail the run (exit 1) naming the cause.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn bulk_item_429_retry_413_and_item_count_mismatch() {
    let dir = tempfile::tempdir().unwrap();
    let input = dir.path().join("in.json");
    std::fs::write(
        &input,
        format!("{}\n{}\n", hit("1", "{\"a\":1}"), hit("2", "{\"a\":2}")),
    )
    .unwrap();
    let info = r#"{"version":{"distribution":"opensearch","number":"2.19.0"}}"#;
    let refresh = r#"{"_shards":{"total":1,"successful":1,"failed":0}}"#;
    let args = |url: String| -> Vec<String> {
        vec![
            "--input".into(),
            input.to_str().unwrap().into(),
            "--output".into(),
            url,
            "--noRefresh".into(),
        ]
    };
    // per-item 429, then success for the retried item only
    let mock = MockEs::serve(vec![
        CannedResponse::ok(info),
        CannedResponse::ok(r#"{"took":1,"errors":true,"items":[{"index":{"status":201}},{"index":{"status":429,"error":{"type":"es_rejected_execution_exception","reason":"rejected execution"}}}]}"#),
        CannedResponse::ok(r#"{"took":1,"errors":false,"items":[{"index":{"status":201}}]}"#),
        CannedResponse::ok(refresh),
    ])
    .await;
    let o = run_async(args(mock.input_url("dst")), vec![]).await;
    assert!(o.status.success(), "{}", stderr(&o));
    assert!(stderr(&o).contains("Total Writes: 2"), "{}", stderr(&o));
    let reqs = mock.requests();
    assert_eq!(reqs.len(), 3, "{reqs:#?}");
    assert!(
        reqs[2].contains("\"_id\":\"2\"") && !reqs[2].contains("\"_id\":\"1\""),
        "only the rejected item is resent: {}",
        reqs[2]
    );
    // 413 for the whole request
    let mock = MockEs::serve(vec![
        CannedResponse::ok(info),
        CannedResponse::new(413, r#"{"error":"Request Entity Too Large"}"#),
    ])
    .await;
    let o = run_async(args(mock.input_url("dst")), vec![]).await;
    assert_eq!(o.status.code(), Some(1), "{}", stderr(&o));
    assert!(stderr(&o).contains("413"), "{}", stderr(&o));
    // fewer items than documents
    let mock = MockEs::serve(vec![
        CannedResponse::ok(info),
        CannedResponse::ok(r#"{"took":1,"errors":true,"items":[{"index":{"status":201}}]}"#),
    ])
    .await;
    let o = run_async(args(mock.input_url("dst")), vec![]).await;
    assert_eq!(o.status.code(), Some(1), "{}", stderr(&o));
    assert!(
        stderr(&o).contains("1 items for 2 documents"),
        "{}",
        stderr(&o)
    );
}

// L-413 (TST-API-022): one `_refresh` per written batch (concurrency 1), none with
// `--noRefresh`; a failing refresh is a warning, not a failure.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn refresh_per_batch_no_refresh_and_refresh_failure_is_a_warning() {
    use support::router_mock::{Reply, Route, RouterMock};
    let dir = tempfile::tempdir().unwrap();
    let input = dir.path().join("in.json");
    std::fs::write(
        &input,
        format!(
            "{}\n{}\n{}\n",
            hit("1", "{}"),
            hit("2", "{}"),
            hit("3", "{}")
        ),
    )
    .unwrap();
    for (extra, refresh_status, want_refreshes) in [
        (None, 200, 3),
        (Some("--noRefresh"), 200, 0),
        (None, 500, 3),
    ] {
        let m = RouterMock::builder()
            .route(Route::exact("GET", "/").reply(Reply::ok(
                r#"{"version":{"distribution":"opensearch","number":"2.19.0"}}"#,
            )))
            .route(Route::contains("POST", "/_bulk").reply(Reply::ok(
                r#"{"took":1,"errors":false,"items":[{"index":{"status":201}}]}"#,
            )))
            .route(
                Route::contains("POST", "/_refresh")
                    .reply(Reply::json(refresh_status, r#"{"error":"boom"}"#)),
            )
            .start()
            .await;
        let mut a: Vec<String> = vec![
            "--input".into(),
            input.to_str().unwrap().into(),
            "--output".into(),
            format!("{}/dst", m.url()),
            "--limit".into(),
            "1".into(),
            "--retryAttempts".into(),
            "0".into(),
        ];
        a.extend(extra.map(String::from));
        let o = run_async(a, vec![]).await;
        assert!(
            o.status.success(),
            "{extra:?}/{refresh_status}: {}",
            stderr(&o)
        );
        let reqs = m.requests();
        let bulks = reqs.iter().filter(|r| r.path.ends_with("/_bulk")).count();
        let refreshes = reqs
            .iter()
            .filter(|r| r.path.ends_with("/_refresh"))
            .count();
        assert_eq!(
            (bulks, refreshes),
            (3, want_refreshes),
            "{extra:?}/{refresh_status}"
        );
        if refresh_status != 200 {
            assert!(
                stderr(&o).contains("refresh failed: HTTP 500"),
                "{}",
                stderr(&o)
            );
        }
    }
}

// L-419/L-425 (TST-API-035, TST-API-041): deprecation `Warning` headers are tolerated and each
// distinct text is logged once; `X-Elastic-Product` is ignored.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn warning_headers_are_logged_once_and_not_fatal() {
    use support::router_mock::{Reply, Route, RouterMock};
    let w = r#"299 Elasticsearch-8.19.23-abc "this request accesses system indices, but in a future major version direct access will be prevented by default""#;
    let m = RouterMock::builder()
        .route(Route::exact("GET", "/").reply(
            Reply::ok(r#"{"version":{"number":"8.19.23","build_flavor":"default"},"tagline":"You Know, for Search"}"#)
                .with_header("X-Elastic-Product", "Elasticsearch"),
        ))
        .route(Route::contains("POST", "/idx/_search").reply(Reply::ok(page("s1", &[hit("1", "{}")], 2)).with_header("Warning", w)))
        .route(Route::exact("POST", "/_search/scroll").replies([
            Reply::ok(page("s1", &[hit("2", "{}")], 2)).with_header("Warning", w),
            Reply::ok(page("s1", &[], 2)).with_header("Warning", w),
        ]))
        .route(Route::prefix("DELETE", "/_search/scroll").reply(Reply::ok(r#"{"succeeded":true,"num_freed":1}"#)))
        .start()
        .await;
    let o = run_async(
        vec![
            "--input".into(),
            format!("{}/idx", m.url()),
            "--output".into(),
            "$".into(),
            "--limit".into(),
            "1".into(),
        ],
        vec![],
    )
    .await;
    assert!(o.status.success(), "{}", stderr(&o));
    assert_eq!(stdout(&o).lines().count(), 2);
    assert_eq!(
        stderr(&o)
            .matches("server warning: this request accesses system indices")
            .count(),
        1,
        "{}",
        stderr(&o)
    );
}

// L-421 (TST-API-037): an ISM overwrite that 409s again (changed concurrently) is reported.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn ism_second_409_is_reported() {
    use support::router_mock::{Reply, Route, RouterMock};
    let dir = tempfile::tempdir().unwrap();
    let input = dir.path().join("pol.json");
    std::fs::write(&input, r#"{"policies":[{"_id":"p1","_seq_no":3,"_primary_term":1,"policy":{"policy_id":"p1","default_state":"hot","states":[{"name":"hot","actions":[],"transitions":[]}]}}],"total_policies":1}"#).unwrap();
    let conflict = r#"{"error":{"type":"version_conflict_engine_exception","reason":"conflict"},"status":409}"#;
    let m = RouterMock::builder()
        .route(Route::exact("GET", "/").reply(Reply::ok(
            r#"{"version":{"distribution":"opensearch","number":"2.19.0"}}"#,
        )))
        .route(
            Route::exact("GET", "/_plugins/_ism/policies/p1").reply(Reply::ok(
                r#"{"_id":"p1","_seq_no":7,"_primary_term":2,"policy":{}}"#,
            )),
        )
        .route(Route::exact("PUT", "/_plugins/_ism/policies/p1").reply(Reply::json(409, conflict)))
        .start()
        .await;
    let o = run_async(
        vec![
            "--input".into(),
            input.to_str().unwrap().into(),
            "--output".into(),
            m.url(),
            "--type".into(),
            "policy".into(),
            "--retryAttempts".into(),
            "0".into(),
        ],
        vec![],
    )
    .await;
    assert_eq!(o.status.code(), Some(1), "{}", stderr(&o));
    assert!(
        stderr(&o).contains("changed concurrently"),
        "{}",
        stderr(&o)
    );
    let puts: Vec<_> = m
        .requests()
        .into_iter()
        .filter(|r| r.method == "PUT")
        .collect();
    assert_eq!(puts.len(), 2);
    assert!(
        puts[1]
            .query
            .iter()
            .any(|(k, v)| k == "if_seq_no" && v == "7"),
        "{:?}",
        puts[1].query
    );
    assert!(
        puts[1]
            .query
            .iter()
            .any(|(k, v)| k == "if_primary_term" && v == "2")
    );
}

// L-423 (TST-API-039): a 403 on the version check or on `_cluster/state/metadata` names the
// 'monitor' cluster privilege and never echoes the password.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn script_403_names_the_monitor_privilege() {
    use support::router_mock::{Reply, Route, RouterMock};
    let denied = r#"{"error":{"type":"security_exception","reason":"action [cluster:monitor/main] is unauthorized"},"status":403}"#;
    for root_denied in [true, false] {
        let mut b = RouterMock::builder();
        b = if root_denied {
            b.route(Route::exact("GET", "/").reply(Reply::json(403, denied)))
        } else {
            b.route(Route::exact("GET", "/").reply(Reply::ok(
                r#"{"version":{"number":"8.19.0","build_flavor":"default"},"tagline":"You Know, for Search"}"#,
            )))
            .route(Route::exact("GET", "/_cluster/state/metadata").reply(Reply::json(403, denied)))
        };
        let m = b.start().await;
        let dir = tempfile::tempdir().unwrap();
        let out = dir.path().join("s.json");
        let url = m.url().replacen("http://", "http://low:s3cr3t-pw@", 1);
        let o = run_async(
            vec![
                "--input".into(),
                url,
                "--output".into(),
                out.to_str().unwrap().into(),
                "--type".into(),
                "script".into(),
                "--retryAttempts".into(),
                "0".into(),
            ],
            vec![],
        )
        .await;
        let err = stderr(&o);
        assert_eq!(o.status.code(), Some(1), "{err}");
        assert!(
            err.contains("403") && err.contains("'monitor' cluster privilege"),
            "{err}"
        );
        assert!(!err.contains("s3cr3t-pw"), "{err}");
        assert!(!out.exists());
    }
}

// L-424 (TST-API-040): `GET /` variants. A proxy/login page or a version-less body fails
// with a clear message (never a silent "Elasticsearch 5.0.0"); OpenSearch 1.x with
// `compatibility.override_main_response_version` is detected by its tagline.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn cluster_info_variants_never_fall_back_silently() {
    use support::router_mock::{Reply, Route, RouterMock};
    for (body, needle) in [
        (
            "<html><title>Proxy login</title></html>",
            "did not return JSON",
        ),
        (
            r#"{"name":"n","cluster_name":"c"}"#,
            "without version.number",
        ),
    ] {
        let m = RouterMock::builder()
            .route(Route::exact("GET", "/").reply(Reply::ok(body)))
            .start()
            .await;
        let dir = tempfile::tempdir().unwrap();
        let out = dir.path().join("o.json");
        let o = run_async(
            vec![
                "--input".into(),
                format!("{}/idx", m.url()),
                "--output".into(),
                out.to_str().unwrap().into(),
                "--type".into(),
                "mapping".into(),
            ],
            vec![],
        )
        .await;
        let err = stderr(&o);
        assert_eq!(o.status.code(), Some(1), "{err}");
        assert!(
            err.contains(needle) && err.contains("Failed to detect the engine"),
            "{err}"
        );
        assert!(!err.contains("5.0.0"), "{err}");
        assert!(!out.exists());
    }
    let m = RouterMock::builder()
        .route(Route::exact("GET", "/").reply(Reply::ok(
            r#"{"version":{"number":"7.10.2","build_flavor":"oss"},"tagline":"The OpenSearch Project: https://opensearch.org/"}"#,
        )))
        .route(Route::exact("GET", "/_plugins/_ism/policies").reply(Reply::ok(
            r#"{"policies":[],"total_policies":0}"#,
        )))
        .start()
        .await;
    let dir = tempfile::tempdir().unwrap();
    let out = dir.path().join("p.json");
    let o = run_async(
        vec![
            "--input".into(),
            m.url(),
            "--output".into(),
            out.to_str().unwrap().into(),
            "--type".into(),
            "policy".into(),
        ],
        vec![],
    )
    .await;
    let err = stderr(&o);
    assert_eq!(o.status.code(), Some(0), "{err}");
    assert!(
        err.contains("compatibility.override_main_response_version"),
        "{err}"
    );
    assert!(
        m.requests()
            .iter()
            .any(|r| r.path.starts_with("/_plugins/_ism/policies"))
    );
    assert!(!m.requests().iter().any(|r| r.path.starts_with("/_ilm")));
}

// L-425 (TST-API-041): `X-Elastic-Product` missing or unexpected is tolerated (the tool does not
// enforce the official client's product check); a `Warning` on `GET /` is logged once.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn x_elastic_product_is_tolerated_on_meta_paths() {
    use support::router_mock::{Reply, Route, RouterMock};
    for product in [None, Some("Elasticsearch"), Some("SomethingElse")] {
        let w = r#"299 Elasticsearch-8.19.23-abc "[ignore_throttled] parameter is deprecated""#;
        let mut root = Reply::ok(
            r#"{"version":{"number":"8.19.23","build_flavor":"default"},"tagline":"You Know, for Search"}"#,
        )
        .with_header("Warning", w);
        let mut mapping =
            Reply::ok(r#"{"idx":{"mappings":{"properties":{"a":{"type":"keyword"}}}}}"#)
                .with_header("Warning", w);
        if let Some(p) = product {
            root = root.with_header("X-Elastic-Product", p);
            mapping = mapping.with_header("X-Elastic-Product", p);
        }
        let m = RouterMock::builder()
            .route(Route::exact("GET", "/").reply(root))
            .route(Route::exact("GET", "/idx/_mapping").reply(mapping))
            .start()
            .await;
        let o = run_async(
            vec![
                "--input".into(),
                format!("{}/idx", m.url()),
                "--output".into(),
                "$".into(),
                "--type".into(),
                "mapping".into(),
            ],
            vec![],
        )
        .await;
        let err = stderr(&o);
        assert!(o.status.success(), "{product:?}: {err}");
        assert!(stdout(&o).contains("keyword"), "{}", stdout(&o));
        assert_eq!(
            err.matches("server warning: [ignore_throttled] parameter is deprecated")
                .count(),
            1,
            "{err}"
        );
    }
}

// L-427 (TST-API-051): 5xx per request kind. Idempotent requests (initial search, meta GET)
// are retried; a scroll continuation is not (the cursor may have moved) and the message says
// why and how to recover. Exit 1, nothing committed, no staging left behind.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn http_5xx_per_request_kind() {
    use support::router_mock::{Reply, Route, RouterMock};
    for status in [500u16, 502, 503, 504] {
        let fail =
            format!(r#"{{"error":{{"type":"x","reason":"boom {status}"}},"status":{status}}}"#);
        let m = RouterMock::builder()
            .route(Route::contains("POST", "/idx/_search").replies([
                Reply::json(status, &fail),
                Reply::ok(page("s1", &[hit("1", "{}")], 3)),
            ]))
            .route(Route::exact("POST", "/_search/scroll").reply(Reply::json(status, &fail)))
            .route(
                Route::prefix("DELETE", "/_search/scroll")
                    .reply(Reply::ok(r#"{"succeeded":true,"num_freed":1}"#)),
            )
            .start()
            .await;
        let dir = tempfile::tempdir().unwrap();
        let out = dir.path().join("d.json");
        let o = run_async(
            vec![
                "--input".into(),
                format!("{}/idx", m.url()),
                "--output".into(),
                out.to_str().unwrap().into(),
                "--limit".into(),
                "1".into(),
                "--retryAttempts".into(),
                "2".into(),
                "--retryDelay".into(),
                "1".into(),
            ],
            vec![],
        )
        .await;
        let err = stderr(&o);
        assert_eq!(o.status.code(), Some(1), "{err}");
        let reqs = m.requests();
        let n = |p: &str| {
            reqs.iter()
                .filter(|r| r.method == "POST" && r.path.starts_with(p))
                .count()
        };
        assert_eq!(n("/idx/_search"), 2, "initial search retried once: {err}");
        assert_eq!(
            n("/_search/scroll"),
            1,
            "continuation never retried on {status}: {err}"
        );
        assert!(
            err.contains(&format!("HTTP {status}")) && err.contains(&format!("boom {status}")),
            "{err}"
        );
        assert!(
            err.contains("not retried") && err.contains("--scrollId"),
            "{err}"
        );
        assert!(!out.exists(), "nothing committed");
        let left: Vec<_> = std::fs::read_dir(dir.path())
            .unwrap()
            .flatten()
            .map(|e| e.file_name())
            .collect();
        assert!(left.is_empty(), "staging left: {left:?}");
    }
}

// L-428 (TST-API-052): partial results. A failed shard on one slice only, or `timed_out: true`
// on a continuation, fails the dump with the shard reason and commits nothing; skipped shards
// (can_match pre-filtering) are not failures.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn partial_results_fail_and_commit_nothing() {
    use support::router_mock::{Reply, Route, RouterMock};
    let ok = |sid: &str, ids: &[&str]| {
        page(
            sid,
            &ids.iter().map(|i| hit(i, "{}")).collect::<Vec<_>>(),
            4,
        )
    };
    let failed_slice1 = r#"{"_scroll_id":"s1","timed_out":false,"_shards":{"total":2,"successful":1,"skipped":0,"failed":1,"failures":[{"shard":1,"index":"idx","node":"n","reason":{"type":"circuit_breaking_exception","reason":"[parent] Data too large"}}]},"hits":{"total":{"value":4,"relation":"eq"},"hits":[]}}"#;
    let timed_out = r#"{"_scroll_id":"s0","timed_out":true,"_shards":{"total":1,"successful":1,"skipped":0,"failed":0},"hits":{"total":{"value":4,"relation":"eq"},"hits":[]}}"#;
    let skipped = r#"{"_scroll_id":"s0","timed_out":false,"_shards":{"total":4,"successful":4,"skipped":3,"failed":0},"hits":{"total":{"value":2,"relation":"eq"},"hits":[{"_index":"idx","_id":"a","_source":{}},{"_index":"idx","_id":"b","_source":{}}]}}"#;
    let cases: Vec<(&str, RouterMock, Vec<&str>, Option<&str>)> = vec![
        (
            "one slice fails",
            RouterMock::builder()
                .route(
                    Route::contains("POST", "/idx/_search")
                        .body_contains("\"id\":0")
                        .reply(Reply::ok(ok("s0", &["1", "2"]))),
                )
                .route(
                    Route::contains("POST", "/idx/_search")
                        .body_contains("\"id\":1")
                        .reply(Reply::ok(failed_slice1)),
                )
                .route(Route::exact("POST", "/_search/scroll").reply(Reply::ok(ok("s0", &[]))))
                .route(
                    Route::prefix("DELETE", "/_search/scroll")
                        .reply(Reply::ok(r#"{"succeeded":true,"num_freed":1}"#)),
                )
                .start()
                .await,
            vec!["--slices", "2"],
            Some("idx[1]: circuit_breaking_exception: [parent] Data too large"),
        ),
        (
            "continuation timed out",
            RouterMock::builder()
                .route(Route::contains("POST", "/idx/_search").reply(Reply::ok(ok("s0", &["1"]))))
                .route(Route::exact("POST", "/_search/scroll").reply(Reply::ok(timed_out)))
                .route(
                    Route::prefix("DELETE", "/_search/scroll")
                        .reply(Reply::ok(r#"{"succeeded":true,"num_freed":1}"#)),
                )
                .start()
                .await,
            vec![],
            Some("timed_out=true"),
        ),
        (
            "skipped shards are fine",
            RouterMock::builder()
                .route(Route::contains("POST", "/idx/_search").reply(Reply::ok(skipped)))
                .route(Route::exact("POST", "/_search/scroll").reply(Reply::ok(ok("s0", &[]))))
                .route(
                    Route::prefix("DELETE", "/_search/scroll")
                        .reply(Reply::ok(r#"{"succeeded":true,"num_freed":1}"#)),
                )
                .start()
                .await,
            vec![],
            None,
        ),
    ];
    for (name, m, extra, want) in cases {
        let dir = tempfile::tempdir().unwrap();
        let out = dir.path().join("d.json");
        let mut args: Vec<String> = vec![
            "--input".into(),
            format!("{}/idx", m.url()),
            "--output".into(),
            out.to_str().unwrap().into(),
            "--retryAttempts".into(),
            "0".into(),
        ];
        args.extend(extra.iter().map(|s| s.to_string()));
        let o = run_async(args, vec![]).await;
        let err = stderr(&o);
        match want {
            Some(w) => {
                assert_eq!(o.status.code(), Some(1), "{name}: {err}");
                assert!(err.contains(w), "{name}: {err}");
                assert!(!out.exists(), "{name}: committed");
                assert_eq!(
                    std::fs::read_dir(dir.path()).unwrap().count(),
                    0,
                    "{name}: staging left"
                );
            }
            None => {
                assert!(o.status.success(), "{name}: {err}");
                assert_eq!(
                    std::fs::read_to_string(&out).unwrap().lines().count(),
                    2,
                    "{name}"
                );
            }
        }
    }
}

// L-431 (TST-API-055): TLS failures against `openssl s_server` endpoints built from the cert
// factory (scripts/tls/certs.sh). Each failure: exit 1, one message naming the cause, no retry
// loop (TLS errors are permanent). Success cases reach HTTP (s_server -www answers HTML, so the
// run stops at "did not return JSON"). Skipped when openssl (>= 3.4 for -not_before) is missing.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn tls_failures_have_one_clear_message() {
    struct Srv(std::process::Child);
    impl Drop for Srv {
        fn drop(&mut self) {
            let _ = self.0.kill();
            let _ = self.0.wait();
        }
    }
    let dir = tempfile::tempdir().unwrap();
    let certs = dir.path().join("certs");
    let script = concat!(env!("CARGO_MANIFEST_DIR"), "/scripts/tls/certs.sh");
    let ok = std::process::Command::new("bash")
        .arg(script)
        .arg(&certs)
        .output()
        .is_ok_and(|o| o.status.success());
    if !ok {
        println!("SKIP: openssl cert factory unavailable");
        return;
    }
    let free_port = || {
        let l = std::net::TcpListener::bind("127.0.0.1:0").unwrap();
        l.local_addr().unwrap().port()
    };
    let start = |cert: &str, extra: &[&str]| -> (Srv, u16) {
        let port = free_port();
        let child = std::process::Command::new("openssl")
            .current_dir(&certs)
            .args(["s_server", "-quiet", "-www", "-accept", &port.to_string()])
            .args([
                "-cert",
                &format!("{cert}.pem"),
                "-key",
                &format!("{cert}.key"),
            ])
            .args(extra)
            .stdout(std::process::Stdio::null())
            .stderr(std::process::Stdio::null())
            .spawn()
            .unwrap();
        for _ in 0..100 {
            if std::net::TcpStream::connect(("127.0.0.1", port)).is_ok() {
                break;
            }
            std::thread::sleep(std::time::Duration::from_millis(50));
        }
        (Srv(child), port)
    };
    let plain_port = free_port();
    let plain = std::net::TcpListener::bind(("127.0.0.1", plain_port)).unwrap();
    std::thread::spawn(move || {
        use std::io::{Read, Write};
        for mut s in plain.incoming().flatten() {
            let mut b = [0u8; 1024];
            let _ = s.read(&mut b);
            let _ = s.write_all(b"HTTP/1.1 400 Bad Request\r\ncontent-length: 0\r\n\r\n");
        }
    });
    let ca = certs.join("ca.pem").to_str().unwrap().to_string();
    let key = |n: &str| certs.join(n).to_str().unwrap().to_string();
    let servers = [
        ("ok", start("ok", &[])),
        ("expired", start("expired", &[])),
        ("future", start("future", &[])),
        ("wrongname", start("wrongname", &[])),
        ("iponly", start("iponly", &[])),
        ("unknown", start("unknown", &[])),
        ("mtls", start("ok", &["-Verify", "1", "-CAfile", "ca.pem"])),
        (
            "tls11",
            start("ok", &["-tls1_1", "-cipher", "DEFAULT:@SECLEVEL=0"]),
        ),
    ];
    let port = |n: &str| servers.iter().find(|(k, _)| *k == n).unwrap().1.1;
    let reached_http = "did not return JSON";
    type Env = Vec<(String, String)>;
    let cases: Vec<(&str, String, Vec<String>, Env, &str)> = vec![
        (
            "valid",
            format!("https://localhost:{}/idx", port("ok")),
            vec!["--input-ca".into(), ca.clone()],
            vec![],
            reached_http,
        ),
        (
            "valid by IP SAN",
            format!("https://127.0.0.1:{}/idx", port("iponly")),
            vec!["--input-ca".into(), ca.clone()],
            vec![],
            reached_http,
        ),
        (
            "unknown CA",
            format!("https://localhost:{}/idx", port("unknown")),
            vec!["--input-ca".into(), ca.clone()],
            vec![],
            "not signed by a trusted CA",
        ),
        (
            "no CA given",
            format!("https://localhost:{}/idx", port("ok")),
            vec![],
            vec![],
            "not signed by a trusted CA",
        ),
        (
            "expired",
            format!("https://localhost:{}/idx", port("expired")),
            vec!["--input-ca".into(), ca.clone()],
            vec![],
            "certificate has expired",
        ),
        (
            "not yet valid",
            format!("https://localhost:{}/idx", port("future")),
            vec!["--input-ca".into(), ca.clone()],
            vec![],
            "not valid yet",
        ),
        (
            "hostname mismatch",
            format!("https://localhost:{}/idx", port("wrongname")),
            vec!["--input-ca".into(), ca.clone()],
            vec![],
            "hostname mismatch",
        ),
        (
            "IP SAN only, by name",
            format!("https://localhost:{}/idx", port("iponly")),
            vec!["--input-ca".into(), ca.clone()],
            vec![],
            "hostname mismatch",
        ),
        (
            "client cert missing",
            format!("https://localhost:{}/idx", port("mtls")),
            vec!["--input-ca".into(), ca.clone()],
            vec![],
            "requires a client certificate",
        ),
        (
            "client cert",
            format!("https://localhost:{}/idx", port("mtls")),
            vec![
                "--input-ca".into(),
                ca.clone(),
                "--input-cert".into(),
                key("client.pem"),
                "--input-key".into(),
                key("client.key"),
            ],
            vec![],
            reached_http,
        ),
        (
            "encrypted key",
            format!("https://localhost:{}/idx", port("mtls")),
            vec![
                "--input-ca".into(),
                ca.clone(),
                "--input-cert".into(),
                key("client.pem"),
                "--input-key".into(),
                key("client-enc.key"),
                "--input-pass".into(),
                "clientpass".into(),
            ],
            vec![],
            reached_http,
        ),
        (
            "wrong --pass",
            format!("https://localhost:{}/idx", port("mtls")),
            vec![
                "--input-ca".into(),
                ca.clone(),
                "--input-cert".into(),
                key("client.pem"),
                "--input-key".into(),
                key("client-enc.key"),
                "--input-pass".into(),
                "WRONG-pass".into(),
            ],
            vec![],
            "wrong --pass",
        ),
        (
            "legacy encrypted PEM",
            format!("https://localhost:{}/idx", port("mtls")),
            vec![
                "--input-ca".into(),
                ca.clone(),
                "--input-cert".into(),
                key("client.pem"),
                "--input-key".into(),
                key("client-legacy.key"),
                "--input-pass".into(),
                "clientpass".into(),
            ],
            vec![],
            "Legacy encrypted PEM key",
        ),
        (
            "TLS 1.1 only",
            format!("https://localhost:{}/idx", port("tls11")),
            vec!["--input-ca".into(), ca.clone()],
            vec![],
            "TLS 1.2+ is required",
        ),
        (
            "https to plain port",
            format!("https://localhost:{plain_port}/idx"),
            vec!["--input-ca".into(), ca.clone()],
            vec![],
            "use http://",
        ),
        (
            "http to TLS port",
            format!("http://localhost:{}/idx", port("ok")),
            vec![],
            vec![],
            "use https://",
        ),
        (
            "--insecure",
            format!("https://localhost:{}/idx", port("unknown")),
            vec!["--insecure".into()],
            vec![],
            "verification is DISABLED",
        ),
        (
            "NODE_TLS_REJECT_UNAUTHORIZED=0",
            format!("https://localhost:{}/idx", port("unknown")),
            vec![],
            vec![("NODE_TLS_REJECT_UNAUTHORIZED".into(), "0".into())],
            "verification is DISABLED",
        ),
    ];
    for (name, url, extra, env, want) in cases {
        let mut args: Vec<String> = vec![
            "--input".into(),
            url,
            "--output".into(),
            dir.path().join("o.json").to_str().unwrap().into(),
            "--type".into(),
            "mapping".into(),
            "--retryAttempts".into(),
            "2".into(),
            "--retryDelay".into(),
            "1".into(),
        ];
        args.extend(extra);
        let o = run_async(args, env).await;
        let err = stderr(&o);
        assert_eq!(o.status.code(), Some(1), "{name}: {err}");
        assert!(err.contains(want), "{name}: want {want:?}: {err}");
        assert!(
            !err.contains("clientpass") && !err.contains("WRONG-pass"),
            "{name}: {err}"
        );
        if want != reached_http
            && !name.starts_with("http to")
            && !name.starts_with("--")
            && !name.starts_with("NODE")
        {
            assert!(
                !err.contains("retrying"),
                "{name}: TLS failure was retried: {err}"
            );
        }
        assert!(!dir.path().join("o.json").exists());
    }
    drop(servers);
}

// L-441 (TST-BRK-NET-003): the connection is reset after the cluster read (and applied) a
// `_bulk` request. Explicit ids: re-sent, harmless. Auto-id documents: never re-sent, the run
// fails and says why (a re-send would duplicate them). `create`: the re-sent request's 409s are
// counted as written by the first attempt, not as failures.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn bulk_reset_after_apply_is_never_a_silent_duplicate() {
    use support::router_mock::{Fault, Reply, Route, RouterMock};
    let info = r#"{"version":{"distribution":"opensearch","number":"2.19.0"}}"#;
    let dir = tempfile::tempdir().unwrap();
    let with_ids = dir.path().join("ids.json");
    std::fs::write(
        &with_ids,
        format!("{}\n{}\n", hit("1", "{\"a\":1}"), hit("2", "{\"a\":2}")),
    )
    .unwrap();
    let no_ids = dir.path().join("src.json");
    std::fs::write(&no_ids, "{\"a\":1}\n{\"a\":2}\n").unwrap();
    let mock = |second: &str| {
        let second = second.to_string();
        async move {
            RouterMock::builder()
                .route(Route::exact("GET", "/").reply(Reply::ok(info)))
                .route(
                    Route::contains("POST", "/_bulk")
                        .replies([Reply::ok("{}").with_fault(Fault::Reset), Reply::ok(second)]),
                )
                .fallback(Reply::ok(
                    r#"{"_shards":{"total":1,"successful":1,"failed":0}}"#,
                ))
                .start()
                .await
        }
    };
    let args = |input: &std::path::Path, url: String, extra: &[&str]| -> Vec<String> {
        let mut a: Vec<String> = vec![
            "--input".into(),
            input.to_str().unwrap().into(),
            "--output".into(),
            url,
            "--noRefresh".into(),
            "--retryDelay".into(),
            "1".into(),
        ];
        a.extend(extra.iter().map(|s| s.to_string()));
        a
    };
    let bulks = |m: &RouterMock| {
        m.requests()
            .iter()
            .filter(|r| r.path.ends_with("/_bulk"))
            .count()
    };

    // explicit ids: the whole batch is re-sent once and the run succeeds
    let m = mock(
        r#"{"took":1,"errors":false,"items":[{"index":{"status":200}},{"index":{"status":200}}]}"#,
    )
    .await;
    let o = run_async(args(&with_ids, format!("{}/dst", m.url()), &[]), vec![]).await;
    let err = stderr(&o);
    assert!(o.status.success(), "{err}");
    assert_eq!(bulks(&m), 2, "{err}");
    assert!(err.contains("Total Writes: 2"), "{err}");

    // auto ids: sent once, never re-sent, exit 1 with the reason
    let m = mock(
        r#"{"took":1,"errors":false,"items":[{"index":{"status":201}},{"index":{"status":201}}]}"#,
    )
    .await;
    let o = run_async(args(&no_ids, format!("{}/dst", m.url()), &[]), vec![]).await;
    let err = stderr(&o);
    assert_eq!(o.status.code(), Some(1), "{err}");
    assert_eq!(bulks(&m), 1, "an auto-id batch must not be re-sent: {err}");
    assert!(
        err.contains("not re-sent") && err.contains("2 document(s) without _id"),
        "{err}"
    );

    // create: the re-sent request answers 409 for what the first attempt created
    let m = mock(r#"{"took":1,"errors":true,"items":[{"create":{"status":409,"error":{"type":"version_conflict_engine_exception","reason":"document already exists"}}},{"create":{"status":409,"error":{"type":"version_conflict_engine_exception","reason":"document already exists"}}}]}"#).await;
    let o = run_async(
        args(
            &with_ids,
            format!("{}/dst", m.url()),
            &[
                "--bulkAction",
                "create",
                "--ignore-es-write-errors",
                "false",
            ],
        ),
        vec![],
    )
    .await;
    let err = stderr(&o);
    assert!(o.status.success(), "{err}");
    assert_eq!(bulks(&m), 2, "{err}");
    assert!(err.contains("Total Writes: 2"), "{err}");
    assert!(
        err.contains("counted as written by the earlier attempt"),
        "{err}"
    );
}

// L-442 (TST-BRK-NET-004): a server that accepts and never answers, sends headers then stops,
// or stops mid-body cannot hang the run: without --timeout, the idle timeout (--idleTimeout,
// default 300 s) ends the request, it is retried where that is safe, and the run then fails
// with a message naming the limit, well within the configured bound.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn stalled_server_fails_within_the_idle_timeout() {
    use std::time::Duration;
    use support::router_mock::{Fault, Reply, Route, RouterMock};
    let info = r#"{"version":{"distribution":"opensearch","number":"2.19.0"}}"#;
    let stall = Duration::from_secs(60);
    let body = page("s1", &[hit("1", "{}")], 1);
    for (label, fault) in [
        ("never answers", Fault::Delay(stall)),
        ("headers then silence", Fault::StallAfterHeaders(stall)),
        ("stops mid-body", Fault::StallMidBody { after: 10, stall }),
    ] {
        let m = RouterMock::builder()
            .route(Route::exact("GET", "/").reply(Reply::ok(info)))
            .route(
                Route::contains("POST", "/idx/_search").reply(Reply::ok(&body).with_fault(fault)),
            )
            .fallback(Reply::ok(r#"{"succeeded":true,"num_freed":1}"#))
            .start()
            .await;
        let dir = tempfile::tempdir().unwrap();
        let out = dir.path().join("d.json");
        let t = std::time::Instant::now();
        let o = run_async(
            vec![
                "--input".into(),
                format!("{}/idx", m.url()),
                "--output".into(),
                out.to_str().unwrap().into(),
                "--idleTimeout".into(),
                "1".into(),
                "--retryAttempts".into(),
                "1".into(),
                "--retryDelay".into(),
                "1".into(),
            ],
            vec![],
        )
        .await;
        let took = t.elapsed();
        let err = stderr(&o);
        assert_eq!(o.status.code(), Some(1), "{label}: {err}");
        assert!(took < Duration::from_secs(20), "{label}: took {took:?}");
        assert!(err.contains("--idleTimeout"), "{label}: {err}");
        assert!(!out.exists(), "{label}: nothing committed");
        let searches = m
            .requests()
            .iter()
            .filter(|r| r.path.starts_with("/idx/_search"))
            .count();
        assert_eq!(
            searches, 2,
            "{label}: the initial search is retried once: {err}"
        );
    }
}

// L-443 (TST-BRK-NET-006): a load balancer closes idle keep-alive connections (after 300 ms here,
// with a FIN or an RST) while the dump waits longer than that between pages
// (--throttleInterval 600 with a 300 ms idle close; --bufferSize 1 so retrieval waits for the throttled
// sink). The next scroll continuation must not fail on the dead pooled
// connection: the dump completes with every page.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn stale_keep_alive_connections_do_not_fail_continuations() {
    use std::time::Duration;
    use support::router_mock::{Reply, Route, RouterMock};
    let info = r#"{"version":{"distribution":"opensearch","number":"2.19.0"}}"#;
    const N: usize = 10;
    for rst in [false, true] {
        let m = RouterMock::builder()
            .idle_close(Duration::from_millis(300), rst)
            .route(Route::exact("GET", "/").reply(Reply::ok(info)))
            .route(
                Route::contains("POST", "/idx/_search").reply(Reply::ok(page(
                    "s1",
                    &[hit("1", "{}")],
                    N,
                ))),
            )
            .route(
                Route::exact("POST", "/_search/scroll").replies(
                    (2..=N)
                        .map(|i| Reply::ok(page("s1", &[hit(&i.to_string(), "{}")], N)))
                        .chain([Reply::ok(page("s1", &[], N))]),
                ),
            )
            .fallback(Reply::ok(r#"{"succeeded":true,"num_freed":1}"#))
            .start()
            .await;
        let dir = tempfile::tempdir().unwrap();
        let out = dir.path().join("d.json");
        let o = run_async(
            vec![
                "--input".into(),
                format!("{}/idx", m.url()),
                "--output".into(),
                out.to_str().unwrap().into(),
                "--limit".into(),
                "1".into(),
                "--throttleInterval".into(),
                "600".into(),
                "--bufferSize".into(),
                "1".into(),
            ],
            vec![],
        )
        .await;
        let err = stderr(&o);
        assert!(o.status.success(), "rst={rst}: {err}");
        assert_eq!(
            std::fs::read_to_string(&out).unwrap().lines().count(),
            N,
            "rst={rst}: {err}"
        );
        assert!(
            m.idle_closed() >= 2,
            "rst={rst}: the LB closed idle connections ({} closes, {} connections, requests {:?})",
            m.idle_closed(),
            m.connections(),
            m.requests()
                .iter()
                .map(|r| (r.conn, r.path.clone()))
                .collect::<Vec<_>>()
        );
        eprintln!(
            "rst={rst}: idle closes {}, connections {}",
            m.idle_closed(),
            m.connections()
        );
    }
}

// L-444 (TST-BRK-SRV-001, Q3): bulk items keep failing (a 429 storm longer than the retry
// budget, a flood-stage `cluster_block_exception` 429, `index.blocks.write` 403 items). By
// default the run continues past them but exits 3 ("finished with failed writes"); only an
// explicit --ignore-es-write-errors (or --ignore-errors) gives 0; `false` stops with 1.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn failed_writes_exit_non_zero_unless_accepted_explicitly() {
    use support::router_mock::{Reply, Route, RouterMock};
    let info = r#"{"version":{"distribution":"opensearch","number":"2.19.0"}}"#;
    let dir = tempfile::tempdir().unwrap();
    let input = dir.path().join("in.json");
    std::fs::write(
        &input,
        format!("{}\n{}\n", hit("1", "{\"a\":1}"), hit("2", "{\"a\":2}")),
    )
    .unwrap();
    let item = |status: u16, ty: &str| {
        format!(
            r#"{{"index":{{"status":{status},"error":{{"type":"{ty}","reason":"{ty} reason"}}}}}}"#
        )
    };
    let cases = [
        ("429 storm", item(429, "es_rejected_execution_exception")),
        ("flood stage", item(429, "cluster_block_exception")),
        ("write block", item(403, "cluster_block_exception")),
    ];
    for (label, failing) in cases {
        let body = format!(
            r#"{{"took":1,"errors":true,"items":[{{"index":{{"status":201}}}},{failing}]}}"#
        );
        let retry_body = format!(r#"{{"took":1,"errors":true,"items":[{failing}]}}"#);
        for (flags, code) in [
            (vec![], 3),
            (vec!["--ignore-es-write-errors"], 0),
            (vec!["--ignore-errors"], 0),
            (vec!["--ignore-es-write-errors", "false"], 1),
        ] {
            let m = RouterMock::builder()
                .route(Route::exact("GET", "/").reply(Reply::ok(info)))
                .route(
                    Route::contains("POST", "/_bulk")
                        .replies([Reply::ok(body.clone()), Reply::ok(retry_body.clone())]),
                )
                .fallback(Reply::ok(
                    r#"{"_shards":{"total":1,"successful":1,"failed":0}}"#,
                ))
                .start()
                .await;
            let mut args: Vec<String> = vec![
                "--input".into(),
                input.to_str().unwrap().into(),
                "--output".into(),
                format!("{}/dst", m.url()),
                "--noRefresh".into(),
                "--retryAttempts".into(),
                "1".into(),
                "--retryDelay".into(),
                "1".into(),
            ];
            args.extend(flags.iter().map(|f| f.to_string()));
            let o = run_async(args, vec![]).await;
            let err = stderr(&o);
            assert_eq!(o.status.code(), Some(code), "{label} {flags:?}: {err}");
            assert!(
                err.contains("cluster_block_exception")
                    || err.contains("es_rejected_execution_exception"),
                "{label}: {err}"
            );
            if code == 3 {
                assert!(
                    err.contains("Failed Writes: 1")
                        && err.contains("1 document(s) were not written")
                        && err.contains("--ignore-es-write-errors"),
                    "{label}: {err}"
                );
            }
            if code == 0 {
                assert!(err.contains("Failed Writes: 1"), "{label}: {err}");
            }
        }
    }
}

// L-445 (TST-BRK-SRV-006): a response larger than --maxResponseMb (default 256) is refused
// while reading, with or without Content-Length, with a hint to lower --limit; it is not
// retried (the same request would return the same body). The 512 MB cgroup run with an
// endless body is scripts/res/huge-response.sh.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn oversized_responses_fail_with_a_limit_hint() {
    use support::router_mock::{Reply, Route, RouterMock};
    let info = r#"{"version":{"distribution":"opensearch","number":"2.19.0"}}"#;
    let big = "x".repeat(1_200_000);
    let body = page("s1", &[hit("1", &format!("{{\"blob\":\"{big}\"}}"))], 1);
    for chunked in [false, true] {
        let reply = if chunked {
            Reply::ok(&body).chunked()
        } else {
            Reply::ok(&body)
        };
        let m = RouterMock::builder()
            .route(Route::exact("GET", "/").reply(Reply::ok(info)))
            .route(Route::contains("POST", "/idx/_search").reply(reply))
            .route(Route::exact("POST", "/_search/scroll").reply(Reply::ok(page("s1", &[], 1))))
            .fallback(Reply::ok(r#"{"succeeded":true,"num_freed":1}"#))
            .start()
            .await;
        let dir = tempfile::tempdir().unwrap();
        let out = dir.path().join("d.json");
        let args = |cap: &str| -> Vec<String> {
            vec![
                "--input".into(),
                format!("{}/idx", m.url()),
                "--output".into(),
                out.to_str().unwrap().into(),
                "--maxResponseMb".into(),
                cap.into(),
                "--overwrite".into(),
                "--retryDelay".into(),
                "1".into(),
            ]
        };
        let o = run_async(args("1"), vec![]).await;
        let err = stderr(&o);
        assert_eq!(o.status.code(), Some(1), "chunked={chunked}: {err}");
        assert!(
            err.contains("larger than 1 MB (--maxResponseMb)") && err.contains("lower --limit"),
            "chunked={chunked}: {err}"
        );
        let searches = m
            .requests()
            .iter()
            .filter(|r| r.path.starts_with("/idx/_search"))
            .count();
        assert_eq!(searches, 1, "not retried: {err}");
        assert!(!out.exists());
        // a cap above the page size reads it
        let o = run_async(args("2"), vec![]).await;
        assert!(o.status.success(), "chunked={chunked}: {}", stderr(&o));
    }
}

// L-446 (TST-BRK-SRV-009): the server reports an exact total for a scroll but ends the read
// early (an empty continuation after 2 of 5): the dump fails, names both numbers and commits
// nothing. With --size the total is not a promise and nothing is checked.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn scroll_total_mismatch_is_an_error() {
    use support::router_mock::{Reply, Route, RouterMock};
    let info = r#"{"version":{"distribution":"opensearch","number":"2.19.0"}}"#;
    let m = RouterMock::builder()
        .route(Route::exact("GET", "/").reply(Reply::ok(info)))
        .route(
            Route::contains("POST", "/idx/_search").reply(Reply::ok(page(
                "s1",
                &[hit("1", "{}"), hit("2", "{}")],
                5,
            ))),
        )
        .route(Route::exact("POST", "/_search/scroll").reply(Reply::ok(page("s1", &[], 5))))
        .fallback(Reply::ok(r#"{"succeeded":true,"num_freed":1}"#))
        .start()
        .await;
    let dir = tempfile::tempdir().unwrap();
    let out = dir.path().join("d.json");
    let args = |extra: &[&str]| -> Vec<String> {
        let mut a: Vec<String> = vec![
            "--input".into(),
            format!("{}/idx", m.url()),
            "--output".into(),
            out.to_str().unwrap().into(),
            "--overwrite".into(),
        ];
        a.extend(extra.iter().map(|s| s.to_string()));
        a
    };
    let o = run_async(args(&[]), vec![]).await;
    let err = stderr(&o);
    assert_eq!(o.status.code(), Some(1), "{err}");
    assert!(err.contains("retrieved 2 of 5"), "{err}");
    assert!(!out.exists(), "nothing committed");
    let o = run_async(args(&["--size", "2"]), vec![]).await;
    assert!(o.status.success(), "{}", stderr(&o));
}

// L-448 (TST-BRK-IN-004): a metadata dump restored with --type data is refused before
// anything is written (exit 2, "looks like a --type X dump"); a data dump restored with a
// metadata --type is refused the same way; ordinary --sourceOnly documents that merely contain
// keys like `settings` are not.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn wrong_type_inputs_are_refused_before_writing() {
    use support::router_mock::{Reply, Route, RouterMock};
    let info = r#"{"version":{"distribution":"opensearch","number":"2.19.0"}}"#;
    let m = RouterMock::builder()
        .route(Route::exact("GET", "/").reply(Reply::ok(info)))
        .fallback(Reply::ok(r#"{"took":1,"errors":false,"items":[]}"#))
        .start()
        .await;
    let fx = std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("tests/fixtures/node");
    for engine in ["opensearch-2.19.7", "elasticsearch-8.19.23"] {
        for ty in [
            "mapping",
            "settings",
            "analyzer",
            "alias",
            "index",
            "template",
            "component_template",
            "index_template",
            "policy",
            "script",
        ] {
            let f = fx.join(engine).join(format!("{ty}.json"));
            let o = run_async(
                vec![
                    "--input".into(),
                    f.to_str().unwrap().into(),
                    "--output".into(),
                    format!("{}/dst", m.url()),
                    "--type".into(),
                    "data".into(),
                ],
                vec![],
            )
            .await;
            let err = stderr(&o);
            assert_eq!(o.status.code(), Some(2), "{engine} {ty}: {err}");
            assert!(
                err.contains("looks like a --type") && err.contains("not --type data"),
                "{engine} {ty}: {err}"
            );
        }
        // a data dump with a metadata type
        let f = fx.join(engine).join("data.json");
        let o = run_async(
            vec![
                "--input".into(),
                f.to_str().unwrap().into(),
                "--output".into(),
                format!("{}/dst", m.url()),
                "--type".into(),
                "mapping".into(),
            ],
            vec![],
        )
        .await;
        let err = stderr(&o);
        assert_ne!(o.status.code(), Some(0), "{engine}: {err}");
        assert!(
            err.contains("looks like a --type data dump"),
            "{engine}: {err}"
        );
    }
    let bulks = m
        .requests()
        .iter()
        .filter(|r| r.path.contains("_bulk") || r.method == "PUT")
        .count();
    assert_eq!(bulks, 0, "nothing written");
    // plain documents that only look similar are restored
    let dir = tempfile::tempdir().unwrap();
    let f = dir.path().join("docs.json");
    std::fs::write(
        &f,
        "{\"user\":{\"settings\":{\"theme\":\"dark\"}}}\n{\"a\":{\"mappings\":{\"x\":1}}}\n",
    )
    .unwrap();
    let out = dir.path().join("out.json");
    let o = run_async(
        vec![
            "--input".into(),
            f.to_str().unwrap().into(),
            "--output".into(),
            out.to_str().unwrap().into(),
        ],
        vec![],
    )
    .await;
    assert!(o.status.success(), "{}", stderr(&o));
}

// L-449 (TST-BRK-IN-005): malformed values the splitter passes through (unbalanced brackets,
// a stray `}`, top-level scalars and arrays, NaN/Infinity, trailing commas) are rejected one
// by one with their input line number; the good documents of the same batch are written in
// one bulk request, and the run exits 1. With --ignore-es-write-errors false the first bad line
// stops the run before its batch is sent.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn malformed_values_are_rejected_per_line_with_their_number() {
    use support::router_mock::{Reply, Route, RouterMock};
    let info = r#"{"version":{"distribution":"opensearch","number":"2.19.0"}}"#;
    let lines = [
        r#"{"_id":"g1","_source":{"a":1}}"#,
        r#"{"a":1]"#,
        r#"{"_id":"g2","_source":{"a":2}}"#,
        "123",
        r#""x""#,
        "null",
        "[1,2]",
        r#"{"_id":"g3","_source":{"a":NaN}}"#,
        r#"{"_id":"g4","_source":{"a":Infinity}}"#,
        r#"{"_id":"g5","_source":{"a":5,}}"#,
        r#"{"_id":"g6","_source":{"a":6}}"#,
        "}",
        r#"{"_id":"g7","_source":{"a":7}}"#,
    ];
    let dir = tempfile::tempdir().unwrap();
    let input = dir.path().join("in.json");
    std::fs::write(&input, lines.join("\n") + "\n").unwrap();
    let ok_items = r#"{"took":1,"errors":false,"items":[{"index":{"status":201}},{"index":{"status":201}},{"index":{"status":201}},{"index":{"status":201}}]}"#;
    let m = RouterMock::builder()
        .route(Route::exact("GET", "/").reply(Reply::ok(info)))
        .route(Route::contains("POST", "/_bulk").reply(Reply::ok(ok_items)))
        .fallback(Reply::ok(
            r#"{"_shards":{"total":1,"successful":1,"failed":0}}"#,
        ))
        .start()
        .await;
    let o = run_async(
        vec![
            "--input".into(),
            input.to_str().unwrap().into(),
            "--output".into(),
            format!("{}/dst", m.url()),
            "--ignore-errors".into(),
            "--noRefresh".into(),
        ],
        vec![],
    )
    .await;
    let err = stderr(&o);
    assert_eq!(o.status.code(), Some(1), "{err}");
    let bulks: Vec<String> = m
        .requests()
        .iter()
        .filter(|r| r.path.ends_with("/_bulk"))
        .map(|r| r.body_str())
        .collect();
    assert_eq!(bulks.len(), 1, "{err}");
    for good in ["g1", "g2", "g6", "g7"] {
        assert!(
            bulks[0].contains(&format!("\"_id\":\"{good}\"")),
            "{}",
            bulks[0]
        );
    }
    assert_eq!(bulks[0].lines().count(), 8, "4 docs: {}", bulks[0]);
    let rejected: Vec<&str> = err
        .lines()
        .filter(|l| l.contains("rejected document"))
        .collect();
    assert_eq!(rejected.len(), 9, "{err}");
    for n in [2, 4, 5, 6, 7, 8, 9, 10, 12] {
        assert!(
            err.contains(&format!("rejected document {n} of the input")),
            "line {n}: {err}"
        );
    }
    // strict: the first bad line stops the run and names it; that batch is not sent
    let m2 = RouterMock::builder()
        .route(Route::exact("GET", "/").reply(Reply::ok(info)))
        .route(Route::contains("POST", "/_bulk").reply(Reply::ok(ok_items)))
        .start()
        .await;
    let o = run_async(
        vec![
            "--input".into(),
            input.to_str().unwrap().into(),
            "--output".into(),
            format!("{}/dst", m2.url()),
            "--noRefresh".into(),
            "--ignore-es-write-errors".into(),
            "false".into(),
        ],
        vec![],
    )
    .await;
    let err = stderr(&o);
    assert_eq!(o.status.code(), Some(1), "{err}");
    assert!(err.contains("rejected document 2 of the input"), "{err}");
    assert_eq!(
        m2.requests()
            .iter()
            .filter(|r| r.path.ends_with("/_bulk"))
            .count(),
        0
    );
}

// L-450 (BRK-IN-007): a giant single value stops the run with its position and
// a --maxDocMb hint instead of being buffered whole; nothing of that batch is sent.
#[tokio::test(flavor = "multi_thread")]
async fn giant_documents_are_refused_with_a_size_hint() {
    use support::router_mock::{Reply, Route, RouterMock};
    let info = r#"{"version":{"distribution":"opensearch","number":"2.19.0"}}"#;
    let dir = tempfile::tempdir().unwrap();
    let input = dir.path().join("in.json");
    let big = "x".repeat(3 << 20);
    std::fs::write(
        &input,
        format!(
            "{}\n{{\"_id\":\"big\",\"_source\":{{\"v\":\"{big}\"}}}}\n{}\n",
            r#"{"_id":"g1","_source":{"a":1}}"#, r#"{"_id":"g2","_source":{"a":2}}"#
        ),
    )
    .unwrap();
    let ok = r#"{"took":1,"errors":false,"items":[{"index":{"status":201}},{"index":{"status":201}},{"index":{"status":201}}]}"#;
    let mock = || {
        RouterMock::builder()
            .route(Route::exact("GET", "/").reply(Reply::ok(info)))
            .route(Route::contains("POST", "/_bulk").reply(Reply::ok(ok)))
            .fallback(Reply::ok(
                r#"{"_shards":{"total":1,"successful":1,"failed":0}}"#,
            ))
            .start()
    };
    let m = mock().await;
    let o = run_async(
        vec![
            "--input".into(),
            input.to_str().unwrap().into(),
            "--output".into(),
            format!("{}/dst", m.url()),
            "--maxDocMb".into(),
            "1".into(),
            "--noRefresh".into(),
        ],
        vec![],
    )
    .await;
    let err = stderr(&o);
    assert_eq!(o.status.code(), Some(1), "{err}");
    assert!(err.contains("document 2 of the input"), "{err}");
    assert!(err.contains("larger than 1 MB"), "{err}");
    assert!(err.contains("--maxDocMb"), "{err}");
    let bulks = m
        .requests()
        .iter()
        .filter(|r| r.path.ends_with("/_bulk"))
        .count();
    assert_eq!(bulks, 0, "{err}");
    // with the default cap (100 MB) the same input goes through
    let m2 = mock().await;
    let o = run_async(
        vec![
            "--input".into(),
            input.to_str().unwrap().into(),
            "--output".into(),
            format!("{}/dst", m2.url()),
            "--noRefresh".into(),
        ],
        vec![],
    )
    .await;
    assert_eq!(o.status.code(), Some(0), "{}", stderr(&o));
}

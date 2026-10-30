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


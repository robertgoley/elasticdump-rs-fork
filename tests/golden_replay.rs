//! Golden record/replay (TST-INF-003 infrastructure; TST-API-003 replay of committed goldens).
//!
//! Offline (T1): every case under `tests/golden/<engine>-<version>/<case>/` is replayed through
//! the router mock; the binary's output must equal `expected.out` byte for byte and every
//! request must match the recorded shape. Failures write the actual output under
//! `target/golden-diff/<engine>-<version>/<case>/` for review.

mod support;

use std::path::Path;
use std::process::{Command, Output};

use serde_json::json;
use support::golden::{self, Meta, Normaliser, Recorder, Replay};
use support::router_mock::{Reply, Route, RouterMock};

fn bin(args: &[String]) -> Output {
    Command::new(env!("CARGO_BIN_EXE_elasticdump-rs"))
        .args(args)
        .arg("--quiet")
        .output()
        .expect("spawn elasticdump-rs")
}

async fn run(args: Vec<String>) -> Output {
    tokio::task::spawn_blocking(move || bin(&args))
        .await
        .unwrap()
}

#[test]
fn normaliser_masks_volatile_fields_and_tokenises_ids() {
    let n = Normaliser::default();
    let mut resp = json!({"_scroll_id": "FGluY2x1ZGVfY29udGV4dF91dWlk", "took": 17, "timed_out": false,
        "_shards": {"total": 2, "successful": 2, "skipped": 0, "failed": 0},
        "hits": {"hits": [{"_id": "1", "_source": {"took": "kept: not a number"}}]}});
    n.learn("/idx/_search", &resp);
    n.apply(&mut resp);
    assert_eq!(resp["_scroll_id"], "<scroll:1>");
    assert_eq!(resp["took"], 0);
    assert_eq!(resp["_shards"]["failed"], 0);
    assert_eq!(
        resp["hits"]["hits"][0]["_source"]["took"],
        "kept: not a number"
    );
    // the same id in a later request body becomes the same token
    let mut req = json!({"scroll": "10m", "scroll_id": "FGluY2x1ZGVfY29udGV4dF91dWlk"});
    n.apply(&mut req);
    assert_eq!(req["scroll_id"], "<scroll:1>");
    // ES open-PIT response {"id": ...}; OpenSearch {"pit_id": ..., "creation_time": ...}
    let mut es = json!({"id": "46ToAwMDaWR5"});
    n.learn("/idx/_pit", &es);
    n.apply(&mut es);
    assert_eq!(es["id"], "<pit:1>");
    let mut os = json!({"pit_id": "o463QQEPbG9n", "creation_time": 1730000000000u64});
    n.learn("/idx/_search/point_in_time", &os);
    n.apply(&mut os);
    assert_eq!(os, json!({"pit_id": "<pit:2>", "creation_time": 0}));
    let mut info = json!({"name": "node-7f3a", "cluster_name": "it", "cluster_uuid": "x9", "version": {"number": "2.19.7"}});
    n.apply(&mut info);
    assert_eq!(info["name"], "<node>");
    assert_eq!(info["cluster_uuid"], "<uuid>");
    let mut settings = json!({"idx": {"settings": {"index": {"creation_date": "1730000000000", "uuid": "abc", "provided_name": "idx"}}}});
    n.apply(&mut settings);
    assert_eq!(
        settings["idx"]["settings"]["index"]["creation_date"],
        "<creation_date>"
    );
    assert_eq!(settings["idx"]["settings"]["index"]["provided_name"], "idx");
}

fn hit(id: &str, src: &str) -> String {
    format!(r#"{{"_index":"idx","_id":"{id}","_score":1.0,"_source":{src}}}"#)
}

fn page(sid: &str, hits: &[String]) -> String {
    format!(
        r#"{{"_scroll_id":"{sid}","took":{},"timed_out":false,"_shards":{{"total":1,"successful":1,"skipped":0,"failed":0}},"hits":{{"total":{{"value":3,"relation":"eq"}},"hits":[{}]}}}}"#,
        hits.len() * 7 + 3,
        hits.join(",")
    )
}

/// A fake cluster whose scroll ids and `took` change on every start, like a real one.
async fn fake_cluster(salt: &str) -> RouterMock {
    let sid = format!("DXF1ZXJ5QW5kRmV0Y2gB{salt}AAAAAAAAAAEWb3Ax");
    RouterMock::builder()
        .route(
            Route::contains("DELETE", "/_search/scroll")
                .reply(Reply::ok(r#"{"succeeded":true,"num_freed":1}"#)),
        )
        .route(Route::contains("POST", "/_search/scroll").replies([
            Reply::ok(page(
                &sid,
                &[hit(
                    "3",
                    r#"{"k":"c","big":123456789012345678901234567890}"#,
                )],
            )),
            Reply::ok(page(&sid, &[])),
        ]))
        .route(Route::contains("POST", "/_search").reply(Reply::ok(page(
            &sid,
            &[
                hit("1", r#"{"k":"a","t":"unicode ✓ \"q\""}"#),
                hit("2", r#"{"k":"b"}"#),
            ],
        ))))
        .start()
        .await
}

fn tmpdir(tag: &str) -> tempfile::TempDir {
    tempfile::Builder::new()
        .prefix(&format!("edrs-golden-{tag}-"))
        .tempdir()
        .unwrap()
}

async fn record_case(salt: &str, dir: &Path) -> Vec<u8> {
    std::fs::create_dir_all(dir).unwrap();
    let up = fake_cluster(salt).await;
    let rec = Recorder::start(&up.url()).await;
    let out = dir.join("live.out");
    let args = vec![
        "--input".into(),
        format!("{}/idx", rec.url()),
        "--output".into(),
        out.to_str().unwrap().into(),
        "--limit".into(),
        "2".into(),
    ];
    let o = run(args).await;
    assert!(o.status.success(), "{}", String::from_utf8_lossy(&o.stderr));
    rec.save(&dir.join("case")).unwrap();
    std::fs::read(out).unwrap()
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn record_twice_gives_zero_diff_and_replay_reproduces_output() {
    let t = tmpdir("rt");
    let (a, b) = (t.path().join("a"), t.path().join("b"));
    let out_a = record_case("AAAA", &a).await;
    let out_b = record_case("BBBB", &b).await;
    assert_eq!(out_a, out_b);
    // zero diff after normalisation, although scroll ids and `took` differed
    let files = |d: &Path| {
        let mut v: Vec<_> = std::fs::read_dir(d.join("case"))
            .unwrap()
            .map(|e| e.unwrap().file_name())
            .collect();
        v.sort();
        v
    };
    assert_eq!(files(&a), files(&b));
    assert!(files(&a).len() >= 6, "{:?}", files(&a));
    for f in files(&a) {
        assert_eq!(
            std::fs::read_to_string(a.join("case").join(&f)).unwrap(),
            std::fs::read_to_string(b.join("case").join(&f)).unwrap(),
            "{f:?} differs between recordings"
        );
    }
    let first = std::fs::read_to_string(a.join("case/001.resp.json")).unwrap();
    assert!(
        first.contains("<scroll:1>") && !first.contains("AAAA"),
        "{first}"
    );

    // replay offline: same bytes out, no request-shape problems
    let rp = Replay::start(&a.join("case")).await;
    let out = t.path().join("replay.out");
    let o = run(vec![
        "--input".into(),
        format!("{}/idx", rp.url()),
        "--output".into(),
        out.to_str().unwrap().into(),
        "--limit".into(),
        "2".into(),
    ])
    .await;
    assert!(o.status.success(), "{}", String::from_utf8_lossy(&o.stderr));
    assert_eq!(std::fs::read(&out).unwrap(), out_a);
    assert!(rp.problems().is_empty(), "{:#?}", rp.problems());

    // a changed request shape is reported (here: --limit 3 instead of 2)
    let rp = Replay::start(&a.join("case")).await;
    let out = t.path().join("replay3.out");
    let _ = run(vec![
        "--input".into(),
        format!("{}/idx", rp.url()),
        "--output".into(),
        out.to_str().unwrap().into(),
        "--limit".into(),
        "3".into(),
    ])
    .await;
    let p = rp.problems();
    assert!(
        p.iter()
            .any(|m| m.contains("body differs") && m.contains("\"size\":3")),
        "{p:#?}"
    );
}

/// TST-API-003: every committed golden case replays offline with identical output and request shapes.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn committed_goldens_replay() {
    let root = golden::golden_root();
    let cases = golden::cases(&root);
    let mut failures = vec![];
    for dir in &cases {
        let meta = Meta::read(dir);
        let rel = dir.strip_prefix(&root).unwrap().to_path_buf();
        let rp = Replay::start(dir).await;
        let t = tmpdir("replay");
        let out = t.path().join("out");
        let o = run(meta.expand(&rp.url(), &out, dir)).await;
        let mut problems = rp.problems();
        if o.status.code() != Some(meta.exit_code) {
            problems.push(format!(
                "exit {:?}, recorded {}: {}",
                o.status.code(),
                meta.exit_code,
                String::from_utf8_lossy(&o.stderr)
            ));
        }
        if meta.has_output {
            let want = std::fs::read(dir.join("expected.out")).unwrap_or_default();
            let got = std::fs::read(&out).unwrap_or_default();
            if want != got {
                let d = Path::new(env!("CARGO_MANIFEST_DIR"))
                    .join("target/golden-diff")
                    .join(&rel);
                std::fs::create_dir_all(&d).unwrap();
                std::fs::write(d.join("actual.out"), &got).unwrap();
                problems.push(format!(
                    "output differs from expected.out (actual in {})",
                    d.display()
                ));
            }
        }
        if !problems.is_empty() {
            failures.push(format!("{}:\n  {}", rel.display(), problems.join("\n  ")));
        }
    }
    eprintln!("golden replay: {} cases", cases.len());
    assert!(failures.is_empty(), "{}", failures.join("\n"));
}

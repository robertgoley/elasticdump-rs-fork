//! Golden recording (TST-INF-003). `#[ignore]`d: runs only through
//! `scripts/golden/record.sh <profile>` (`IT_RECORD=1`), which writes
//! `tests/golden/<engine>-<version>/<case>/` (or `$IT_GOLDEN_DIR/...` for a re-record check).
//! Replay of the committed files is `tests/golden_replay.rs` (offline, T1).

use crate::harness::*;
use crate::support::golden::{Meta, Recorder, golden_root};
use serde_json::json;

const SRC: &str = "golden_src";
const DST: &str = "golden_dst";

/// 25 deterministic documents in one shard (one shard keeps the scroll order stable).
async fn seed(c: &Cluster) {
    c.delete_index(SRC).await;
    c.ok(
        "PUT",
        SRC,
        Some(json!({
            "settings": {"number_of_shards": 1, "number_of_replicas": 0},
            "mappings": {"properties": {"k": {"type": "keyword"}, "n": {"type": "long"}, "t": {"type": "text"}, "big": {"type": "keyword"}}}
        })),
    )
    .await;
    let mut nd = String::new();
    for i in 0..25 {
        nd.push_str(&json!({"index": {"_index": SRC, "_id": format!("g{i:02}")}}).to_string());
        nd.push('\n');
        nd.push_str(
            &json!({"k": format!("key{i}"), "n": i, "t": format!("golden doc {i}")}).to_string(),
        );
        nd.push('\n');
    }
    nd.push_str(&json!({"index": {"_index": SRC, "_id": "adv"}}).to_string());
    nd.push('\n');
    nd.push_str(r#"{"k":"unicode-✓-漢字","t":"line\nbreak \"q\"","n":1,"big":123456789012345678901234567890}"#);
    nd.push('\n');
    c.bulk_raw(nd).await;
    c.ok("POST", &format!("{SRC}/_refresh"), None).await;
}

const RESTORE_INPUT: &str = concat!(
    r#"{"_index":"golden_dst","_id":"r1","_source":{"k":"a","n":1}}"#,
    "\n",
    r#"{"_index":"golden_dst","_id":"r2","_source":{"k":"b","n":2}}"#,
    "\n",
    r#"{"_index":"golden_dst","_id":"r3","_source":{"k":"c","n":3,"t":"unicode ✓"}}"#,
    "\n",
    r#"{"_index":"golden_dst","_id":"r4","_source":{"k":"d","n":4}}"#,
    "\n",
);

struct Case {
    name: &'static str,
    args: &'static [&'static str],
    output: bool,
}

const CASES: &[Case] = &[
    Case {
        name: "data-scroll",
        args: &[
            "--input",
            "{{URL}}/golden_src",
            "--output",
            "{{OUT}}",
            "--limit",
            "7",
        ],
        output: true,
    },
    Case {
        name: "mapping",
        args: &[
            "--input",
            "{{URL}}/golden_src",
            "--output",
            "{{OUT}}",
            "--type",
            "mapping",
        ],
        output: true,
    },
    Case {
        name: "settings",
        args: &[
            "--input",
            "{{URL}}/golden_src",
            "--output",
            "{{OUT}}",
            "--type",
            "settings",
        ],
        output: true,
    },
    Case {
        name: "data-restore",
        args: &[
            "--input",
            "{{DIR}}/input.ndjson",
            "--output",
            "{{URL}}/golden_dst",
            "--limit",
            "3",
        ],
        output: false,
    },
];

#[tokio::test]
#[ignore = "recording only: scripts/golden/record.sh (IT_RECORD=1)"]
async fn record() {
    if env("IT_RECORD").as_deref() != Some("1") {
        eprintln!("SKIP: golden::record needs IT_RECORD=1");
        return;
    }
    let c = Cluster::from_env();
    let version = c.version().await;
    let engine = match c.engine {
        Engine::OpenSearch => "opensearch",
        Engine::Elasticsearch if c.is_oss().await => "elasticsearch-oss",
        Engine::Elasticsearch => "elasticsearch",
    };
    let root = golden_root().join(format!("{engine}-{version}"));
    seed(&c).await;
    for case in CASES {
        let dir = root.join(case.name);
        std::fs::create_dir_all(&dir).unwrap();
        if case.name == "data-restore" {
            c.delete_index(DST).await;
            std::fs::write(dir.join("input.ndjson"), RESTORE_INPUT).unwrap();
        }
        let rec = Recorder::start(&c.url_with_auth("")).await;
        let out = tmp_path(&format!("golden-{}", case.name), "out");
        let meta = Meta {
            case: case.name.into(),
            engine: engine.into(),
            version: version.clone(),
            image: env("IT_IMAGE").unwrap_or_default(),
            args: case.args.iter().map(|s| s.to_string()).collect(),
            exit_code: 0,
            has_output: case.output,
        };
        let o = run(meta.expand(&rec.url(), &out, &dir)).await;
        assert_success(&o);
        let n = rec.save(&dir).unwrap();
        Meta {
            exit_code: o.status.code().unwrap_or(-1),
            ..meta
        }
        .write(&dir)
        .unwrap();
        if case.output {
            std::fs::copy(&out, dir.join("expected.out")).unwrap();
            let _ = std::fs::remove_file(&out);
        }
        eprintln!(
            "golden: {}/{} recorded ({n} exchanges)",
            root.display(),
            case.name
        );
    }
    c.delete_index(SRC).await;
    c.delete_index(DST).await;
}

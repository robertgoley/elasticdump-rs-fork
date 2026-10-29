//! Cross-engine copies (OpenSearch <-> Elasticsearch).
//!
//! * Cluster to cluster needs both engines up at once: only the `cross`
//!   profile (`IT_ES_URL` = OpenSearch, `IT_PEER_URL` = Elasticsearch) runs it,
//!   and `run.sh` skips that profile when the box has too little free memory.
//! * File dump/restore runs one container at a time: every plain profile
//!   exports a dump plus the expected result to `$IT_CROSS_DIR/<engine>/`
//!   and restores whatever the *other* engine exported earlier
//!   (`run.sh all` brings OpenSearch back once at the end for the last leg).

use std::collections::BTreeMap;
use std::path::PathBuf;

use serde_json::{Value, json};

use crate::harness::*;

fn pair() -> Option<(Cluster, Cluster)> {
    let peer = Cluster::peer()?;
    Some((Cluster::from_env(), peer))
}

fn both_directions() -> Option<Vec<(Cluster, Cluster)>> {
    let (a, b) = pair()?;
    Some(vec![(a.clone(), b.clone()), (b, a)])
}

fn name(c: &Cluster) -> &'static str {
    match c.engine {
        Engine::OpenSearch => "opensearch",
        Engine::Elasticsearch => "elasticsearch",
    }
}

async fn shape(c: &Cluster, index: &str) -> (Value, Value) {
    let st = c.ok("GET", &format!("{index}/_settings"), None).await;
    let i = &st[index]["settings"]["index"];
    let m = c.ok("GET", &format!("{index}/_mapping"), None).await;
    (
        json!({"shards": i["number_of_shards"], "replicas": i["number_of_replicas"], "analysis": i["analysis"]}),
        m[index]["mappings"].clone(),
    )
}

async fn green(c: &Cluster, index: &str) {
    c.ok(
        "GET",
        &format!("_cluster/health/{index}?wait_for_status=green&timeout=20s"),
        None,
    )
    .await;
}

#[tokio::test]
async fn cluster_to_cluster_full_copy_both_directions() {
    let Some(dirs) = both_directions() else {
        eprintln!("SKIP: no peer cluster (cross profile only)");
        return;
    };
    for (src_c, dst_c) in dirs {
        let label = format!("{}->{}", name(&src_c), name(&dst_c));
        let src = unique("x_src");
        let dst = unique("x_dst");
        seed_index(&src_c, &src, 1200).await;
        dst_c.delete_index(&dst).await;
        for ty in ["settings", "mapping", "data"] {
            let o = run_loud(s(&[
                "--input",
                &src_c.url_with_auth(&src),
                "--output",
                &dst_c.url_with_auth(&dst),
                "--type",
                ty,
                "--limit",
                "500",
            ]))
            .await;
            assert_success(&o);
            let err = String::from_utf8_lossy(&o.stderr);
            if ty == "settings" {
                // the source engine's own keys are dropped with a warning
                let dropped = match src_c.engine {
                    Engine::Elasticsearch => "_tier_preference",
                    Engine::OpenSearch => "replication.type",
                };
                assert!(
                    err.contains("Dropping") && err.contains(dropped),
                    "{label}: {err}"
                );
            }
        }
        green(&dst_c, &dst).await;
        assert_eq!(
            shape(&src_c, &src).await,
            shape(&dst_c, &dst).await,
            "{label}"
        );
        assert_eq!(src_c.docs(&src).await, dst_c.docs(&dst).await, "{label}");
        src_c.delete_index(&src).await;
        dst_c.delete_index(&dst).await;
    }
}

type Docs = BTreeMap<String, (Option<String>, Value)>;

const TYPES: [&str; 3] = ["settings", "mapping", "data"];

fn cross_base() -> Option<PathBuf> {
    // plain profiles only: the secured ones exercise the same code paths
    if is_secure() {
        return None;
    }
    std::env::var("IT_CROSS_DIR").ok().map(PathBuf::from)
}

/// Exports of one engine, one directory per major version
/// (`elasticsearch-8`, `elasticsearch-9`, `opensearch-2`, ...).
fn exports_of(e: Engine) -> Vec<PathBuf> {
    let Some(base) = cross_base() else {
        return Vec::new();
    };
    let prefix = format!("{}-", engine_name(e));
    let mut v: Vec<PathBuf> = std::fs::read_dir(base)
        .into_iter()
        .flatten()
        .flatten()
        .filter(|d| d.file_name().to_string_lossy().starts_with(&prefix))
        .map(|d| d.path())
        .filter(|p| p.join("expected.json").exists())
        .collect();
    v.sort();
    v
}

fn engine_name(e: Engine) -> &'static str {
    match e {
        Engine::OpenSearch => "opensearch",
        Engine::Elasticsearch => "elasticsearch",
    }
}

fn other(e: Engine) -> Engine {
    match e {
        Engine::OpenSearch => Engine::Elasticsearch,
        Engine::Elasticsearch => Engine::OpenSearch,
    }
}

/// Settings key the given engine adds by itself and the other one lacks.
fn own_key(e: Engine) -> &'static str {
    match e {
        Engine::Elasticsearch => "index.routing.allocation.include._tier_preference",
        Engine::OpenSearch => "index.replication.type",
    }
}

#[tokio::test]
async fn file_export_for_the_other_engine() {
    let c = Cluster::from_env();
    let Some(base) = cross_base() else {
        eprintln!("SKIP: IT_CROSS_DIR not set or secured profile");
        return;
    };
    let dir = base.join(format!("{}-{}", engine_name(c.engine), c.major().await));
    let _ = std::fs::remove_dir_all(&dir);
    std::fs::create_dir_all(&dir).unwrap();
    let src = unique("xf_src");
    seed_index(&c, &src, 800).await;
    for ty in TYPES {
        let f = dir.join(format!("{ty}.json.zst"));
        assert_success(
            &run(s(&[
                "--input",
                &c.url_with_auth(&src),
                "--output",
                f.to_str().unwrap(),
                "--type",
                ty,
                "--compressionType",
                "zstd",
            ]))
            .await,
        );
    }
    let (settings, mapping) = shape(&c, &src).await;
    let expected = json!({
        "version": c.version().await,
        "settings": settings,
        "mapping": mapping,
        "docs": c.docs(&src).await,
    });
    std::fs::write(dir.join("expected.json"), expected.to_string()).unwrap();
    c.delete_index(&src).await;
}

#[tokio::test]
async fn file_dump_from_the_other_engine_restores_here() {
    let c = Cluster::from_env();
    let from = other(c.engine);
    let dirs = exports_of(from);
    if dirs.is_empty() {
        eprintln!(
            "SKIP: no {} export yet (or IT_CROSS_DIR unset)",
            engine_name(from)
        );
        return;
    }
    for dir in dirs {
        restore_export(&c, from, &dir).await;
    }
}

async fn restore_export(c: &Cluster, from: Engine, dir: &std::path::Path) {
    let raw = std::fs::read_to_string(dir.join("expected.json")).unwrap();
    let label = format!(
        "{}->{}",
        dir.file_name().unwrap().to_string_lossy(),
        engine_name(c.engine)
    );
    let expected: Value = serde_json::from_str(&raw).unwrap();
    eprintln!("{label}: dump written by version {}", expected["version"]);
    let settings_file = dir.join("settings.json.zst");
    let foreign = own_key(from);

    // 1. without --excludeSettings the foreign key is reported: OpenSearch keys
    //    are rejected by Elasticsearch, Elasticsearch keys are stored silently
    //    by OpenSearch, so the tool warns before sending them
    let probe = unique("xf_probe");
    c.delete_index(&probe).await;
    let o = run_loud(s(&[
        "--input",
        settings_file.to_str().unwrap(),
        "--output",
        &c.url_with_auth(&probe),
        "--type",
        "settings",
    ]))
    .await;
    let err = String::from_utf8_lossy(&o.stderr).to_string();
    let short = foreign.rsplit('.').next().unwrap();
    assert!(
        err.contains("--excludeSettings") && err.contains(short),
        "{label}: {err}"
    );
    match c.engine {
        Engine::Elasticsearch => assert!(!o.status.success(), "{label}: {err}"),
        Engine::OpenSearch => {
            assert_success(&o);
            assert!(err.contains("Elasticsearch-only"), "{label}: {err}");
        }
    }
    c.delete_index(&probe).await;

    // 2. full restore with the foreign key excluded
    let dst = unique("xf_dst");
    c.delete_index(&dst).await;
    for ty in TYPES {
        let f = dir.join(format!("{ty}.json.zst"));
        let mut a = s(&[
            "--input",
            f.to_str().unwrap(),
            "--output",
            &c.url_with_auth(&dst),
            "--type",
            ty,
        ]);
        if ty == "settings" {
            a.extend(s(&["--excludeSettings", foreign]));
        }
        let o = run_loud(a).await;
        assert_success(&o);
        let err = String::from_utf8_lossy(&o.stderr);
        assert!(!err.contains("-only keys"), "{label}: {err}");
    }
    green(c, &dst).await;
    let (settings, mapping) = shape(c, &dst).await;
    assert_eq!(settings, expected["settings"], "{label}");
    assert_eq!(mapping, expected["mapping"], "{label}");
    let want: Docs = serde_json::from_value(expected["docs"].clone()).unwrap();
    assert_eq!(c.docs(&dst).await, want, "{label}");
    c.delete_index(&dst).await;
}

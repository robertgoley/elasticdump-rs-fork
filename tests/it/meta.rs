//! M3: `--type mapping` and `--type settings`, dump and restore.

use std::path::{Path, PathBuf};

use serde_json::{Value, json};

use crate::harness::*;

fn io_args(c: &Cluster, input: &str, output: &str, ty: &str) -> Vec<String> {
    let mut a = s(&["--input", input, "--output", output, "--type", ty]);
    if input.starts_with("http") {
        a.extend(c.tls_args("input"));
    }
    if output.starts_with("http") {
        a.extend(c.tls_args("output"));
    }
    a
}

async fn dump(c: &Cluster, index: &str, ty: &str) -> PathBuf {
    let out = tmp_path(ty, "json");
    assert_success(
        &run(io_args(
            c,
            &c.url_with_auth(index),
            out.to_str().unwrap(),
            ty,
        ))
        .await,
    );
    out
}

async fn restore(
    c: &Cluster,
    file: &Path,
    target: &str,
    ty: &str,
    extra: &[&str],
) -> std::process::Output {
    let mut a = io_args(c, file.to_str().unwrap(), &c.url_with_auth(target), ty);
    a.extend(s(extra));
    run(a).await
}

/// The user-visible parts of an index's settings.
async fn shape(c: &Cluster, index: &str) -> (Value, Value) {
    let st = c.ok("GET", &format!("{index}/_settings"), None).await;
    let i = &st[index]["settings"]["index"];
    let settings = json!({
        "number_of_shards": i["number_of_shards"],
        "number_of_replicas": i["number_of_replicas"],
        "analysis": i["analysis"],
        "refresh_interval": i["refresh_interval"],
    });
    let m = c.ok("GET", &format!("{index}/_mapping"), None).await;
    (settings, m[index]["mappings"].clone())
}

async fn seed_with_settings(c: &Cluster, index: &str, n: usize) {
    seed_index(c, index, n).await;
    c.ok(
        "PUT",
        &format!("{index}/_settings"),
        Some(json!({"index": {"refresh_interval": "7s"}})),
    )
    .await;
}

#[tokio::test]
async fn full_restore_settings_mapping_data() {
    let c = Cluster::from_env();
    let src = unique("meta_src");
    let dst = unique("meta_dst");
    seed_with_settings(&c, &src, 1500).await;

    let settings = dump(&c, &src, "settings").await;
    let mapping = dump(&c, &src, "mapping").await;
    let data = dump(&c, &src, "data").await;

    // settings file: one JSON string literal line, generated keys removed
    let l = lines(&settings);
    assert_eq!(l.len(), 1);
    let inner: Value = serde_json::from_str(
        serde_json::from_str::<Value>(&l[0])
            .unwrap()
            .as_str()
            .unwrap(),
    )
    .unwrap();
    let idx = &inner[&src]["settings"]["index"];
    for k in ["uuid", "creation_date", "provided_name", "version"] {
        assert!(idx.get(k).is_none(), "{k} should be excluded: {idx}");
    }
    assert_eq!(lines(&mapping).len(), 1);

    c.delete_index(&dst).await;
    for (f, ty) in [
        (&settings, "settings"),
        (&mapping, "mapping"),
        (&data, "data"),
    ] {
        let o = restore(&c, f, &dst, ty, &[]).await;
        assert_success(&o);
    }
    assert_eq!(shape(&c, &src).await, shape(&c, &dst).await);
    assert_eq!(c.docs(&src).await, c.docs(&dst).await);

    // the custom analyzer is live on the target
    let a = c
        .ok(
            "POST",
            &format!("{dst}/_analyze"),
            Some(json!({"analyzer": "content", "text": "Foo BAR"})),
        )
        .await;
    let toks: Vec<&str> = a["tokens"]
        .as_array()
        .unwrap()
        .iter()
        .map(|t| t["token"].as_str().unwrap())
        .collect();
    assert_eq!(toks, ["foo", "bar"]);

    // settings on an existing index fail without --skip-existing ...
    let o = restore(&c, &settings, &dst, "settings", &[]).await;
    assert!(!o.status.success());
    assert!(String::from_utf8_lossy(&o.stderr).contains("--skip-existing"));
    // ... and are skipped with it; mapping restore is idempotent
    assert_success(&restore(&c, &settings, &dst, "settings", &["--skip-existing"]).await);
    assert_success(&restore(&c, &mapping, &dst, "mapping", &[]).await);
    c.delete_index(&src).await;
    c.delete_index(&dst).await;
}

#[tokio::test]
async fn cluster_to_cluster_meta_and_exclude_settings() {
    let c = Cluster::from_env();
    let src = unique("meta_c2c_src");
    let dst = unique("meta_c2c_dst");
    seed_with_settings(&c, &src, 10).await;
    c.delete_index(&dst).await;
    let mut a = io_args(
        &c,
        &c.url_with_auth(&src),
        &c.url_with_auth(&dst),
        "settings",
    );
    a.extend(s(&["--excludeSettings", "index.refresh_interval"]));
    assert_success(&run(a).await);
    assert_success(
        &run(io_args(
            &c,
            &c.url_with_auth(&src),
            &c.url_with_auth(&dst),
            "mapping",
        ))
        .await,
    );
    let (ss, sm) = shape(&c, &src).await;
    let (ds, dm) = shape(&c, &dst).await;
    assert_eq!(sm, dm);
    assert_eq!(ss["analysis"], ds["analysis"]);
    assert_eq!(ss["number_of_shards"], ds["number_of_shards"]);
    assert_eq!(ds["refresh_interval"], Value::Null, "excluded");
    c.delete_index(&src).await;
    c.delete_index(&dst).await;
}

#[tokio::test]
async fn multi_index_host_only_restore_with_prefix() {
    let c = Cluster::from_env();
    let a_idx = unique("meta_ma");
    let b_idx = unique("meta_mb");
    seed_index(&c, &a_idx, 3).await;
    seed_index(&c, &b_idx, 3).await;
    let list = format!("{a_idx},{b_idx}");
    let settings = dump(&c, &list, "settings").await;
    let mapping = dump(&c, &list, "mapping").await;

    // explicit single output index + multiple indices is an error
    let o = restore(&c, &mapping, "whatever", "mapping", &[]).await;
    assert!(!o.status.success());
    assert!(String::from_utf8_lossy(&o.stderr).contains("host-only"));

    for (f, ty) in [(&settings, "settings"), (&mapping, "mapping")] {
        let mut a = io_args(&c, f.to_str().unwrap(), &c.url_with_auth(""), ty);
        a.extend(s(&["--prefix", "cp_"]));
        assert_success(&run(a).await);
    }
    for i in [&a_idx, &b_idx] {
        let (ss, sm) = shape(&c, i).await;
        let (ds, dm) = shape(&c, &format!("cp_{i}")).await;
        assert_eq!(sm, dm);
        assert_eq!(ss["analysis"], ds["analysis"]);
        c.delete_index(i).await;
        c.delete_index(&format!("cp_{i}")).await;
    }
}

#[tokio::test]
async fn typed_mapping_file_is_unwrapped() {
    let c = Cluster::from_env();
    let dst = unique("meta_typed");
    c.delete_index(&dst).await;
    let f = tmp_path("typed", "json");
    std::fs::write(
        &f,
        json!({"old": {"mappings": {"doc": {"_all": {"enabled": false}, "properties": {
            "a": {"type": "text", "include_in_all": false},
            "properties": {"type": "keyword"}
        }}}}})
        .to_string(),
    )
    .unwrap();
    assert_success(&restore(&c, &f, &dst, "mapping", &[]).await);
    let (_, m) = shape(&c, &dst).await;
    assert_eq!(m["properties"]["a"]["type"], "text");
    assert_eq!(m["properties"]["properties"]["type"], "keyword");
    c.delete_index(&dst).await;
}

fn node_elasticdump() -> Option<&'static str> {
    let p = "/tmp/ed/node_modules/.bin/elasticdump";
    (Path::new(p).exists() && Path::new("/usr/bin/node").exists()).then_some(p)
}

async fn run_node(args: Vec<String>) -> std::process::Output {
    let mut cmd = tokio::process::Command::new(node_elasticdump().unwrap());
    cmd.args(&args).env("NODE_TLS_REJECT_UNAUTHORIZED", "0");
    cmd.output().await.unwrap()
}

#[tokio::test]
async fn node_interop_settings_and_mapping() {
    if node_elasticdump().is_none() {
        eprintln!("SKIP: node elasticdump not installed");
        return;
    }
    let c = Cluster::from_env();
    let src = unique("meta_node_src");
    seed_with_settings(&c, &src, 5).await;

    let mut node_files = Vec::new();
    let mut rust_files = Vec::new();
    for ty in ["settings", "mapping"] {
        let nf = tmp_path(&format!("node_{ty}"), "json");
        let o = run_node(s(&[
            "--input",
            &c.url_with_auth(&src),
            "--output",
            nf.to_str().unwrap(),
            "--type",
            ty,
        ]))
        .await;
        assert!(
            o.status.success(),
            "node dump: {}",
            String::from_utf8_lossy(&o.stderr)
        );
        let rf = dump(&c, &src, ty).await;
        // same file format: settings are a string literal, mapping an object
        let nv: Value = serde_json::from_str(&lines(&nf)[0]).unwrap();
        let rv: Value = serde_json::from_str(&lines(&rf)[0]).unwrap();
        assert_eq!(lines(&nf).len(), 1);
        assert_eq!(nv.is_string(), rv.is_string(), "{ty}");
        assert_eq!(rv.is_string(), ty == "settings");
        if ty == "mapping" {
            assert_eq!(nv, rv);
        }
        node_files.push((nf, ty));
        rust_files.push((rf, ty));
    }

    // Node files -> Rust restore
    let rdst = unique("meta_node_r");
    c.delete_index(&rdst).await;
    for (f, ty) in &node_files {
        assert_success(&restore(&c, f, &rdst, ty, &[]).await);
    }
    // Rust files -> Node restore
    let ndst = unique("meta_node_n");
    c.delete_index(&ndst).await;
    for (f, ty) in &rust_files {
        let o = run_node(s(&[
            "--input",
            f.to_str().unwrap(),
            "--output",
            &c.url_with_auth(&ndst),
            "--type",
            ty,
        ]))
        .await;
        assert!(
            o.status.success(),
            "node restore: {}",
            String::from_utf8_lossy(&o.stderr)
        );
    }
    let want = shape(&c, &src).await;
    for d in [&rdst, &ndst] {
        assert_eq!(want, shape(&c, d).await, "{d}");
        c.delete_index(d).await;
    }
    c.delete_index(&src).await;
}

//! M4: gzip output/input and split dumps (`--fileSize`, `--maxRows`).

use std::path::{Path, PathBuf};

use serde_json::json;

use crate::harness::*;

fn io_args(c: &Cluster, input: &str, output: &str) -> Vec<String> {
    let mut a = s(&["--input", input, "--output", output]);
    if input.starts_with("http") {
        a.extend(c.tls_args("input"));
    }
    if output.starts_with("http") {
        a.extend(c.tls_args("output"));
    }
    a
}

fn tmp_dir(label: &str) -> PathBuf {
    let d = tmp_path(label, "d");
    std::fs::create_dir_all(&d).unwrap();
    d
}

fn read_any(p: &Path) -> String {
    let raw = std::fs::read(p).unwrap();
    if raw.starts_with(&[0x28, 0xb5, 0x2f, 0xfd]) {
        return String::from_utf8(zstd::decode_all(&raw[..]).unwrap()).unwrap();
    }
    if raw.starts_with(&[0x1f, 0x8b]) {
        let mut s = String::new();
        std::io::Read::read_to_string(&mut flate2::read::MultiGzDecoder::new(&raw[..]), &mut s)
            .unwrap();
        s
    } else {
        String::from_utf8(raw).unwrap()
    }
}

fn names(dir: &Path) -> Vec<String> {
    let mut v: Vec<String> = std::fs::read_dir(dir)
        .unwrap()
        .map(|e| e.unwrap().file_name().to_string_lossy().into_owned())
        .collect();
    v.sort_by_key(|n| {
        let i = n.find(".split-").map(|i| i + 7).unwrap_or(0);
        n[i..]
            .split('.')
            .next()
            .unwrap_or("")
            .parse::<u64>()
            .unwrap_or(0)
    });
    v
}

/// ASCII-only docs so Node's UTF-16 length equals our byte length.
async fn seed_ascii(c: &Cluster, index: &str, n: usize) {
    c.delete_index(index).await;
    c.ok(
        "PUT",
        index,
        Some(json!({"settings": {"number_of_shards": 1, "number_of_replicas": 0}})),
    )
    .await;
    let mut nd = String::new();
    for i in 0..n {
        nd.push_str(&json!({"index": {"_index": index, "_id": format!("{i:06}")}}).to_string());
        nd.push('\n');
        nd.push_str(&json!({"k": format!("key{i}"), "pad": "x".repeat(40 + i % 50)}).to_string());
        nd.push('\n');
    }
    c.bulk_raw(nd).await;
}

async fn restore_parts(c: &Cluster, dir: &Path, dst: &str) {
    for n in names(dir) {
        let p = dir.join(&n);
        assert_success(&run(io_args(c, p.to_str().unwrap(), &c.url_with_auth(dst))).await);
    }
}

#[tokio::test]
async fn file_size_split_with_and_without_gzip() {
    let c = Cluster::from_env();
    let src = unique("split_src");
    seed_index(&c, &src, 3000).await;
    for gz in [false, true] {
        let dir = tmp_dir("split");
        let root = dir.join("out.json");
        let mut a = io_args(&c, &c.url_with_auth(&src), root.to_str().unwrap());
        a.extend(s(&["--fileSize", "64kb"]));
        if gz {
            a.push("--fsCompress".into());
        }
        assert_success(&run(a).await);
        let parts = names(&dir);
        assert!(parts.len() > 3, "{parts:?}");
        assert!(!root.exists());
        for (n, name) in parts.iter().enumerate() {
            assert_eq!(
                name,
                &format!("out.split-{n}.json{}", if gz { ".gz" } else { "" })
            );
            let body = read_any(&dir.join(name));
            let lens: Vec<usize> = body.lines().map(str::len).collect();
            let total: usize = lens.iter().sum();
            if n + 1 < parts.len() {
                let last = *lens.last().unwrap();
                assert!(total >= 65536 && total - last < 65536, "part {n}: {total}");
            }
        }
        let dst = unique("split_dst");
        c.copy_shape(&src, &dst).await;
        restore_parts(&c, &dir, &dst).await;
        assert_eq!(c.docs(&src).await, c.docs(&dst).await, "gz={gz}");
        c.delete_index(&dst).await;

        // a second run refuses to clobber existing parts without --overwrite
        let mut a = io_args(&c, &c.url_with_auth(&src), root.to_str().unwrap());
        a.extend(s(&["--fileSize", "64kb"]));
        if gz {
            a.push("--fsCompress".into());
        }
        let o = run(a.clone()).await;
        assert!(!o.status.success());
        a.push("--overwrite".into());
        assert_success(&run(a).await);
        assert_eq!(names(&dir), parts);
    }
    c.delete_index(&src).await;
}

#[tokio::test]
async fn max_rows_split_and_gzip_single_file() {
    let c = Cluster::from_env();
    let src = unique("rows_src");
    seed_index(&c, &src, 1000).await;
    let dir = tmp_dir("rows");
    let root = dir.join("dump.ndjson");
    let mut a = io_args(&c, &c.url_with_auth(&src), root.to_str().unwrap());
    a.extend(s(&["--maxRows", "300"]));
    assert_success(&run(a).await);
    let parts = names(&dir);
    // 1004 docs (1000 + 4 adversarial)
    assert_eq!(parts.len(), 4, "{parts:?}");
    for (n, p) in parts.iter().enumerate() {
        let rows = read_any(&dir.join(p)).lines().count();
        assert_eq!(rows, if n < 3 { 300 } else { 104 });
    }

    // single gzip file, read back without any flag (magic detection)
    let gz = tmp_path("single", "json.gz");
    let mut a = io_args(&c, &c.url_with_auth(&src), gz.to_str().unwrap());
    a.extend(s(&["--fsCompress", "--compressionLevel", "9"]));
    assert_success(&run(a).await);
    assert_eq!(&std::fs::read(&gz).unwrap()[..2], &[0x1f, 0x8b]);
    let dst = unique("rows_dst");
    c.copy_shape(&src, &dst).await;
    assert_success(&run(io_args(&c, gz.to_str().unwrap(), &c.url_with_auth(&dst))).await);
    assert_eq!(c.docs(&src).await, c.docs(&dst).await);
    c.delete_index(&src).await;
    c.delete_index(&dst).await;
}

#[tokio::test]
async fn zstd_round_trip_single_and_split() {
    let c = Cluster::from_env();
    let src = unique("zstd_src");
    seed_index(&c, &src, 3000).await;

    // single file, explicit level; restored with no compression flags
    let f = tmp_path("zstd", "json.zst");
    let mut a = io_args(&c, &c.url_with_auth(&src), f.to_str().unwrap());
    a.extend(s(&[
        "--compressionType",
        "zstd",
        "--compressionLevel",
        "19",
    ]));
    assert_success(&run(a).await);
    let raw = std::fs::read(&f).unwrap();
    assert!(raw.starts_with(&[0x28, 0xb5, 0x2f, 0xfd]), "zstd magic");
    let plain = read_any(&f);
    assert!(
        raw.len() * 4 < plain.len(),
        "{} vs {}",
        raw.len(),
        plain.len()
    );
    let dst = unique("zstd_dst");
    c.copy_shape(&src, &dst).await;
    assert_success(&run(io_args(&c, f.to_str().unwrap(), &c.url_with_auth(&dst))).await);
    assert_eq!(c.docs(&src).await, c.docs(&dst).await);
    c.delete_index(&dst).await;

    // split: every part is a complete zstd stream named *.zst
    let dir = tmp_dir("zsplit");
    let root = dir.join("out.json");
    let mut a = io_args(&c, &c.url_with_auth(&src), root.to_str().unwrap());
    a.extend(s(&["--compressionType", "zstd", "--fileSize", "64kb"]));
    assert_success(&run(a).await);
    let parts = names(&dir);
    assert!(parts.len() > 3, "{parts:?}");
    for (n, name) in parts.iter().enumerate() {
        assert_eq!(name, &format!("out.split-{n}.json.zst"));
        // decodes on its own
        let body = read_any(&dir.join(name));
        assert!(body.ends_with('\n'));
    }
    let dst = unique("zsplit_dst");
    c.copy_shape(&src, &dst).await;
    restore_parts(&c, &dir, &dst).await;
    assert_eq!(c.docs(&src).await, c.docs(&dst).await);
    c.delete_index(&dst).await;

    // zstd mapping + settings files restore too
    let st = tmp_path("zst_settings", "json.zst");
    let mp = tmp_path("zst_mapping", "json.zst");
    for (p, ty) in [(&st, "settings"), (&mp, "mapping")] {
        let mut a = io_args(&c, &c.url_with_auth(&src), p.to_str().unwrap());
        a.extend(s(&["--type", ty, "--compressionType", "zstd"]));
        assert_success(&run(a).await);
    }
    let dst = unique("zmeta_dst");
    c.delete_index(&dst).await;
    for (p, ty) in [(&st, "settings"), (&mp, "mapping")] {
        let mut a = io_args(&c, p.to_str().unwrap(), &c.url_with_auth(&dst));
        a.extend(s(&["--type", ty]));
        assert_success(&run(a).await);
    }
    let m1 = c.ok("GET", &format!("{src}/_mapping"), None).await;
    let m2 = c.ok("GET", &format!("{dst}/_mapping"), None).await;
    assert_eq!(m1[&src]["mappings"], m2[&dst]["mappings"]);
    c.delete_index(&dst).await;
    c.delete_index(&src).await;
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
async fn node_interop_split_names_and_gzip() {
    if node_elasticdump().is_none() {
        eprintln!("SKIP: node elasticdump not installed");
        return;
    }
    let c = Cluster::from_env();
    let src = unique("nsplit_src");
    seed_ascii(&c, &src, 3000).await;

    // identical part names for the same dump
    let rdir = tmp_dir("rsplit");
    let ndir = tmp_dir("nsplit");
    let mut a = io_args(
        &c,
        &c.url_with_auth(&src),
        rdir.join("out.json").to_str().unwrap(),
    );
    a.extend(s(&["--fileSize", "64kb", "--fsCompress"]));
    assert_success(&run(a).await);
    let o = run_node(s(&[
        "--input",
        &c.url_with_auth(&src),
        "--output",
        ndir.join("out.json").to_str().unwrap(),
        "--fileSize",
        "64kb",
        "--fsCompress",
    ]))
    .await;
    assert!(
        o.status.success(),
        "node: {}",
        String::from_utf8_lossy(&o.stderr)
    );
    assert_eq!(names(&rdir), names(&ndir));

    // Rust reads Node's .gz parts
    let dst = unique("nsplit_r");
    c.copy_shape(&src, &dst).await;
    restore_parts(&c, &ndir, &dst).await;
    assert_eq!(c.count(&src).await, c.count(&dst).await);
    assert_eq!(c.docs(&src).await, c.docs(&dst).await);
    c.delete_index(&dst).await;

    // Node reads Rust's .gz parts
    let dst = unique("nsplit_n");
    c.copy_shape(&src, &dst).await;
    for n in names(&rdir) {
        let o = run_node(s(&[
            "--input",
            rdir.join(&n).to_str().unwrap(),
            "--output",
            &c.url_with_auth(&dst),
            "--fsCompress",
        ]))
        .await;
        assert!(
            o.status.success(),
            "node restore: {}",
            String::from_utf8_lossy(&o.stderr)
        );
    }
    assert_eq!(c.docs(&src).await, c.docs(&dst).await);
    c.delete_index(&dst).await;
    c.delete_index(&src).await;
}

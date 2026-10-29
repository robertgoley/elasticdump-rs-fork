//! TST-INF-005 acceptance at test scale: 1 changed byte, 1 missing doc and
//! 1 duplicate doc are each detected and named by `diff`.
use std::io::Write;
use std::process::Command;

fn write(path: &std::path::Path, lines: &[String]) {
    let mut f = std::io::BufWriter::new(std::fs::File::create(path).unwrap());
    for l in lines {
        writeln!(f, "{l}").unwrap();
    }
}

fn corpus(n: usize) -> Vec<String> {
    (0..n)
        .map(|i| format!(r#"{{"_index":"src","_id":"doc-{i}","_source":{{"n":{i},"msg":"hello world {i}","tags":["a","b"]}}}}"#))
        .collect()
}

fn diff(a: &std::path::Path, b: &std::path::Path) -> (i32, String) {
    let o = Command::new(env!("CARGO_BIN_EXE_edverify"))
        .args(["diff", "--a"])
        .arg(a)
        .arg("--b")
        .arg(b)
        .output()
        .unwrap();
    (
        o.status.code().unwrap(),
        String::from_utf8_lossy(&o.stdout).into_owned(),
    )
}

#[test]
fn detects_byte_missing_duplicate_and_split_sets() {
    let dir = tempfile::tempdir().unwrap();
    let base = corpus(200_000);
    let a = dir.path().join("a.ndjson");
    write(&a, &base);

    // identical content, different order and split in two parts
    let mut rev = base.clone();
    rev.reverse();
    let (p1, p2) = (dir.path().join("p1.ndjson"), dir.path().join("p2.ndjson"));
    write(&p1, &rev[..70_000]);
    write(&p2, &rev[70_000..]);
    let o = Command::new(env!("CARGO_BIN_EXE_edverify"))
        .args(["diff", "--a"])
        .arg(&a)
        .arg("--b")
        .arg(&p1)
        .arg(&p2)
        .output()
        .unwrap();
    assert_eq!(
        o.status.code(),
        Some(0),
        "{}",
        String::from_utf8_lossy(&o.stdout)
    );

    // 1 changed byte
    let mut m = base.clone();
    m[123_456] = m[123_456].replace("hello world 123456", "hello world 123457");
    let b = dir.path().join("b.ndjson");
    write(&b, &m);
    let (rc, out) = diff(&a, &b);
    assert_eq!(rc, 1);
    assert!(out.contains("changed: src/doc-123456"), "{out}");

    // 1 missing doc
    let mut m = base.clone();
    m.remove(777);
    write(&b, &m);
    let (rc, out) = diff(&a, &b);
    assert_eq!(rc, 1);
    assert!(out.contains("missing in B: src/doc-777"), "{out}");

    // 1 duplicate doc
    let mut m = base.clone();
    m.push(base[4242].clone());
    write(&b, &m);
    let (rc, out) = diff(&a, &b);
    assert_eq!(rc, 1);
    assert!(out.contains("doc-4242 A=1 B=2"), "{out}");
}

#[test]
fn compressed_inputs_match_plain() {
    let dir = tempfile::tempdir().unwrap();
    let base = corpus(5_000);
    let a = dir.path().join("a.ndjson");
    write(&a, &base);
    let body: String = base.iter().map(|l| l.clone() + "\n").collect();
    let gz = dir.path().join("a.ndjson.gz");
    let mut e = flate2::write::GzEncoder::new(
        std::fs::File::create(&gz).unwrap(),
        flate2::Compression::fast(),
    );
    e.write_all(body.as_bytes()).unwrap();
    e.finish().unwrap();
    let zs = dir.path().join("a.ndjson.zst");
    std::fs::write(&zs, zstd::encode_all(body.as_bytes(), 3).unwrap()).unwrap();
    assert_eq!(diff(&a, &gz).0, 0);
    assert_eq!(diff(&a, &zs).0, 0);
}

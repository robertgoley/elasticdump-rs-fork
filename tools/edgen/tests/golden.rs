//! TST-DATA-001 CI hash check: the same seed gives the same bytes on every
//! run, platform and thread count. Changing a profile on purpose means
//! updating these values (and noting it: old corpora are no longer equal).
use sha2::{Digest, Sha256};
use std::process::Command;

fn generate(args: &[&str], threads: &str) -> Vec<u8> {
    let o = Command::new(env!("CARGO_BIN_EXE_edgen"))
        .arg("gen")
        .args(args)
        .args(["--threads", threads])
        .output()
        .unwrap();
    assert!(o.status.success(), "{}", String::from_utf8_lossy(&o.stderr));
    o.stdout
}

fn sha(b: &[u8]) -> String {
    Sha256::digest(b)
        .iter()
        .map(|x| format!("{x:02x}"))
        .collect::<Vec<_>>()
        .join("")
}

const GOLDEN: &[(&str, &str, &str)] = &[
    (
        "logs",
        "100000",
        "0fef9488d40b0ec2e5ce28f6c7d688d8d472d26aac52c240fb4ac9321e8ab86d",
    ),
    (
        "metrics",
        "10000",
        "51d134064c52817c0963de73ae5699af3258f686712ed011d5fbf7d872dba26e",
    ),
    (
        "nested",
        "10000",
        "28e75f4eb676c43f35b74d3914b96e4753e9059b46ef4d102f7519a7b29e28a6",
    ),
    (
        "geo",
        "10000",
        "f56644c5b2eb5f50803f52249847bd7ab3f9a60c9458237cfdb908c94591db3f",
    ),
    (
        "wide",
        "10000",
        "e43bd27e13acfc139cfce459f7e005a311bac0a74a5f40a7d1f566c7525cf295",
    ),
    (
        "huge",
        "3",
        "007c38a2894aebbc12e1c87cb672ad468e7a318fa154d4a6bfac321f2bcfb240",
    ),
    (
        "edge",
        "10000",
        "de3357c1dc6ee834f6e200e69ccb9ac243058db118b342b86213bf79931a2db6",
    ),
    (
        "mixed",
        "10000",
        "9affc63f9b977c1b285d6749050b46d648889af8aaf5800c05a58ce7e79a5ec5",
    ),
];

#[test]
fn golden_hashes_for_every_profile_and_thread_count() {
    for (p, n, want) in GOLDEN {
        for t in ["1", "3"] {
            let out = generate(
                &[
                    "--profile",
                    p,
                    "--seed",
                    "1",
                    "--count",
                    n,
                    "--fields",
                    "200",
                    "--huge-min-mb",
                    "1",
                    "--huge-max-mb",
                    "2",
                ],
                t,
            );
            assert_eq!(&sha(&out), want, "profile {p} threads {t}");
        }
    }
}

#[test]
fn split_parts_concatenate_to_the_whole_and_zstd_roundtrips() {
    let dir = tempfile::tempdir().unwrap();
    let whole = generate(&["--profile", "logs", "--count", "20000"], "2");
    let base = dir.path().join("logs.ndjson.zst");
    let o = Command::new(env!("CARGO_BIN_EXE_edgen"))
        .args([
            "gen",
            "--profile",
            "logs",
            "--count",
            "20000",
            "--compress",
            "zstd",
            "--split-bytes",
            "4000000",
            "--out",
        ])
        .arg(&base)
        .output()
        .unwrap();
    assert!(o.status.success());
    let mut parts: Vec<_> = std::fs::read_dir(dir.path())
        .unwrap()
        .map(|e| e.unwrap().path())
        .collect();
    parts.sort();
    assert!(parts.len() >= 4, "{parts:?}");
    let mut cat = Vec::new();
    for p in &parts {
        let d = zstd::decode_all(std::fs::File::open(p).unwrap()).unwrap();
        assert!(
            d.len() <= 4_000_000 + 2_000 && d.ends_with(b"\n"),
            "{}",
            d.len()
        );
        cat.extend(d);
    }
    assert_eq!(sha(&cat), sha(&whole));
}

#[test]
fn convert_is_deterministic() {
    let dir = tempfile::tempdir().unwrap();
    let inp = dir.path().join("access.log");
    std::fs::write(&inp, "1.2.3.4 - - [01/Jul/1995:00:00:01 -0400] \"GET / HTTP/1.0\" 200 10\n5.6.7.8 - - [01/Jul/1995:00:00:02 -0400] \"GET /x HTTP/1.0\" 404 -\n").unwrap();
    let run = || {
        let o = Command::new(env!("CARGO_BIN_EXE_edgen"))
            .args([
                "convert",
                "--kind",
                "clf",
                "--dataset",
                "nasa",
                "--index",
                "nasa",
                "--input",
            ])
            .arg(&inp)
            .output()
            .unwrap();
        assert!(o.status.success());
        o.stdout
    };
    let a = run();
    assert_eq!(a, run());
    assert_eq!(a.iter().filter(|b| **b == b'\n').count(), 2);
}

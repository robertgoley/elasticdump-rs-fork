//! L-643 (TST-BRK-CLI-004): regex abuse in `--renameIndex` / `--templateRegex` is a clear
//! usage error (exit 2) before any request: oversized patterns get a size-limit hint, invalid
//! patterns name the flag, and replacement references to missing capture groups are refused
//! instead of silently expanding to nothing.

use std::net::TcpListener;
use std::process::{Command, Output};

fn bin() -> Command {
    let mut c = Command::new(env!("CARGO_BIN_EXE_elasticdump-rs"));
    for v in [
        "ELASTICDUMP_INPUT_USERNAME",
        "ELASTICDUMP_INPUT_PASSWORD",
        "ELASTICDUMP_OUTPUT_USERNAME",
        "ELASTICDUMP_OUTPUT_PASSWORD",
        "NODE_TLS_REJECT_UNAUTHORIZED",
        "RUST_LOG",
        "SSL_CERT_FILE",
        "SSL_CERT_DIR",
    ] {
        c.env_remove(v);
    }
    c
}

fn stderr(o: &Output) -> String {
    String::from_utf8_lossy(&o.stderr).into_owned()
}

/// A listener that must never see a connection: proves the error came before any request.
struct Silent(TcpListener);

impl Silent {
    fn new() -> Self {
        let l = TcpListener::bind("127.0.0.1:0").unwrap();
        l.set_nonblocking(true).unwrap();
        Silent(l)
    }
    fn url(&self) -> String {
        format!("http://{}", self.0.local_addr().unwrap())
    }
    fn assert_untouched(&self, what: &str) {
        match self.0.accept() {
            Err(e) if e.kind() == std::io::ErrorKind::WouldBlock => {}
            other => panic!("{what}: a connection reached the server: {other:?}"),
        }
    }
}

/// The largest single argument Linux passes to a process is 128 KiB (MAX_ARG_STRLEN), so a
/// 1 MB pattern cannot reach the binary through argv; the 1 MB case is a unit test in
/// src/rename.rs. This one is as large as argv allows and is still far past the size limit.
fn big_pattern() -> String {
    ".".repeat(120_000)
}

struct Case {
    name: &'static str,
    pattern: String,
    expect: &'static [&'static str],
}

fn rename_cases() -> Vec<Case> {
    vec![
        Case {
            name: "nested repetition",
            pattern: "(a{1000}){1000}:x".into(),
            expect: &["size limit", "simplify it"],
        },
        Case {
            name: "120 KB pattern",
            pattern: format!("{}:x", big_pattern()),
            expect: &["size limit", "simplify it", "120002 bytes"],
        },
        Case {
            name: "invalid UTF-8 escape",
            pattern: r"(?-u:\xff):x".into(),
            expect: &["invalid regex", "invalid UTF-8"],
        },
        Case {
            name: "look-around",
            pattern: "(?=a)b:x".into(),
            expect: &["invalid regex", "look-around"],
        },
        Case {
            name: "$9 with one group",
            pattern: "^(.*)$:x-$9".into(),
            expect: &["capture group '9'", "1 group(s)"],
        },
        Case {
            name: "${name} typo",
            pattern: "^(?P<name>.*)$:x-${nmae}".into(),
            expect: &["capture group 'nmae'", "named: name"],
        },
        Case {
            name: "$1x is the group named '1x'",
            pattern: "^(.*)$:$1x".into(),
            expect: &["capture group '1x'", "${1}x"],
        },
    ]
}

#[test]
fn l643_rename_index_regex_abuse_is_a_usage_error() {
    let dir = tempfile::tempdir().unwrap();
    let input = dir.path().join("in.json");
    std::fs::write(&input, "{\"_index\":\"a\",\"_id\":\"1\",\"_source\":{}}\n").unwrap();
    let server = Silent::new();
    let url = server.url();
    for c in rename_cases() {
        let o = bin()
            .args(["--input", input.to_str().unwrap(), "--output", &url])
            .arg("--renameIndex")
            .arg(&c.pattern)
            .output()
            .expect("spawn");
        let err = stderr(&o);
        let short = &err[..err.len().min(2000)];
        assert_eq!(o.status.code(), Some(2), "{}: {short}", c.name);
        assert!(
            err.contains("--renameIndex"),
            "{}: flag not named: {short}",
            c.name
        );
        for want in c.expect {
            assert!(err.contains(want), "{}: missing '{want}': {short}", c.name);
        }
        // a huge pattern is not echoed back in full
        assert!(
            err.len() < 4096,
            "{}: {} bytes of stderr",
            c.name,
            err.len()
        );
        server.assert_untouched(c.name);
    }
}

#[test]
fn l643_template_regex_abuse_is_a_usage_error() {
    let dir = tempfile::tempdir().unwrap();
    let server = Silent::new();
    let url = server.url();
    let cases: Vec<(&str, String, &[&str])> = vec![
        (
            "nested repetition",
            "(a{1000}){1000}".into(),
            &["size limit", "simplify it"],
        ),
        (
            "120 KB pattern",
            big_pattern(),
            &["size limit", "simplify it"],
        ),
        (
            "invalid UTF-8 escape",
            r"(?-u:\xff)".into(),
            &["unsupported pattern", "invalid UTF-8"],
        ),
        (
            "look-around",
            "(?!x)".into(),
            &["unsupported pattern", "look-around"],
        ),
    ];
    for (i, (name, pattern, expect)) in cases.into_iter().enumerate() {
        let out = dir.path().join(format!("t{i}.json"));
        let o = bin()
            .args(["--type", "template", "--input", &url, "--output"])
            .arg(&out)
            .arg("--templateRegex")
            .arg(&pattern)
            .output()
            .expect("spawn");
        let err = stderr(&o);
        let short = &err[..err.len().min(2000)];
        assert_eq!(o.status.code(), Some(2), "{name}: {short}");
        assert!(
            err.contains("--templateRegex"),
            "{name}: flag not named: {short}"
        );
        for want in expect {
            assert!(err.contains(want), "{name}: missing '{want}': {short}");
        }
        assert!(err.len() < 4096, "{name}: {} bytes of stderr", err.len());
        assert!(!out.exists(), "{name}: output file created");
        server.assert_untouched(name);
    }
}

//! Phase A6 chaos tests (TST-BRK, L-6xx) that need no cluster: an in-process router mock
//! plays the misbehaving server or proxy. Live-engine variants live in `tests/it`.

mod support;

use std::process::{Command, Output};

use support::router_mock::{Fault, Recorded, Reply, Route, RouterMock};

const INFO: &str = r#"{"version":{"distribution":"opensearch","number":"2.19.0"}}"#;
const CLEARED: &str = r#"{"succeeded":true,"num_freed":1}"#;

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

async fn run_async(args: Vec<String>, envs: Vec<(String, String)>) -> Output {
    tokio::task::spawn_blocking(move || bin().args(args).envs(envs).output().expect("spawn"))
        .await
        .unwrap()
}

fn s(v: &[&str]) -> Vec<String> {
    v.iter().map(|x| x.to_string()).collect()
}

fn stderr(o: &Output) -> String {
    String::from_utf8_lossy(&o.stderr).into_owned()
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

fn count(reqs: &[Recorded], method: &str, prefix: &str) -> usize {
    reqs.iter()
        .filter(|r| r.method == method && r.path.starts_with(prefix))
        .count()
}

fn dir_names(d: &std::path::Path) -> Vec<String> {
    std::fs::read_dir(d)
        .unwrap()
        .flatten()
        .map(|e| e.file_name().to_string_lossy().into_owned())
        .collect()
}

// L-601 (TST-BRK-NET-001): the connection is reset (RST) after N bytes of a scroll
// continuation's body. The continuation is not retried (the cursor may have advanced), the
// run exits 1, nothing is committed, the scroll is cleared, and the message says a larger
// --retryAttempts cannot help and how to recover. A rerun against a healthy server
// produces the full dump.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn l601_reset_mid_scroll_continuation_is_not_retried() {
    const N: usize = 4;
    let full: Vec<String> = (1..=N)
        .map(|i| hit(&i.to_string(), &format!("{{\"n\":{i}}}")))
        .collect();
    let mock = |cut: Option<usize>| {
        let full = full.clone();
        async move {
            let second = Reply::ok(page("s1", &full[1..2], N));
            let second = match cut {
                Some(n) => second.with_fault(Fault::ResetAt(n)),
                None => second,
            };
            RouterMock::builder()
                .route(Route::exact("GET", "/").reply(Reply::ok(INFO)))
                .route(
                    Route::contains("POST", "/idx/_search").reply(Reply::ok(page(
                        "s1",
                        &full[..1],
                        N,
                    ))),
                )
                .route(
                    Route::exact("POST", "/_search/scroll").replies(
                        [second]
                            .into_iter()
                            .chain((2..N).map(|i| Reply::ok(page("s1", &full[i..i + 1], N))))
                            .chain([Reply::ok(page("s1", &[], N))]),
                    ),
                )
                .fallback(Reply::ok(CLEARED))
                .start()
                .await
        }
    };
    for cut in [150usize, 260] {
        let m = mock(Some(cut)).await;
        let dir = tempfile::tempdir().unwrap();
        let out = dir.path().join("d.json");
        let o = run_async(
            s(&[
                "--input",
                &format!("{}/idx", m.url()),
                "--output",
                out.to_str().unwrap(),
                "--limit",
                "1",
                "--retryAttempts",
                "5",
                "--retryDelay",
                "1",
            ]),
            vec![],
        )
        .await;
        let err = stderr(&o);
        assert!(
            err.contains("reset by peer"),
            "cut {cut}: a mid-body RST: {err}"
        );
        let reqs = m.requests();
        assert_eq!(o.status.code(), Some(1), "cut {cut}: {err}");
        assert_eq!(
            count(&reqs, "POST", "/_search/scroll"),
            1,
            "cut {cut}: {err}"
        );
        assert!(
            err.contains("not retried") && err.contains("--retryAttempts cannot help"),
            "cut {cut}: {err}"
        );
        assert!(
            err.contains("--scrollId") && err.contains("Re-run"),
            "{err}"
        );
        assert_eq!(
            count(&reqs, "DELETE", "/_search/scroll"),
            1,
            "cut {cut}: scroll cleared: {err}"
        );
        assert!(!out.exists(), "cut {cut}: nothing committed");
        assert!(dir_names(dir.path()).is_empty(), "staging left");

        // the rerun the message recommends
        let m = mock(None).await;
        let o = run_async(
            s(&[
                "--input",
                &format!("{}/idx", m.url()),
                "--output",
                out.to_str().unwrap(),
                "--limit",
                "1",
            ]),
            vec![],
        )
        .await;
        assert!(o.status.success(), "{}", stderr(&o));
        let got = std::fs::read_to_string(&out).unwrap();
        let ids: Vec<String> = got
            .lines()
            .map(|l| serde_json::from_str::<serde_json::Value>(l).unwrap()["_id"].to_string())
            .collect();
        assert_eq!(ids, ["\"1\"", "\"2\"", "\"3\"", "\"4\""], "{got}");
    }
}

/// A stateful Elasticsearch 8.19 PIT server: `n` docs (`_id` = i, sort value = i), slice of
/// doc i = i % max, a new PIT id on every delivered response (ES may rotate ids), and a
/// seeded chance of an RST part-way through any search response (first attempt per cursor
/// only, so a retry always gets through).
struct PitServer {
    n: u64,
    rng: std::sync::Mutex<support::chaos::Rng>,
    p_reset: f64,
    issued: std::sync::Mutex<Vec<String>>,
    faulted: std::sync::Mutex<std::collections::HashSet<String>>,
    resets: std::sync::atomic::AtomicUsize,
    bad_ids: std::sync::Mutex<Vec<String>>,
    next_id: std::sync::atomic::AtomicUsize,
    max_slices: std::sync::atomic::AtomicU64,
}

impl PitServer {
    fn reply(&self, r: &Recorded) -> Reply {
        if r.method == "GET" && r.path == "/" {
            return Reply::ok(
                r#"{"version":{"number":"8.19.0","build_flavor":"default"},"tagline":"You Know, for Search"}"#,
            );
        }
        if r.method == "POST" && r.path.ends_with("/_pit") {
            let id = self.issue();
            return Reply::ok(format!(r#"{{"id":"{id}"}}"#));
        }
        if r.method == "DELETE" {
            return Reply::ok(r#"{"succeeded":true,"num_freed":1}"#);
        }
        if r.method == "POST" && r.path == "/_search" {
            let b: serde_json::Value = serde_json::from_slice(&r.body).unwrap();
            let id = b["pit"]["id"].as_str().unwrap_or("").to_string();
            if !self.issued.lock().unwrap().contains(&id) {
                self.bad_ids.lock().unwrap().push(id.clone());
            }
            let size = b["size"].as_u64().unwrap_or(10);
            let after = b["search_after"][0]
                .as_u64()
                .map(|x| x as i64)
                .unwrap_or(-1);
            let (sid, max) = (
                b["slice"]["id"].as_u64().unwrap_or(0),
                b["slice"]["max"].as_u64().unwrap_or(1),
            );
            self.max_slices
                .fetch_max(max, std::sync::atomic::Ordering::Relaxed);
            let in_slice: Vec<u64> = (0..self.n).filter(|i| i % max == sid).collect();
            let hits: Vec<String> = in_slice
                .iter()
                .filter(|&&i| i as i64 > after)
                .take(size as usize)
                .map(|i| {
                    format!(
                        r#"{{"_index":"idx","_id":"{i}","_score":null,"_source":{{"n":{i}}},"sort":[{i}]}}"#
                    )
                })
                .collect();
            let next = self.issue();
            let body = format!(
                r#"{{"pit_id":"{next}","took":1,"timed_out":false,"_shards":{{"total":1,"successful":1,"skipped":0,"failed":0}},"hits":{{"total":{{"value":{},"relation":"eq"}},"hits":[{}]}}}}"#,
                in_slice.len(),
                hits.join(",")
            );
            let key = format!("{sid}/{max}/{after}");
            let reply = Reply::ok(body.clone());
            let mut rng = self.rng.lock().unwrap();
            if rng.chance(self.p_reset) && self.faulted.lock().unwrap().insert(key) {
                // the client never sees this response, so its id is not "issued" to it
                self.issued.lock().unwrap().pop();
                self.resets
                    .fetch_add(1, std::sync::atomic::Ordering::Relaxed);
                // headers are > 100 bytes, so a cut below the body length is always mid-response
                let cut = 1 + rng.below(body.len() as u64 - 1) as usize;
                return reply.with_fault(Fault::ResetAt(cut));
            }
            return reply;
        }
        Reply::json(404, r#"{"error":"unexpected"}"#)
    }

    fn issue(&self) -> String {
        let mut g = self.issued.lock().unwrap();
        let id = format!(
            "pit-{}",
            self.next_id
                .fetch_add(1, std::sync::atomic::Ordering::Relaxed)
        );
        g.push(id.clone());
        id
    }
}

// L-602 (TST-BRK-NET-002): RSTs part-way through PIT search pages (unsliced and 16 slices)
// on Elasticsearch, whose PIT id rotates on every response. Each failed page is retried
// with the same search_after and a PIT id the server issued; the dump has no duplicates and
// no gaps: the sorted output equals the fault-free run's over 50 seeded runs per mode.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn l602_reset_mid_pit_page_is_retried_without_dups_or_gaps() {
    use std::sync::Arc;
    const N: u64 = 300;
    let base = support::chaos::seed("l602_reset_mid_pit_page_is_retried_without_dups_or_gaps");
    let dump = |slices: &'static str, seed: u64, p_reset: f64| async move {
        let srv = Arc::new(PitServer {
            n: N,
            rng: std::sync::Mutex::new(support::chaos::Rng::new(seed)),
            p_reset,
            issued: Default::default(),
            faulted: Default::default(),
            resets: Default::default(),
            bad_ids: Default::default(),
            next_id: Default::default(),
            max_slices: Default::default(),
        });
        let s2 = Arc::clone(&srv);
        let m = RouterMock::builder()
            .handler(move |r| {
                let rep = s2.reply(&r);
                async move { rep }
            })
            .start()
            .await;
        let dir = tempfile::tempdir().unwrap();
        let out = dir.path().join("d.json");
        let o = run_async(
            s(&[
                "--input",
                &format!("{}/idx", m.url()),
                "--output",
                out.to_str().unwrap(),
                "--searchType",
                "pit",
                "--limit",
                "7",
                "--slices",
                slices,
                "--retryAttempts",
                "3",
                "--retryDelay",
                "1",
            ]),
            vec![],
        )
        .await;
        let tag = format!("slices={slices} seed={seed}");
        assert!(o.status.success(), "{tag}: {}", stderr(&o));
        let mut lines: Vec<String> = std::fs::read_to_string(&out)
            .unwrap()
            .lines()
            .map(str::to_string)
            .collect();
        lines.sort();
        let ids: std::collections::BTreeSet<String> = lines
            .iter()
            .map(|l| serde_json::from_str::<serde_json::Value>(l).unwrap()["_id"].to_string())
            .collect();
        assert_eq!(ids.len() as u64, N, "{tag}: gaps or duplicates");
        assert_eq!(lines.len() as u64, N, "{tag}: duplicates");
        let bad = srv.bad_ids.lock().unwrap().clone();
        assert!(
            bad.is_empty(),
            "{tag}: PIT ids the server never delivered: {bad:?}"
        );
        assert_eq!(
            srv.max_slices
                .load(std::sync::atomic::Ordering::Relaxed)
                .to_string(),
            slices,
            "{tag}: sliced PIT"
        );
        (lines, srv.resets.load(std::sync::atomic::Ordering::Relaxed))
    };
    let mut total_resets = 0;
    for slices in ["1", "16"] {
        let (reference, r0) = dump(slices, 0, 0.0).await;
        assert_eq!(r0, 0);
        for run in 0..50u64 {
            let seed = base.wrapping_add(run);
            let (got, resets) = dump(slices, seed, 0.2).await;
            assert!(
                got == reference,
                "slices={slices} seed={seed}: output differs from the fault-free run"
            );
            total_resets += resets;
        }
    }
    eprintln!("L-602: {total_resets} mid-response resets over 100 runs");
    assert!(total_resets > 100, "faults were injected ({total_resets})");
}

// L-603 (TST-BRK-NET-005): a large, legitimate response dripped slowly. `--timeout 30000` bounds
// silence, not the whole request (Node's `request` semantics), so a response that takes longer
// than the timeout but keeps progressing succeeds; a server that goes silent for longer than
// the timeout still fails. Scaled down for CI: --timeout 1000 with a body that takes ~3 s at
// 2 KB every 100 ms (20 KB/s) succeeds; the same with a 1.5 s pause fails.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn l603_slow_but_progressing_response_is_not_cut_off_by_timeout() {
    use std::time::{Duration, Instant};
    let docs: Vec<String> = (0..120)
        .map(|i| {
            hit(
                &i.to_string(),
                &format!(r#"{{"pad":"{}"}}"#, "x".repeat(400)),
            )
        })
        .collect();
    let body = page("s1", &docs, docs.len());
    assert!(body.len() > 55_000);
    for (label, fault, ok) in [
        (
            "drip",
            Fault::SlowDrip {
                chunk: 2048,
                every: Duration::from_millis(100),
            },
            true,
        ),
        (
            "silence",
            Fault::StallMidBody {
                after: 4096,
                stall: Duration::from_millis(1500),
            },
            false,
        ),
    ] {
        let m = RouterMock::builder()
            .route(Route::exact("GET", "/").reply(Reply::ok(INFO)))
            .route(
                Route::contains("POST", "/idx/_search").reply(Reply::ok(&body).with_fault(fault)),
            )
            .route(
                Route::exact("POST", "/_search/scroll").reply(Reply::ok(page(
                    "s1",
                    &[],
                    docs.len(),
                ))),
            )
            .fallback(Reply::ok(CLEARED))
            .start()
            .await;
        let dir = tempfile::tempdir().unwrap();
        let out = dir.path().join("d.json");
        let t = Instant::now();
        let o = run_async(
            s(&[
                "--input",
                &format!("{}/idx", m.url()),
                "--output",
                out.to_str().unwrap(),
                "--limit",
                "200",
                "--timeout",
                "1000",
                "--retryAttempts",
                "1",
                "--retryDelay",
                "1",
            ]),
            vec![],
        )
        .await;
        let took = t.elapsed();
        let err = stderr(&o);
        if ok {
            assert!(o.status.success(), "{label}: {err}");
            assert!(
                took > Duration::from_millis(2000),
                "{label}: really slow ({took:?})"
            );
            assert_eq!(
                std::fs::read_to_string(&out).unwrap().lines().count(),
                docs.len()
            );
        } else {
            assert_eq!(o.status.code(), Some(1), "{label}: {err}");
            assert!(
                err.contains("timed out") && err.contains("--timeout"),
                "{label}: {err}"
            );
            assert!(took < Duration::from_secs(15), "{label}: {took:?}");
            assert!(!out.exists());
        }
    }
}

// L-604 (TST-BRK-NET-007): half-closed sockets. The server sends FIN right after the headers
// (no body), or reads the request and sends FIN without any answer. A search page and an
// explicit-id `_bulk` are retried and the run completes; when every attempt is half-closed
// the run fails with a clear transport error well within the bound.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn l604_half_closed_sockets_are_retried_or_fail_clearly() {
    use std::time::{Duration, Instant};
    let body = page("s1", &[hit("1", "{}"), hit("2", "{}")], 2);
    let dir = tempfile::tempdir().unwrap();
    let input = dir.path().join("in.json");
    std::fs::write(&input, format!("{}\n{}\n", hit("1", "{}"), hit("2", "{}"))).unwrap();
    let bulk_ok =
        r#"{"took":1,"errors":false,"items":[{"index":{"status":201}},{"index":{"status":201}}]}"#;
    for (label, fault) in [
        ("FIN after headers", Fault::FinAfterHeaders),
        ("FIN without an answer", Fault::TruncateAt(0)),
    ] {
        for persistent in [false, true] {
            let tag = format!("{label}, persistent={persistent}");
            let replies = |good: Reply| -> Vec<Reply> {
                let bad = good.clone().with_fault(fault.clone());
                if persistent {
                    vec![bad]
                } else {
                    vec![bad, good]
                }
            };
            // dump: the initial search
            let m = RouterMock::builder()
                .route(Route::exact("GET", "/").reply(Reply::ok(INFO)))
                .route(Route::contains("POST", "/idx/_search").replies(replies(Reply::ok(&body))))
                .route(Route::exact("POST", "/_search/scroll").reply(Reply::ok(page("s1", &[], 2))))
                .fallback(Reply::ok(CLEARED))
                .start()
                .await;
            let out = dir.path().join("d.json");
            let _ = std::fs::remove_file(&out);
            let t = Instant::now();
            let o = run_async(
                s(&[
                    "--input",
                    &format!("{}/idx", m.url()),
                    "--output",
                    out.to_str().unwrap(),
                    "--retryAttempts",
                    "2",
                    "--retryDelay",
                    "1",
                    "--timeout",
                    "2000",
                ]),
                vec![],
            )
            .await;
            let err = stderr(&o);
            assert!(t.elapsed() < Duration::from_secs(20), "{tag}");
            if persistent {
                assert_eq!(o.status.code(), Some(1), "dump {tag}: {err}");
                assert_eq!(
                    count(&m.requests(), "POST", "/idx/_search"),
                    3,
                    "dump {tag}: {err}"
                );
                assert!(
                    err.contains("initial search")
                        && (err.contains("connection") || err.contains("body")),
                    "dump {tag}: {err}"
                );
                assert!(!out.exists());
            } else {
                assert!(o.status.success(), "dump {tag}: {err}");
                assert_eq!(std::fs::read_to_string(&out).unwrap().lines().count(), 2);
            }
            // restore: an explicit-id _bulk
            let m = RouterMock::builder()
                .route(Route::exact("GET", "/").reply(Reply::ok(INFO)))
                .route(Route::contains("POST", "/_bulk").replies(replies(Reply::ok(bulk_ok))))
                .fallback(Reply::ok(
                    r#"{"_shards":{"total":1,"successful":1,"failed":0}}"#,
                ))
                .start()
                .await;
            let o = run_async(
                s(&[
                    "--input",
                    input.to_str().unwrap(),
                    "--output",
                    &format!("{}/dst", m.url()),
                    "--noRefresh",
                    "--retryAttempts",
                    "2",
                    "--retryDelay",
                    "1",
                    "--timeout",
                    "2000",
                ]),
                vec![],
            )
            .await;
            let err = stderr(&o);
            let bulks = count(&m.requests(), "POST", "/dst/_bulk");
            if persistent {
                assert_eq!(o.status.code(), Some(1), "restore {tag}: {err}");
                assert_eq!(bulks, 3, "restore {tag}: {err}");
            } else {
                assert!(o.status.success(), "restore {tag}: {err}");
                assert_eq!(bulks, 2, "restore {tag}: {err}");
                assert!(err.contains("Total Writes: 2"), "{err}");
            }
        }
    }
}

/// Run the binary in an unprivileged user+mount namespace with `/etc/hosts`,
/// `/etc/nsswitch.conf` and `/etc/resolv.conf` replaced by the given files (L-605). `None` when
/// unprivileged namespaces are not available on this machine.
fn in_dns_sandbox(
    hosts: &std::path::Path,
    nsswitch: &std::path::Path,
    resolv: &std::path::Path,
) -> Option<Command> {
    let ok = Command::new("unshare")
        .args(["-rm", "true"])
        .status()
        .is_ok_and(|s| s.success());
    if !ok {
        return None;
    }
    let mut c = Command::new("unshare");
    c.args(["-rm", "sh", "-c"]).arg(format!(
        "mount --bind '{}' /etc/hosts && mount --bind '{}' /etc/nsswitch.conf && mount --bind '{}' /etc/resolv.conf && exec \"$@\"",
        hosts.display(),
        nsswitch.display(),
        resolv.display()
    ));
    c.arg("sh").arg(env!("CARGO_BIN_EXE_elasticdump-rs"));
    for v in [
        "HTTP_PROXY",
        "HTTPS_PROXY",
        "ALL_PROXY",
        "http_proxy",
        "https_proxy",
        "all_proxy",
        "RUST_LOG",
    ] {
        c.env_remove(v);
    }
    Some(c)
}

// L-605 (TST-BRK-NET-008): DNS failures, offline, in a private mount namespace.
// NXDOMAIN at start (`hosts: files` with no entry): fails fast, not retried, names the host and
// never the password. A black-holed resolver (TEST-NET nameserver, 30 s resolver timeout): bounded
// by --timeout per attempt. DNS failing mid-run (the hosts entry disappears for ~0.6 s while every
// request opens a new connection): retried, the dump completes.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn l605_dns_failures_fail_fast_are_bounded_or_retried() {
    use std::time::{Duration, Instant};
    let d = tempfile::tempdir().unwrap();
    let hosts = d.path().join("hosts");
    let nss = d.path().join("nsswitch.conf");
    let resolv = d.path().join("resolv.conf");
    std::fs::write(&hosts, "127.0.0.1 localhost\n").unwrap();
    std::fs::write(&nss, "hosts: files\n").unwrap();
    std::fs::write(&resolv, "nameserver 127.0.0.1\n").unwrap();
    let Some(_) = in_dns_sandbox(&hosts, &nss, &resolv) else {
        eprintln!("SKIP: unprivileged user namespaces are not available");
        return;
    };
    let out = d.path().join("d.json");

    // NXDOMAIN at start
    let t = Instant::now();
    let o = in_dns_sandbox(&hosts, &nss, &resolv)
        .unwrap()
        .args(s(&[
            "--input",
            "http://admin:Sup3rSecret@edrs-nx.invalid:9200/idx",
            "--output",
            out.to_str().unwrap(),
        ]))
        .output()
        .unwrap();
    let err = stderr(&o);
    assert_eq!(o.status.code(), Some(1), "{err}");
    assert!(
        t.elapsed() < Duration::from_secs(3),
        "fails fast: {:?}",
        t.elapsed()
    );
    assert!(
        err.contains("cannot resolve host 'edrs-nx.invalid'") && err.contains("not retried"),
        "{err}"
    );
    assert!(!err.contains("retrying"), "{err}");
    assert!(!err.contains("Sup3rSecret"), "{err}");

    // black-holed resolver: DNS through a TEST-NET address nobody answers
    std::fs::write(&nss, "hosts: files dns\n").unwrap();
    std::fs::write(
        &resolv,
        "nameserver 192.0.2.1\noptions timeout:30 attempts:1\n",
    )
    .unwrap();
    let t = Instant::now();
    let o = in_dns_sandbox(&hosts, &nss, &resolv)
        .unwrap()
        .args(s(&[
            "--input",
            "http://admin:Sup3rSecret@edrs-blackhole.example:9200/idx",
            "--output",
            out.to_str().unwrap(),
            "--timeout",
            "1500",
            "--retryAttempts",
            "1",
            "--retryDelay",
            "1",
        ]))
        .output()
        .unwrap();
    let err = stderr(&o);
    let took = t.elapsed();
    assert_eq!(o.status.code(), Some(1), "{err}");
    assert!(
        took < Duration::from_secs(8),
        "bounded by 2 x --timeout: {took:?}"
    );
    assert!(
        err.contains("edrs-blackhole.example") && err.contains("timed out"),
        "{err}"
    );
    assert!(!err.contains("Sup3rSecret"), "{err}");

    // mid-run: the name stops resolving for a moment
    std::fs::write(&nss, "hosts: files\n").unwrap();
    std::fs::write(&hosts, "127.0.0.1 localhost\n127.0.0.1 edrs-dns-test\n").unwrap();
    const N: usize = 6;
    let m = RouterMock::builder()
        .close_always()
        .route(Route::exact("GET", "/").reply(Reply::ok(INFO)))
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
        .fallback(Reply::ok(CLEARED))
        .start()
        .await;
    let port = m.addr().rsplit(':').next().unwrap().to_string();
    let mut cmd = in_dns_sandbox(&hosts, &nss, &resolv).unwrap();
    cmd.args(s(&[
        "--input",
        &format!("http://edrs-dns-test:{port}/idx"),
        "--output",
        out.to_str().unwrap(),
        "--limit",
        "1",
        "--throttleInterval",
        "200",
        "--bufferSize",
        "1",
        "--retryAttempts",
        "8",
        "--retryDelay",
        "200",
    ]));
    let child = tokio::task::spawn_blocking(move || cmd.output().unwrap());
    // wait for the first continuation, then take the name away for ~600 ms
    let t = Instant::now();
    while count(&m.requests(), "POST", "/_search/scroll") < 1 {
        assert!(t.elapsed() < Duration::from_secs(10), "no continuation");
        tokio::time::sleep(Duration::from_millis(5)).await;
    }
    std::fs::write(&hosts, "127.0.0.1 localhost\n").unwrap();
    tokio::time::sleep(Duration::from_millis(600)).await;
    std::fs::write(&hosts, "127.0.0.1 localhost\n127.0.0.1 edrs-dns-test\n").unwrap();
    let o = child.await.unwrap();
    let err = stderr(&o);
    assert!(o.status.success(), "{err}");
    assert!(
        err.contains("DNS lookup of 'edrs-dns-test' failed") && err.contains("retried"),
        "the outage was seen and retried: {err}"
    );
    assert_eq!(std::fs::read_to_string(&out).unwrap().lines().count(), N);
}

// L-606 (TST-BRK-NET-009), mock half: after a node restart the cluster answers the next page
// with 404 search_context_missing. Scroll and PIT dumps fail with a "context no longer exists
// ... re-run" message and commit nothing. (The live restart run is scripts/res/restart.sh.)
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn l606_lost_search_context_says_rerun() {
    let missing = r#"{"error":{"root_cause":[{"type":"search_context_missing_exception","reason":"No search context found for id [7]"}],"type":"search_phase_execution_exception","reason":"all shards failed"},"status":404}"#;
    let es = r#"{"version":{"number":"8.19.0","build_flavor":"default"},"tagline":"You Know, for Search"}"#;
    for pit in [false, true] {
        let first = if pit {
            r#"{"pit_id":"p1","hits":{"total":{"value":3,"relation":"eq"},"hits":[{"_index":"idx","_id":"1","_source":{},"sort":[1]}]},"_shards":{"total":1,"successful":1,"skipped":0,"failed":0},"timed_out":false}"#.to_string()
        } else {
            page("s1", &[hit("1", "{}")], 3)
        };
        let m = RouterMock::builder()
            .route(Route::exact("GET", "/").reply(Reply::ok(es)))
            .route(Route::contains("POST", "/idx/_pit").reply(Reply::ok(r#"{"id":"p1"}"#)))
            .route(Route::contains("POST", "/idx/_search").reply(Reply::ok(&first)))
            .route(
                Route::exact("POST", "/_search")
                    .replies([Reply::ok(&first), Reply::json(404, missing)]),
            )
            .route(Route::exact("POST", "/_search/scroll").reply(Reply::json(404, missing)))
            .fallback(Reply::ok(CLEARED))
            .start()
            .await;
        let dir = tempfile::tempdir().unwrap();
        let out = dir.path().join("d.json");
        let mut a = s(&[
            "--input",
            &format!("{}/idx", m.url()),
            "--output",
            out.to_str().unwrap(),
            "--limit",
            "1",
            "--retryDelay",
            "1",
        ]);
        if pit {
            a.extend(s(&["--searchType", "pit"]));
        }
        let o = run_async(a, vec![]).await;
        let err = stderr(&o);
        let kind = if pit { "point-in-time" } else { "scroll" };
        assert_eq!(o.status.code(), Some(1), "{kind}: {err}");
        assert!(
            err.contains(&format!("the {kind} context no longer exists"))
                && err.contains("restarted")
                && err.contains("Re-run the dump"),
            "{kind}: {err}"
        );
        assert!(!out.exists(), "{kind}");
    }
}

// L-607 (TST-BRK-NET-010): a proxy answers with HTML: a 200 login page, 502/503 error pages,
// a WAF 403. Each fails with exit 1 and a short message that says an HTML page answered
// (with its title) instead of quoting it: the final error line stays under 4 KB though the
// page is 50 KB. A 200 HTML page is not retried (it is not a transient failure).
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn l607_html_from_a_proxy_is_named_not_dumped() {
    let page_html = |title: &str| {
        format!(
            "<!DOCTYPE html>\n<html><head><title>{title}</title></head><body>{}</body></html>",
            "<p>lorem ipsum</p>\n".repeat(2700)
        )
    };
    let dir = tempfile::tempdir().unwrap();
    let input = dir.path().join("in.json");
    std::fs::write(&input, format!("{}\n", hit("1", "{}"))).unwrap();
    for (status, title, want) in [
        (200u16, "Sign in - Corp SSO", "expected JSON from"),
        (502, "502 Bad Gateway", "an HTML page, not JSON"),
        (
            503,
            "Service Temporarily Unavailable",
            "an HTML page, not JSON",
        ),
        (403, "Access Denied - WAF", "an HTML page, not JSON"),
    ] {
        let body = page_html(title);
        assert!(body.len() > 50_000);
        for restore in [false, true] {
            let tag = format!("HTTP {status} restore={restore}");
            let html = Reply::html(status, &body);
            let mut b =
                RouterMock::builder().route(Route::exact("GET", "/").reply(Reply::ok(INFO)));
            b = if restore {
                b.route(Route::contains("POST", "/_bulk").reply(html))
            } else {
                b.route(Route::contains("POST", "/idx/_search").reply(html))
            };
            let m = b.fallback(Reply::ok(CLEARED)).start().await;
            let out = dir.path().join("d.json");
            let a = if restore {
                s(&[
                    "--input",
                    input.to_str().unwrap(),
                    "--output",
                    &format!("{}/dst", m.url()),
                    "--noRefresh",
                ])
            } else {
                s(&[
                    "--input",
                    &format!("{}/idx", m.url()),
                    "--output",
                    out.to_str().unwrap(),
                ])
            };
            let mut a = a;
            a.extend(s(&["--retryAttempts", "1", "--retryDelay", "1"]));
            let o = run_async(a, vec![]).await;
            let err = stderr(&o);
            assert_eq!(o.status.code(), Some(1), "{tag}: {err}");
            let last = err.lines().rfind(|l| l.starts_with("Error")).unwrap_or("");
            assert!(last.contains(want) && last.contains(title), "{tag}: {last}");
            assert!(last.len() < 4096, "{tag}: {} bytes", last.len());
            assert!(
                err.len() < 16 * 1024,
                "{tag}: whole stderr {} bytes",
                err.len()
            );
            assert!(!err.contains("lorem ipsum</p> <p>lorem ipsum</p> <p>lorem ipsum</p> <p>lorem ipsum</p> <p>lorem ipsum</p> <p>lorem ipsum</p> <p>lorem ipsum</p> <p>lorem ipsum</p> <p>lorem ipsum</p> <p>lorem ipsum</p> <p>lorem ipsum</p> <p>lorem ipsum</p>"), "{tag}: the page is not dumped");
            let path = if restore {
                "/dst/_bulk"
            } else {
                "/idx/_search"
            };
            let tries = count(&m.requests(), "POST", path);
            if status == 200 {
                assert_eq!(tries, 1, "{tag}: a 200 HTML page is not retried");
            }
            assert!(!out.exists());
        }
    }
}

// L-608 (TST-BRK-NET-011): broken response encodings: `Content-Encoding: gzip` on a plain
// body, a gzip stream cut short (valid HTTP framing), `Content-Length` shorter or longer than
// the body. With and without --esCompress: always exit 1 within a few seconds, a message that
// names the problem, nothing committed, never a partial page; transient ones are retried.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn l608_broken_encodings_fail_clearly_or_are_retried() {
    use std::time::{Duration, Instant};
    let docs: Vec<String> = (0..40)
        .map(|i| hit(&i.to_string(), r#"{"a":"xxxxxxxxxxxxxxxx"}"#))
        .collect();
    let body = page("s1", &docs, docs.len());
    let cases: Vec<(&str, Reply)> = vec![
        (
            "gzip header, plain body",
            Reply::ok(&body).with_fault(Fault::GzipPlainBody),
        ),
        ("gzip body nobody asked for", Reply::ok(&body).gzipped()),
        (
            "gzip stream cut",
            Reply::ok(&body)
                .gzipped()
                .with_fault(Fault::TruncateBodyBy(30)),
        ),
        (
            "Content-Length short",
            Reply::ok(&body).with_fault(Fault::WrongContentLength(-40)),
        ),
        (
            "Content-Length long",
            Reply::ok(&body).with_fault(Fault::WrongContentLength(40)),
        ),
    ];
    for (label, bad) in cases {
        for compress in [false, true] {
            for persistent in [true, false] {
                let tag = format!("{label}, esCompress={compress}, persistent={persistent}");
                let good = if compress {
                    Reply::ok(&body).gzipped()
                } else {
                    Reply::ok(&body)
                };
                let replies = if persistent {
                    vec![bad.clone()]
                } else {
                    vec![bad.clone(), good]
                };
                let m = RouterMock::builder()
                    .route(Route::exact("GET", "/").reply(Reply::ok(INFO)))
                    .route(Route::contains("POST", "/idx/_search").replies(replies))
                    .route(
                        Route::exact("POST", "/_search/scroll").reply(Reply::ok(page(
                            "s1",
                            &[],
                            docs.len(),
                        ))),
                    )
                    .fallback(Reply::ok(CLEARED))
                    .start()
                    .await;
                let dir = tempfile::tempdir().unwrap();
                let out = dir.path().join("d.json");
                let mut a = s(&[
                    "--input",
                    &format!("{}/idx", m.url()),
                    "--output",
                    out.to_str().unwrap(),
                    "--retryAttempts",
                    "1",
                    "--retryDelay",
                    "1",
                    "--timeout",
                    "2000",
                ]);
                if compress {
                    a.push("--esCompress".into());
                }
                let t = Instant::now();
                let o = run_async(a, vec![]).await;
                let err = stderr(&o);
                assert!(t.elapsed() < Duration::from_secs(15), "{tag}");
                if persistent && o.status.success() {
                    // only acceptable when the body was in fact intact: every doc, byte-exact
                    let got = std::fs::read_to_string(&out).unwrap();
                    let ids: Vec<String> = got
                        .lines()
                        .map(|l| {
                            serde_json::from_str::<serde_json::Value>(l).unwrap()["_id"].to_string()
                        })
                        .collect();
                    let want: Vec<String> = (0..docs.len()).map(|i| format!("\"{i}\"")).collect();
                    assert_eq!(ids, want, "{tag}");
                    eprintln!("L-608 {tag}: exit 0 with every document (the body was intact)");
                } else if persistent {
                    assert_eq!(o.status.code(), Some(1), "{tag}: {err}");
                    assert!(!out.exists(), "{tag}");
                    let last = err
                        .lines()
                        .rfind(|l| l.starts_with("Error"))
                        .unwrap_or("")
                        .to_string();
                    assert!(
                        [
                            "decod",
                            "malformed",
                            "end of file",
                            "gzip",
                            "invalid",
                            "not exactly one JSON",
                            "length",
                            "EOF"
                        ]
                        .iter()
                        .any(|n| last.contains(n)),
                        "{tag}: {last}"
                    );
                    eprintln!("L-608 {tag}: {}", &last[..last.len().min(300)]);
                } else {
                    // the first (bad) answer is either retried, or rejected before any doc
                    if o.status.success() {
                        assert_eq!(
                            std::fs::read_to_string(&out).unwrap().lines().count(),
                            docs.len(),
                            "{tag}"
                        );
                    } else {
                        assert!(!out.exists(), "{tag}: {err}");
                    }
                    eprintln!(
                        "L-608 {tag}: exit {:?}, searches {}",
                        o.status.code(),
                        count(&m.requests(), "POST", "/idx/_search")
                    );
                }
            }
        }
    }
}

// L-609 (TST-BRK-NET-012): redirects (301, 302, 307, 308; same-origin and to another host)
// on `GET /`, the search and `_bulk` are never followed: exit 1, a message naming the
// redirect target (without credentials), and no request ever reaches the target, so no
// `Authorization` header leaves for it.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn l609_redirects_are_never_followed() {
    let target = RouterMock::builder()
        .fallback(Reply::ok(INFO))
        .start()
        .await;
    let dir = tempfile::tempdir().unwrap();
    let input = dir.path().join("in.json");
    std::fs::write(&input, format!("{}\n", hit("1", "{}"))).unwrap();
    for status in [301u16, 302, 307, 308] {
        for (what, same_origin) in [
            ("info", false),
            ("search", false),
            ("search", true),
            ("bulk", false),
        ] {
            let tag = format!("{status} {what} same_origin={same_origin}");
            let loc = if same_origin {
                "/elsewhere/_search".to_string()
            } else {
                format!("{}/stolen?x=1", target.url())
            };
            let redirect = Reply::redirect(status, &loc);
            let mut b = RouterMock::builder();
            b = match what {
                "info" => b.route(Route::exact("GET", "/").reply(redirect)),
                "search" => b
                    .route(Route::exact("GET", "/").reply(Reply::ok(INFO)))
                    .route(Route::contains("POST", "/idx/_search").reply(redirect)),
                _ => b
                    .route(Route::exact("GET", "/").reply(Reply::ok(INFO)))
                    .route(Route::contains("POST", "/_bulk").reply(redirect)),
            };
            let m = b.fallback(Reply::ok(CLEARED)).start().await;
            let auth_url = m
                .url()
                .replacen("http://", "http://admin:Redir3ctSecret@", 1);
            let out = dir.path().join("d.json");
            // `GET /` is probed by restores (scroll dumps do not need it)
            let a = if what == "bulk" || what == "info" {
                s(&[
                    "--input",
                    input.to_str().unwrap(),
                    "--output",
                    &format!("{auth_url}/dst"),
                    "--noRefresh",
                ])
            } else {
                s(&[
                    "--input",
                    &format!("{auth_url}/idx"),
                    "--output",
                    out.to_str().unwrap(),
                ])
            };
            let mut a = a;
            a.extend(s(&["--retryAttempts", "1", "--retryDelay", "1"]));
            let o = run_async(a, vec![]).await;
            let err = stderr(&o);
            assert_eq!(o.status.code(), Some(1), "{tag}: {err}");
            assert!(
                err.contains("redirect") && err.contains("not followed"),
                "{tag}: {err}"
            );
            assert!(
                err.contains(if same_origin {
                    "/elsewhere/_search"
                } else {
                    "/stolen"
                }),
                "{tag}: {err}"
            );
            assert!(
                !err.contains("Redir3ctSecret") && !err.contains("x=1"),
                "{tag}: {err}"
            );
            assert!(!out.exists(), "{tag}");
            assert_eq!(count(&m.requests(), "POST", "/elsewhere"), 0, "{tag}");
        }
    }
    assert!(
        target.requests().is_empty(),
        "the redirect target was contacted: {:?}",
        target
            .requests()
            .iter()
            .map(|r| (
                r.method.clone(),
                r.path.clone(),
                r.header("authorization").is_some()
            ))
            .collect::<Vec<_>>()
    );
}

struct Srv(std::process::Child);
impl Drop for Srv {
    fn drop(&mut self) {
        let _ = self.0.kill();
        let _ = self.0.wait();
    }
}

/// `scripts/tls/certs.sh` into `dir/certs`, or `None` (no openssl) after printing SKIP.
fn cert_factory(dir: &std::path::Path) -> Option<std::path::PathBuf> {
    let certs = dir.join("certs");
    let script = concat!(env!("CARGO_MANIFEST_DIR"), "/scripts/tls/certs.sh");
    let ok = Command::new("bash")
        .arg(script)
        .arg(&certs)
        .output()
        .is_ok_and(|o| o.status.success());
    if !ok {
        println!("SKIP: openssl cert factory unavailable");
        return None;
    }
    Some(certs)
}

fn s_server(certs: &std::path::Path, cert: &str, extra: &[&str]) -> (Srv, u16) {
    let port = std::net::TcpListener::bind("127.0.0.1:0")
        .unwrap()
        .local_addr()
        .unwrap()
        .port();
    let child = Command::new("openssl")
        .current_dir(certs)
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
}

// L-611 (TST-BRK-TLS-002): client certificate faults beyond L-431 (missing, wrong --pass, legacy
// PEM): a client cert from a CA the server does not trust, DER instead of PEM (cert and key),
// a cert/key mismatch, empty files, a CA bundle that also holds a private key. Each: exit 1 or
// 2 (never a panic), one message naming the file or the cause, and the passphrase canary never
// appears.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn l611_client_certificate_faults_have_clear_messages() {
    let dir = tempfile::tempdir().unwrap();
    let Some(certs) = cert_factory(dir.path()) else {
        return;
    };
    let sh = |cmd: &str| {
        let o = Command::new("sh")
            .current_dir(&certs)
            .args(["-c", cmd])
            .output()
            .unwrap();
        assert!(
            o.status.success(),
            "{cmd}: {}",
            String::from_utf8_lossy(&o.stderr)
        );
    };
    sh(
        "openssl req -newkey rsa:2048 -nodes -keyout rogue.key -out rogue.csr -subj /CN=rogue 2>/dev/null \
        && printf 'extendedKeyUsage=clientAuth\\n' > rogue.ext \
        && openssl x509 -req -in rogue.csr -CA other-ca.pem -CAkey other-ca.key -CAcreateserial -out rogue.pem -days 2 -extfile rogue.ext 2>/dev/null",
    );
    sh(
        "openssl x509 -in client.pem -outform DER -out client.der && openssl pkey -in client.key -outform DER -out client.key.der",
    );
    sh(": > empty.pem && cat ca.pem client.key > ca-with-key.pem");
    let (_srv, port) = s_server(
        &certs,
        "ok",
        &["-Verify", "1", "-verify_return_error", "-CAfile", "ca.pem"],
    );
    let p = |n: &str| certs.join(n).to_str().unwrap().to_string();
    let canary = "Canary-Passphrase-611";
    let cases: Vec<(&str, Vec<String>, &[&str])> = vec![
        (
            "client cert from an untrusted CA",
            s(&[
                "--input-ca",
                &p("ca.pem"),
                "--input-cert",
                &p("rogue.pem"),
                "--input-key",
                &p("rogue.key"),
            ]),
            &["rejected this client certificate"],
        ),
        (
            "DER client cert",
            s(&[
                "--input-ca",
                &p("ca.pem"),
                "--input-cert",
                &p("client.der"),
                "--input-key",
                &p("client.key"),
            ]),
            &["client.der", "PEM"],
        ),
        (
            "DER client key",
            s(&[
                "--input-ca",
                &p("ca.pem"),
                "--input-cert",
                &p("client.pem"),
                "--input-key",
                &p("client.key.der"),
            ]),
            &["client.key.der", "PEM"],
        ),
        (
            "cert/key mismatch",
            s(&[
                "--input-ca",
                &p("ca.pem"),
                "--input-cert",
                &p("client.pem"),
                "--input-key",
                &p("ok.key"),
            ]),
            &["match"],
        ),
        (
            "empty cert file",
            s(&[
                "--input-ca",
                &p("ca.pem"),
                "--input-cert",
                &p("empty.pem"),
                "--input-key",
                &p("client.key"),
            ]),
            &["empty.pem"],
        ),
        (
            "empty key file",
            s(&[
                "--input-ca",
                &p("ca.pem"),
                "--input-cert",
                &p("client.pem"),
                "--input-key",
                &p("empty.pem"),
            ]),
            &["empty.pem"],
        ),
        (
            "empty CA file",
            s(&[
                "--input-ca",
                &p("empty.pem"),
                "--input-cert",
                &p("client.pem"),
                "--input-key",
                &p("client.key"),
            ]),
            &["empty.pem"],
        ),
        (
            "CA bundle with a private key",
            s(&[
                "--input-ca",
                &p("ca-with-key.pem"),
                "--input-cert",
                &p("client.pem"),
                "--input-key",
                &p("client.key"),
            ]),
            &["ca-with-key.pem", "also contains a private key"],
        ),
    ];
    for (name, mut extra, want) in cases {
        extra.extend(s(&["--input-pass", canary]));
        let mut a = s(&[
            "--input",
            &format!("https://localhost:{port}/idx"),
            "--output",
            dir.path().join("o.json").to_str().unwrap(),
            "--type",
            "mapping",
            "--retryAttempts",
            "1",
            "--retryDelay",
            "1",
        ]);
        a.extend(extra);
        let o = run_async(a, vec![]).await;
        let err = stderr(&o);
        println!(
            "L-611 {name}: exit {:?}: {}",
            o.status.code(),
            err.lines().rfind(|l| l.starts_with("Error")).unwrap_or("")
        );
        assert!(
            matches!(o.status.code(), Some(1) | Some(2)),
            "{name}: {err}"
        );
        assert!(!err.contains("panicked"), "{name}: {err}");
        assert!(!err.contains(canary), "{name}: passphrase echoed: {err}");
        assert!(
            want.iter().all(|w| err.contains(w)),
            "{name}: want {want:?}: {err}"
        );
        assert!(!dir.path().join("o.json").exists());
    }
}

// L-612 (TST-BRK-TLS-003): protocol mismatches beyond L-431 (https to a plain HTTP port, http
// to a TLS port, TLS 1.1 only): a TLS 1.0-only server, a server with an RSA-1024 key, and
// https to a port that accepts but never speaks. Each fails with a clear, permanent message
// (no retry loop) within a few seconds.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn l612_protocol_mismatches_fail_clearly_and_fast() {
    use std::time::{Duration, Instant};
    let dir = tempfile::tempdir().unwrap();
    let Some(certs) = cert_factory(dir.path()) else {
        return;
    };
    let o = Command::new("sh")
        .current_dir(&certs)
        .args(["-c", "openssl req -newkey rsa:1024 -nodes -keyout weak.key -out weak.csr -subj /CN=localhost 2>/dev/null \
            && printf 'subjectAltName=DNS:localhost\\nextendedKeyUsage=serverAuth\\n' > weak.ext \
            && openssl x509 -req -in weak.csr -CA ca.pem -CAkey ca.key -CAcreateserial -out weak.pem -days 2 -extfile weak.ext 2>/dev/null"])
        .output()
        .unwrap();
    assert!(o.status.success(), "{}", String::from_utf8_lossy(&o.stderr));
    let (_t10, p10) = s_server(&certs, "ok", &["-tls1", "-cipher", "DEFAULT:@SECLEVEL=0"]);
    let (_weak, pweak) = s_server(&certs, "weak", &["-cipher", "DEFAULT:@SECLEVEL=0"]);
    let silent = std::net::TcpListener::bind("127.0.0.1:0").unwrap();
    let psilent = silent.local_addr().unwrap().port();
    std::thread::spawn(move || {
        let mut keep = Vec::new();
        for c in silent.incoming().flatten() {
            keep.push(c); // accept, never answer
        }
    });
    let ca = certs.join("ca.pem").to_str().unwrap().to_string();
    for (name, port, want) in [
        ("TLS 1.0 only", p10, "TLS 1.2+ is required"),
        ("RSA-1024 server key", pweak, "RSA key under 2048 bits"),
        ("https to a silent port", psilent, "timed out"),
    ] {
        let t = Instant::now();
        let o = run_async(
            s(&[
                "--input",
                &format!("https://localhost:{port}/idx"),
                "--output",
                dir.path().join("o.json").to_str().unwrap(),
                "--type",
                "mapping",
                "--input-ca",
                &ca,
                "--retryAttempts",
                "2",
                "--retryDelay",
                "1",
                "--timeout",
                "1500",
            ]),
            vec![],
        )
        .await;
        let took = t.elapsed();
        let err = stderr(&o);
        println!(
            "L-612 {name}: {took:?}: {}",
            err.lines().rfind(|l| l.starts_with("Error")).unwrap_or("")
        );
        assert_eq!(o.status.code(), Some(1), "{name}: {err}");
        assert!(err.contains(want), "{name}: want {want:?}: {err}");
        assert!(took < Duration::from_secs(10), "{name}: bounded: {took:?}");
        if name != "https to a silent port" {
            assert!(
                !err.contains("retrying"),
                "{name}: a TLS failure is permanent: {err}"
            );
        }
    }
}

// L-613 (TST-BRK-TLS-004): SSL_CERT_FILE / SSL_CERT_DIR. A usable SSL_CERT_FILE is honoured
// (the test CA becomes trusted). Garbage or empty ones fall back to the bundled webpki roots
// with a warning naming the variable (so the test CA is not trusted), never silently.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn l613_ssl_cert_env_is_honoured_or_falls_back_loudly() {
    let dir = tempfile::tempdir().unwrap();
    let Some(certs) = cert_factory(dir.path()) else {
        return;
    };
    let (_srv, port) = s_server(&certs, "ok", &[]);
    let garbage = dir.path().join("garbage.pem");
    std::fs::write(
        &garbage,
        "-----BEGIN CERTIFICATE-----\nnot base64 at all!\n-----END CERTIFICATE-----\n",
    )
    .unwrap();
    let empty_dir = dir.path().join("empty-dir");
    std::fs::create_dir(&empty_dir).unwrap();
    let ca = certs.join("ca.pem").to_str().unwrap().to_string();
    for (name, env, want, warn) in [
        (
            "SSL_CERT_FILE = test CA",
            vec![("SSL_CERT_FILE", ca.as_str())],
            "expected JSON from",
            false,
        ),
        (
            "SSL_CERT_FILE = garbage",
            vec![("SSL_CERT_FILE", garbage.to_str().unwrap())],
            "not signed by a trusted CA",
            true,
        ),
        (
            "SSL_CERT_FILE = missing file",
            vec![("SSL_CERT_FILE", "/nonexistent/ca.pem")],
            "not signed by a trusted CA",
            true,
        ),
        (
            "SSL_CERT_DIR = empty dir, no file",
            vec![
                ("SSL_CERT_FILE", "/dev/null"),
                ("SSL_CERT_DIR", empty_dir.to_str().unwrap()),
            ],
            "not signed by a trusted CA",
            true,
        ),
    ] {
        let o = run_async(
            s(&[
                "--input",
                &format!("https://localhost:{port}/idx"),
                "--output",
                dir.path().join("o.json").to_str().unwrap(),
                "--type",
                "mapping",
                "--retryAttempts",
                "1",
                "--retryDelay",
                "1",
            ]),
            env.iter()
                .map(|(k, v)| (k.to_string(), v.to_string()))
                .collect(),
        )
        .await;
        let err = stderr(&o);
        assert_eq!(o.status.code(), Some(1), "{name}: {err}");
        assert!(err.contains(want), "{name}: want {want:?}: {err}");
        if warn {
            assert!(
                err.contains("WARN") && err.contains("SSL_CERT_") && err.contains("bundled webpki"),
                "{name}: loud fallback: {err}"
            );
        } else {
            assert!(!err.contains("bundled webpki"), "{name}: {err}");
        }
    }
}

// L-614 (TST-BRK-SRV-002): `_bulk` answers 5xx longer than the retry budget. Without
// --ignore-errors the run stops with exit 1 naming the status. With --ignore-errors (Q3: an
// explicit opt-in) it goes on, and every lost document is counted: "Failed Writes" equals
// the documents of the failed batches, "Total Writes" the rest.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn l614_bulk_5xx_beyond_the_retry_budget_is_counted() {
    let dir = tempfile::tempdir().unwrap();
    let input = dir.path().join("in.json");
    let lines: String = (0..10).fold(String::new(), |mut acc, i| {
        acc.push_str(&hit(&i.to_string(), "{}"));
        acc.push('\n');
        acc
    });
    std::fs::write(&input, lines).unwrap();
    let ok2 =
        r#"{"took":1,"errors":false,"items":[{"index":{"status":201}},{"index":{"status":201}}]}"#;
    for status in [500u16, 502, 503] {
        for ignore in [false, true] {
            let tag = format!("HTTP {status} ignore-errors={ignore}");
            // 5 batches of 2; batch 2 (all 3 attempts) and batch 4 (all 3 attempts) fail
            let fail = || Reply::json(status, r#"{"error":"overloaded"}"#);
            let replies: Vec<Reply> = vec![
                Reply::ok(ok2),
                fail(),
                fail(),
                fail(),
                Reply::ok(ok2),
                fail(),
                fail(),
                fail(),
                Reply::ok(ok2),
            ];
            let m = RouterMock::builder()
                .route(Route::exact("GET", "/").reply(Reply::ok(INFO)))
                .route(Route::contains("POST", "/_bulk").replies(replies))
                .fallback(Reply::ok(
                    r#"{"_shards":{"total":1,"successful":1,"failed":0}}"#,
                ))
                .start()
                .await;
            let mut a = s(&[
                "--input",
                input.to_str().unwrap(),
                "--output",
                &format!("{}/dst", m.url()),
                "--noRefresh",
                "--limit",
                "2",
                "--concurrency",
                "1",
                "--retryAttempts",
                "2",
                "--retryDelay",
                "1",
            ]);
            if ignore {
                a.push("--ignore-errors".into());
            }
            let o = run_async(a, vec![]).await;
            let err = stderr(&o);
            println!(
                "L-614 {tag}: exit {:?}; {}",
                o.status.code(),
                err.lines()
                    .filter(|l| l.contains("Writes") || l.starts_with("Error"))
                    .collect::<Vec<_>>()
                    .join(" | ")
            );
            if ignore {
                assert_eq!(o.status.code(), Some(0), "{tag}: {err}");
                assert!(
                    err.contains("Total Writes: 6") && err.contains("Failed Writes: 4"),
                    "{tag}: {err}"
                );
                assert_eq!(count(&m.requests(), "POST", "/dst/_bulk"), 9, "{tag}");
            } else {
                assert_eq!(o.status.code(), Some(1), "{tag}: {err}");
                assert!(err.contains(&format!("HTTP {status}")), "{tag}: {err}");
            }
        }
    }
}

// L-615 (TST-BRK-SRV-003): one `_bulk` answer with every kind of item: 201, 200, 409, 400
// mapper_parsing, 429, 503, 500, an item without `status`, and an item under a different key
// than the action (`create` for an `index` action, as data streams answer). Only the 429 and
// 503 documents are re-sent; the counts equal the ground truth; the failed ids are listed.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn l615_mixed_bulk_items_are_classified_one_by_one() {
    let dir = tempfile::tempdir().unwrap();
    let input = dir.path().join("in.json");
    let ids = ["a", "b", "c", "d", "e", "f", "g", "h", "i"];
    let text = ids.iter().fold(String::new(), |mut acc, id| {
        acc.push_str(&hit(&format!("doc-{id}"), "{}"));
        acc.push('\n');
        acc
    });
    std::fs::write(&input, text).unwrap();
    let err = |status: u16, ty: &str| {
        format!(
            r#"{{"index":{{"_id":"x","status":{status},"error":{{"type":"{ty}","reason":"r"}}}}}}"#
        )
    };
    let first = format!(
        r#"{{"took":1,"errors":true,"items":[{},{},{},{},{},{},{},{},{}]}}"#,
        r#"{"index":{"_id":"doc-a","status":201,"result":"created"}}"#,
        r#"{"index":{"_id":"doc-b","status":200,"result":"updated"}}"#,
        err(409, "version_conflict_engine_exception"),
        err(400, "mapper_parsing_exception"),
        err(429, "es_rejected_execution_exception"),
        err(503, "unavailable_shards_exception"),
        err(500, "illegal_state_exception"),
        r#"{"index":{"_id":"doc-h"}}"#,
        r#"{"create":{"_id":"doc-i","status":201,"result":"created"}}"#,
    );
    let second =
        r#"{"took":1,"errors":false,"items":[{"index":{"status":201}},{"index":{"status":201}}]}"#;
    let m = RouterMock::builder()
        .route(Route::exact("GET", "/").reply(Reply::ok(INFO)))
        .route(Route::contains("POST", "/_bulk").replies([Reply::ok(first), Reply::ok(second)]))
        .fallback(Reply::ok(
            r#"{"_shards":{"total":1,"successful":1,"failed":0}}"#,
        ))
        .start()
        .await;
    let o = run_async(
        s(&[
            "--input",
            input.to_str().unwrap(),
            "--output",
            &format!("{}/dst", m.url()),
            "--noRefresh",
            "--retryAttempts",
            "2",
            "--retryDelay",
            "1",
        ]),
        vec![],
    )
    .await;
    let e = stderr(&o);
    let bulks: Vec<String> = m
        .requests()
        .iter()
        .filter(|r| r.path.ends_with("/_bulk"))
        .map(|r| r.body_str())
        .collect();
    assert_eq!(bulks.len(), 2, "{e}");
    let resent: Vec<&str> = bulks[1]
        .lines()
        .filter(|l| l.contains("\"_id\"") && l.contains("\"index\""))
        .collect();
    assert_eq!(resent.len(), 2, "only 429 + 503 re-sent: {}", bulks[1]);
    assert!(
        resent[0].contains("doc-e") && resent[1].contains("doc-f"),
        "{}",
        bulks[1]
    );
    assert!(
        e.contains("Total Writes: 5") && e.contains("Failed Writes: 4"),
        "{e}"
    );
    for id in ["doc-c", "doc-d", "doc-g", "doc-h"] {
        assert!(e.contains(id), "failed id {id} listed: {e}");
    }
    for id in ["doc-a", "doc-b", "doc-e", "doc-f", "doc-i"] {
        assert!(
            !e.lines().any(|l| l.contains("ERROR") && l.contains(id)),
            "{id} is not a failure: {e}"
        );
    }
    assert_eq!(
        o.status.code(),
        Some(3),
        "finished with failed writes (Q3): {e}"
    );
}

// L-616 (TST-BRK-SRV-004): `_bulk` answers that do not account for every document: fewer or
// more items than documents (with `errors:false` too), `errors:false` without `items`, a body
// that is not JSON. The run fails clearly and none of those documents counts as written.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn l616_bulk_answers_that_do_not_match_are_never_counted_as_written() {
    let dir = tempfile::tempdir().unwrap();
    let input = dir.path().join("in.json");
    std::fs::write(&input, format!("{}\n{}\n", hit("1", "{}"), hit("2", "{}"))).unwrap();
    for (label, body, want) in [
        (
            "one item, errors:false",
            r#"{"took":1,"errors":false,"items":[{"index":{"status":201}}]}"#,
            "has 1 item(s) for 2 document(s)",
        ),
        (
            "three items",
            r#"{"took":1,"errors":false,"items":[{"index":{"status":201}},{"index":{"status":201}},{"index":{"status":201}}]}"#,
            "has 3 item(s) for 2 document(s)",
        ),
        (
            "errors:false, no items",
            r#"{"took":1,"errors":false}"#,
            "has no items",
        ),
        (
            "items not an array",
            r#"{"took":1,"errors":false,"items":{"a":1}}"#,
            "not an array",
        ),
        ("not JSON", "OK", "not a JSON object"),
        ("empty body", "", "not a JSON object"),
    ] {
        let m = RouterMock::builder()
            .route(Route::exact("GET", "/").reply(Reply::ok(INFO)))
            .route(Route::contains("POST", "/_bulk").reply(Reply::ok(body)))
            .fallback(Reply::ok(
                r#"{"_shards":{"total":1,"successful":1,"failed":0}}"#,
            ))
            .start()
            .await;
        let o = run_async(
            s(&[
                "--input",
                input.to_str().unwrap(),
                "--output",
                &format!("{}/dst", m.url()),
                "--noRefresh",
                "--retryAttempts",
                "1",
                "--retryDelay",
                "1",
            ]),
            vec![],
        )
        .await;
        let err = stderr(&o);
        assert_eq!(o.status.code(), Some(1), "{label}: {err}");
        assert!(
            err.contains(want) && err.contains("none of them is counted as written"),
            "{label}: {err}"
        );
        assert!(
            !err.contains("Total Writes: 2") && !err.contains("Total Writes: 1"),
            "{label}: {err}"
        );
    }
}

// L-617 (TST-BRK-SRV-005): a 200 search page that is cut inside `hits.hits[k]`, has no `hits`,
// or whose `hits.hits` is not an array. With stdout output (no staging to hide a partial
// page) the consumer sees the earlier pages whole and nothing of the broken one; exit 1.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn l617_malformed_search_page_never_reaches_stdout_partially() {
    let first = page("s1", &[hit("1", "{}"), hit("2", "{}")], 6);
    let good2 = page(
        "s1",
        &[hit("3", r#"{"k":"v"}"#), hit("4", "{}"), hit("5", "{}")],
        6,
    );
    let cut = good2[..good2.find(r#""_id":"5""#).unwrap() + 4].to_string();
    for (label, bad) in [
        ("cut inside hits.hits[2]", cut),
        ("no hits", r#"{"_scroll_id":"s1","timed_out":false,"_shards":{"total":1,"successful":1,"skipped":0,"failed":0}}"#.to_string()),
        ("hits.hits not an array", r#"{"_scroll_id":"s1","timed_out":false,"_shards":{"total":1,"successful":1,"skipped":0,"failed":0},"hits":{"total":{"value":6,"relation":"eq"},"hits":{"0":{}}}}"#.to_string()),
    ] {
        let m = RouterMock::builder()
            .route(Route::exact("GET", "/").reply(Reply::ok(INFO)))
            .route(Route::contains("POST", "/idx/_search").reply(Reply::ok(&first)))
            .route(Route::exact("POST", "/_search/scroll").reply(Reply::ok(&bad)))
            .fallback(Reply::ok(CLEARED))
            .start()
            .await;
        let o = run_async(s(&["--input", &format!("{}/idx", m.url()), "--output", "$", "--limit", "3"]), vec![]).await;
        let err = stderr(&o);
        let out = String::from_utf8_lossy(&o.stdout).into_owned();
        assert_eq!(o.status.code(), Some(1), "{label}: {err}");
        let ids: Vec<String> = out
            .lines()
            .map(|l| serde_json::from_str::<serde_json::Value>(l).unwrap_or_else(|e| panic!("{label}: partial line {l:?}: {e}"))["_id"].to_string())
            .collect();
        assert_eq!(ids, ["\"1\"", "\"2\""], "{label}: only the first page, whole: {out}");
        assert!(err.contains("malformed search response") || err.contains("hits"), "{label}: {err}");
    }
}

// L-620 (TST-BRK-SRV-010): the server returns the same hit on two pages (an overlapping or
// buggy cursor). With the snapshot total intact (4 docs, 5 hits served) the scroll total check
// already fails the run (exit 1, nothing committed). When the total agrees with the pages (5)
// the tool does not dedupe (Node parity): both copies are written in order, and the test-side
// id check catches the overlap, which is the regression guard a digest comparison relies on.
#[tokio::test]
async fn l620_duplicate_hits_fail_the_total_check_or_are_written_as_served() {
    let h: Vec<String> = ["a", "b", "c", "d"]
        .iter()
        .map(|i| hit(i, &format!("{{\"k\":\"{i}\"}}")))
        .collect();
    for total in [4usize, 5] {
        let m = RouterMock::builder()
            .route(Route::exact("GET", "/").reply(Reply::ok(INFO)))
            .route(
                Route::contains("POST", "/idx/_search").reply(Reply::ok(page(
                    "s1",
                    &h[..2],
                    total,
                ))),
            )
            .route(Route::exact("POST", "/_search/scroll").replies([
                // overlaps the first page by one hit
                Reply::ok(page("s1", &[h[1].clone(), h[2].clone()], total)),
                Reply::ok(page("s1", &h[3..], total)),
                Reply::ok(page("s1", &[], total)),
            ]))
            .fallback(Reply::ok(CLEARED))
            .start()
            .await;
        let dir = tempfile::tempdir().unwrap();
        let out = dir.path().join("d.json");
        let o = run_async(
            s(&[
                "--input",
                &format!("{}/idx", m.url()),
                "--output",
                out.to_str().unwrap(),
                "--limit",
                "2",
            ]),
            vec![],
        )
        .await;
        if total == 4 {
            assert_eq!(o.status.code(), Some(1), "{}", stderr(&o));
            assert!(
                stderr(&o).contains("retrieved 5 of 4 documents"),
                "{}",
                stderr(&o)
            );
            assert!(!out.exists(), "nothing committed");
            continue;
        }
        assert!(o.status.success(), "{}", stderr(&o));
        let ids: Vec<String> = std::fs::read_to_string(&out)
            .unwrap()
            .lines()
            .map(|l| {
                serde_json::from_str::<serde_json::Value>(l).unwrap()["_id"]
                    .as_str()
                    .unwrap()
                    .to_string()
            })
            .collect();
        assert_eq!(
            ids,
            ["a", "b", "b", "c", "d"],
            "written as served, in order"
        );
        let unique: std::collections::BTreeSet<_> = ids.iter().collect();
        assert_ne!(unique.len(), ids.len(), "the id check sees the duplicate");
    }
}

// L-623 (TST-BRK-IN-001): a truncated gzip (cut at many offsets, including inside the 8-byte
// trailer) and a gzip with trailing garbage. Every case exits 1 naming the file, the byte offset
// and the documents read so far, and a file output is never committed.
#[tokio::test]
async fn l623_truncated_gzip_names_file_offset_and_docs_read() {
    use std::io::Write as _;
    let dir = tempfile::tempdir().unwrap();
    let mut plain = String::new();
    for i in 0..300 {
        plain.push_str(&format!(
            "{{\"_index\":\"x\",\"_id\":\"{i}\",\"_source\":{{\"n\":{i}}}}}\n"
        ));
    }
    let mut enc = flate2::write::GzEncoder::new(Vec::new(), flate2::Compression::default());
    enc.write_all(plain.as_bytes()).unwrap();
    let gz = enc.finish().unwrap();
    let mut cases: Vec<(String, Vec<u8>)> = [
        10usize,
        gz.len() / 3,
        gz.len() / 2,
        gz.len() - 8,
        gz.len() - 3,
        gz.len() - 1,
    ]
    .iter()
    .map(|&cut| (format!("cut{cut}"), gz[..cut].to_vec()))
    .collect();
    let mut garbage = gz.clone();
    garbage.extend_from_slice(b"not gzip at all\n");
    cases.push(("garbage".into(), garbage));
    for (name, bytes) in cases {
        let input = dir.path().join(format!("{name}.json.gz"));
        std::fs::write(&input, bytes).unwrap();
        let out = dir.path().join(format!("{name}.out.json"));
        let o = run_async(
            s(&[
                "--input",
                input.to_str().unwrap(),
                "--fsCompress",
                "--output",
                out.to_str().unwrap(),
            ]),
            vec![],
        )
        .await;
        let err = stderr(&o);
        assert_eq!(o.status.code(), Some(1), "{name}: {err}");
        assert!(err.contains(&format!("{name}.json.gz")), "{name}: {err}");
        assert!(
            err.contains("near byte") && err.contains("complete documents"),
            "{name}: {err}"
        );
        assert!(!out.exists(), "{name}: output committed");
    }
}

// L-624 (TST-BRK-IN-003): wrong compression. bzip2 (the Rally corpora), xz, lzma, lz4, zip, 7z
// and tar archives (plain, gzip, zstd) are refused by magic bytes with "not supported;
// decompress/extract first", exit 1, and nothing is written.
#[tokio::test]
async fn l624_unsupported_compression_is_named() {
    use std::io::Write as _;
    let dir = tempfile::tempdir().unwrap();
    let doc = b"{\"_index\":\"x\",\"_id\":\"1\",\"_source\":{}}\n";
    let mut tar = vec![0u8; 512];
    tar[..6].copy_from_slice(b"d.json");
    tar[257..263].copy_from_slice(b"ustar\0");
    tar.extend_from_slice(doc);
    tar.resize(2048, 0);
    let mut gz = flate2::write::GzEncoder::new(Vec::new(), flate2::Compression::default());
    gz.write_all(&tar).unwrap();
    let tar_gz = gz.finish().unwrap();
    let tar_zst = zstd::encode_all(&tar[..], 3).unwrap();
    let cases: Vec<(&str, Vec<u8>, &str)> = vec![
        (
            "d.json.bz2",
            b"BZh91AY&SY\x00\x01".to_vec(),
            "bzip2-compressed",
        ),
        (
            "d.json.xz",
            b"\xfd7zXZ\x00\x00\x04".to_vec(),
            "xz-compressed",
        ),
        (
            "d.json.lzma",
            b"\x5d\x00\x00\x80\x00".to_vec(),
            "lzma-compressed",
        ),
        (
            "d.json.lz4",
            b"\x04\x22\x4d\x18\x64".to_vec(),
            "lz4-compressed",
        ),
        ("d.zip", b"PK\x03\x04\x14\x00".to_vec(), "a zip archive"),
        ("d.7z", b"7z\xbc\xaf\x27\x1c\x00".to_vec(), "a 7z archive"),
        ("d.tar", tar.clone(), "a tar archive"),
        ("d.tar.gz", tar_gz, "a gzip-compressed tar archive"),
        ("d.tar.zst", tar_zst, "a zstd-compressed tar archive"),
    ];
    for (name, bytes, kind) in cases {
        let input = dir.path().join(name);
        std::fs::write(&input, bytes).unwrap();
        let out = dir.path().join(format!("{name}.out"));
        let o = run_async(
            s(&[
                "--input",
                input.to_str().unwrap(),
                "--output",
                out.to_str().unwrap(),
            ]),
            vec![],
        )
        .await;
        let err = stderr(&o);
        assert_eq!(o.status.code(), Some(1), "{name}: {err}");
        assert!(
            err.contains(&format!("is {kind}, which is not supported")),
            "{name}: {err}"
        );
        assert!(!out.exists(), "{name}: output written");
    }
}

// L-625 (TST-BRK-IN-006): invalid UTF-8 inside a string fails naming the document (Node would
// silently write U+FFFD); UTF-16 LE/BE files (with or without a BOM) are refused up front with
// an iconv hint; a UTF-8 BOM at the start of the file or of a later line (cat of two dumps) is
// tolerated. A refused input never commits the output file.
#[tokio::test]
async fn l625_text_encoding_problems_are_named() {
    let dir = tempfile::tempdir().unwrap();
    let l = r#"{"_index":"x","_id":"1","_source":{"s":"ok"}}"#;
    let utf16 = |be: bool, bom: bool| {
        let mut v: Vec<u8> = Vec::new();
        if bom {
            v.extend(if be { [0xfe, 0xff] } else { [0xff, 0xfe] });
        }
        for u in format!("{l}\n").encode_utf16() {
            v.extend(if be { u.to_be_bytes() } else { u.to_le_bytes() });
        }
        v
    };
    let mut bad = format!("{l}\n").into_bytes();
    bad.extend_from_slice(
        b"{\"_index\":\"x\",\"_id\":\"2\",\"_source\":{\"s\":\"bad\xff\xfe\"}}\n",
    );
    let bom_mid = format!("\u{feff}{l}\n\u{feff}{l}\n").into_bytes();
    let cases: Vec<(&str, Vec<u8>, Option<&str>)> = vec![
        (
            "bad",
            bad,
            Some("document 2 of the input is not valid UTF-8"),
        ),
        ("le", utf16(false, true), Some("is UTF-16LE text")),
        ("be", utf16(true, true), Some("is UTF-16BE text")),
        ("le-nobom", utf16(false, false), Some("is UTF-16LE text")),
        ("be-nobom", utf16(true, false), Some("is UTF-16BE text")),
        ("bom-mid", bom_mid, None),
    ];
    for (name, bytes, want) in cases {
        let input = dir.path().join(format!("{name}.json"));
        std::fs::write(&input, bytes).unwrap();
        let out = dir.path().join(format!("{name}.out"));
        let o = run_async(
            s(&[
                "--input",
                input.to_str().unwrap(),
                "--output",
                out.to_str().unwrap(),
            ]),
            vec![],
        )
        .await;
        let err = stderr(&o);
        match want {
            Some(w) => {
                assert_eq!(o.status.code(), Some(1), "{name}: {err}");
                assert!(err.contains(w), "{name}: {err}");
                assert!(!out.exists(), "{name}: output committed");
            }
            None => {
                assert!(o.status.success(), "{name}: {err}");
                assert_eq!(
                    std::fs::read_to_string(&out).unwrap(),
                    format!("{l}\n{l}\n")
                );
            }
        }
    }
}

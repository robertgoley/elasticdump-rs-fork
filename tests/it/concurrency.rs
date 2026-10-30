//! M5: concurrent writes, throttling, the interval limiter, retries under
//! injected network faults, and `--searchWithTemplate` / `--format human`.

use std::time::{Duration, Instant};

use serde_json::{Value, json};

use crate::harness::*;
use crate::support::fault_proxy::{Dir, FaultProxy, Policy};

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

async fn dump_to_file(c: &Cluster, index: &str) -> std::path::PathBuf {
    let out = tmp_path("cdump", "json");
    assert_success(&run(io_args(c, &c.url_with_auth(index), out.to_str().unwrap())).await);
    out
}

#[tokio::test]
async fn es_to_es_with_concurrency_8_is_complete() {
    let c = Cluster::from_env();
    let src = unique("conc_src");
    let dst = unique("conc_dst");
    seed_index(&c, &src, 8000).await;
    c.copy_shape(&src, &dst).await;
    let mut a = io_args(&c, &c.url_with_auth(&src), &c.url_with_auth(&dst));
    a.extend(s(&["--concurrency", "8", "--limit", "200"]));
    assert_success(&run(a).await);
    assert_eq!(c.docs(&src).await, c.docs(&dst).await);
    c.delete_index(&src).await;
    c.delete_index(&dst).await;
}

#[tokio::test]
async fn interval_cap_and_throttle_timing() {
    let c = Cluster::from_env();
    let src = unique("lim_src");
    seed_index(&c, &src, 196).await; // 200 docs with the adversarial ones
    let file = dump_to_file(&c, &src).await;

    // 1 info request + 10 bulk requests, at most 2 starts per 1 s window:
    // the 11th request starts in window 5 → ≥ 5 s (assert ≥ 4 s, plan ±10% slack).
    let dst = unique("lim_dst");
    c.copy_shape(&src, &dst).await;
    let mut a = io_args(&c, file.to_str().unwrap(), &c.url_with_auth(&dst));
    a.extend(s(&[
        "--limit",
        "20",
        "--noRefresh",
        "--intervalCap",
        "2",
        "--concurrencyInterval",
        "1000",
    ]));
    let t = Instant::now();
    assert_success(&run(a).await);
    let el = t.elapsed();
    assert!(
        el >= Duration::from_secs(4) && el < Duration::from_secs(8),
        "{el:?}"
    );
    c.ok("POST", &format!("{dst}/_refresh"), None).await;
    assert_eq!(c.count(&dst).await, 200);

    // --throttleInterval 300 over 10 batches → ≥ 3 s
    let dst2 = unique("thr_dst");
    c.copy_shape(&src, &dst2).await;
    let mut a = io_args(&c, file.to_str().unwrap(), &c.url_with_auth(&dst2));
    a.extend(s(&["--limit", "20", "--throttleInterval", "300"]));
    let t = Instant::now();
    assert_success(&run(a).await);
    let el = t.elapsed();
    assert!(
        el >= Duration::from_millis(2700) && el < Duration::from_secs(6),
        "{el:?}"
    );
    assert_eq!(c.count(&dst2).await, 200);
    for i in [&src, &dst, &dst2] {
        c.delete_index(i).await;
    }
}

/// The original fault pattern on the shared proxy (tests/support/fault_proxy.rs): every
/// `refuse_every`-th connection is closed immediately; every other connection is cut after
/// forwarding a pseudo-random number of response bytes (small for every `cut_every`-th
/// connection, up to ~64 KiB otherwise), so pooled keep-alive connections keep dying
/// mid-response throughout the run.
async fn fault_proxy(upstream: String, cut_every: u64, refuse_every: u64) -> FaultProxy {
    let policy = Policy::new()
        .refuse_every(refuse_every)
        .cut_with(Dir::Down, move |n| {
            Some(if n % cut_every == 0 {
                64 + (n * 7919) % 2048
            } else {
                4096 + (n * 104_729) % 61_440
            })
        });
    FaultProxy::start(&upstream, policy).await
}

#[tokio::test]
async fn restore_through_fault_injection_20_of_20() {
    let c = Cluster::from_env();
    let src = unique("flt_src");
    seed_index(&c, &src, 1500).await;
    let file = dump_to_file(&c, &src).await;
    let want = c.docs(&src).await;
    let upstream = c
        .url
        .split("://")
        .nth(1)
        .unwrap()
        .trim_end_matches('/')
        .to_string();
    let scheme = c.url.split("://").next().unwrap().to_string();
    let mut total_faults = 0;
    for run_no in 0..20u64 {
        let proxy = fault_proxy(upstream.clone(), 3 + run_no % 3, 5 + run_no % 4).await;
        let dst = unique("flt_dst");
        c.copy_shape(&src, &dst).await;
        let base = format!("{scheme}://localhost:{}", proxy.port);
        let mut target = url::Url::parse(&format!("{base}/{dst}")).unwrap();
        if let Some(u) = env("IT_ADMIN_USER") {
            target.set_username(&u).unwrap();
            target.set_password(env("IT_ADMIN_PW").as_deref()).unwrap();
        }
        let mut a = io_args(&c, file.to_str().unwrap(), target.as_str());
        a.extend(s(&[
            "--limit",
            "100",
            "--concurrency",
            "4",
            // Every proxied connection is cut sooner or later, so roughly half
            // of all attempts fail; on TLS with ES 7 (larger bulk responses,
            // `_type` per item) 9 attempts lost one bulk in ~1 of 3 runs.
            "--retryAttempts",
            "25",
            "--retryDelay",
            "20",
            "--maxSockets",
            "2",
        ]));
        let o = run(a).await;
        assert_success(&o);
        total_faults += proxy.faults();
        // explicit ids: retried bulks never duplicate
        c.ok("POST", &format!("{dst}/_refresh"), None).await;
        assert_eq!(c.count(&dst).await, want.len() as u64, "run {run_no}");
        assert_eq!(c.docs(&dst).await, want, "run {run_no}");
        c.delete_index(&dst).await;
    }
    assert!(total_faults >= 40, "faults injected: {total_faults}");
    eprintln!("faults injected over 20 runs: {total_faults}");
    c.delete_index(&src).await;
}

#[tokio::test]
async fn search_with_template_and_human_format() {
    let c = Cluster::from_env();
    let src = unique("tpl_src");
    seed_index(&c, &src, 50).await;
    let out = tmp_path("tpl", "json");
    let body = json!({
        "source": {"query": {"term": {"k": "{{key}}"}}},
        "params": {"key": "key7"}
    });
    let mut a = io_args(&c, &c.url_with_auth(&src), out.to_str().unwrap());
    a.extend(s(&[
        "--searchWithTemplate",
        "--searchBody",
        &body.to_string(),
        "--format",
        "human",
    ]));
    assert_success(&run(a).await);
    let text = std::fs::read_to_string(&out).unwrap();
    assert!(text.lines().count() > 3, "pretty-printed: {text}");
    let docs: Vec<Value> = serde_json::Deserializer::from_str(&text)
        .into_iter::<Value>()
        .map(Result::unwrap)
        .collect();
    assert_eq!(docs.len(), 1);
    assert_eq!(docs[0]["_id"], "7");
    // human output is still restorable
    let dst = unique("tpl_dst");
    c.copy_shape(&src, &dst).await;
    assert_success(&run(io_args(&c, out.to_str().unwrap(), &c.url_with_auth(&dst))).await);
    assert_eq!(c.count(&dst).await, 1);
    // template without id/source is a usage error
    let mut a = io_args(
        &c,
        &c.url_with_auth(&src),
        tmp_path("tpl2", "json").to_str().unwrap(),
    );
    a.extend(s(&[
        "--searchWithTemplate",
        "--searchBody",
        "{\"query\":{}}",
    ]));
    let o = run(a).await;
    assert_eq!(o.status.code(), Some(2));
    c.delete_index(&src).await;
    c.delete_index(&dst).await;
}

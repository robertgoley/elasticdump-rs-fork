//! Fault conformance suite (TST-INF-004): the in-process proxy (`tests/support/fault_proxy.rs`)
//! and, when `TOXIPROXY_URL` is set (`scripts/it/toxiproxy.sh up`), the toxiproxy container must
//! show the same behaviour for every policy both can express. Policies only the in-process proxy
//! has (refuse-every-N, half-close, RST cuts) are checked for it alone.
//!
//! The upstream is a tiny line protocol server: `GET n\n` answers n pattern bytes;
//! `PUT n\n<n bytes>` answers `OK <n>\n`. It records how many bytes each connection delivered.

mod support;

use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

use support::fault_proxy::{Dir, FaultProxy, Policy, toxiproxy::Toxiproxy};
use tokio::io::{AsyncBufReadExt, AsyncReadExt, AsyncWriteExt, BufReader};
use tokio::net::{TcpListener, TcpStream};

fn pattern(i: usize) -> u8 {
    (i % 251) as u8
}

struct Upstream {
    addr: String,
    received: Arc<Mutex<Vec<u64>>>,
    _h: tokio::task::JoinHandle<()>,
}

async fn upstream() -> Upstream {
    let l = TcpListener::bind("127.0.0.1:0").await.unwrap();
    let addr = l.local_addr().unwrap().to_string();
    let received = Arc::new(Mutex::new(Vec::new()));
    let r2 = Arc::clone(&received);
    let h = tokio::spawn(async move {
        loop {
            let Ok((s, _)) = l.accept().await else { return };
            let rec = Arc::clone(&r2);
            tokio::spawn(async move {
                let idx = {
                    let mut g = rec.lock().unwrap();
                    g.push(0);
                    g.len() - 1
                };
                let (r, mut w) = s.into_split();
                let mut r = BufReader::new(r);
                loop {
                    let mut line = String::new();
                    match r.read_line(&mut line).await {
                        Ok(0) | Err(_) => return,
                        Ok(k) => rec.lock().unwrap()[idx] += k as u64,
                    }
                    let mut it = line.split_whitespace();
                    let (cmd, n) = (
                        it.next().unwrap_or(""),
                        it.next().and_then(|x| x.parse::<usize>().ok()).unwrap_or(0),
                    );
                    match cmd {
                        "GET" => {
                            let body: Vec<u8> = (0..n).map(pattern).collect();
                            if w.write_all(&body).await.is_err() {
                                return;
                            }
                        }
                        "PUT" => {
                            let mut buf = vec![0u8; 4096];
                            let mut got = 0;
                            while got < n {
                                match r.read(&mut buf[..(n - got).min(4096)]).await {
                                    Ok(0) | Err(_) => return,
                                    Ok(k) => {
                                        got += k;
                                        rec.lock().unwrap()[idx] += k as u64;
                                    }
                                }
                            }
                            if w.write_all(format!("OK {got}\n").as_bytes()).await.is_err() {
                                return;
                            }
                        }
                        _ => return,
                    }
                }
            });
        }
    });
    Upstream {
        addr,
        received,
        _h: h,
    }
}

/// A started proxy of either kind.
enum Started {
    InProcess(FaultProxy),
    Toxi(support::fault_proxy::toxiproxy::Proxy),
}

impl Started {
    fn addr(&self) -> String {
        match self {
            Started::InProcess(p) => p.addr(),
            Started::Toxi(p) => p.listen.clone(),
        }
    }
    async fn stop(self) {
        if let Started::Toxi(p) = self {
            p.delete().await;
        }
    }
}

#[derive(Clone, Copy, Debug)]
enum Kind {
    InProcess,
    Toxiproxy,
}

fn kinds() -> Vec<Kind> {
    let mut k = vec![Kind::InProcess];
    if Toxiproxy::from_env().is_some() {
        k.push(Kind::Toxiproxy);
    } else {
        eprintln!("SKIP: toxiproxy leg (TOXIPROXY_URL not set; scripts/it/toxiproxy.sh up)");
    }
    k
}

/// `slot` gives each test its own toxiproxy listen port (nextest runs tests in separate processes).
async fn start(kind: Kind, up: &str, policy: Policy, name: &str, slot: u16) -> Started {
    match kind {
        Kind::InProcess => Started::InProcess(FaultProxy::start(up, policy).await),
        Kind::Toxiproxy => {
            let base: u16 = std::env::var("TOXIPROXY_LISTEN_BASE")
                .ok()
                .and_then(|v| v.parse().ok())
                .unwrap_or(19281);
            let port = base + slot;
            let t = Toxiproxy::from_env().unwrap();
            Started::Toxi(
                t.proxy(name, &format!("127.0.0.1:{port}"), up, &policy)
                    .await
                    .expect("toxiproxy proxy"),
            )
        }
    }
}

async fn get(addr: &str, n: usize) -> std::io::Result<Vec<u8>> {
    let mut s = TcpStream::connect(addr).await?;
    s.write_all(format!("GET {n}\n").as_bytes()).await?;
    let mut out = Vec::new();
    let mut buf = vec![0u8; 8192];
    while out.len() < n {
        match tokio::time::timeout(Duration::from_secs(20), s.read(&mut buf)).await {
            Ok(Ok(0)) | Err(_) => break,
            Ok(Ok(k)) => out.extend_from_slice(&buf[..k]),
            Ok(Err(e)) => return if out.is_empty() { Err(e) } else { Ok(out) },
        }
    }
    Ok(out)
}

fn intact(b: &[u8]) -> bool {
    b.iter().enumerate().all(|(i, x)| *x == pattern(i))
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn conformance_passthrough_is_byte_exact() {
    let u = upstream().await;
    for k in kinds() {
        let p = start(k, &u.addr, Policy::new(), "conf_pass", 0).await;
        let b = get(&p.addr(), 200_000).await.unwrap();
        assert_eq!(b.len(), 200_000, "{k:?}");
        assert!(intact(&b), "{k:?}");
        p.stop().await;
    }
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn conformance_cut_down_after_bytes() {
    let u = upstream().await;
    for k in kinds() {
        let p = start(
            k,
            &u.addr,
            Policy::parse("cut_down=10000").unwrap(),
            "conf_cutdown",
            1,
        )
        .await;
        let b = get(&p.addr(), 200_000).await.unwrap_or_default();
        assert_eq!(
            b.len(),
            10_000,
            "{k:?}: client must see exactly the first 10000 bytes then EOF"
        );
        assert!(intact(&b), "{k:?}");
        p.stop().await;
    }
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn conformance_cut_up_after_bytes() {
    let u = upstream().await;
    for k in kinds() {
        let before = u.received.lock().unwrap().len();
        let p = start(
            k,
            &u.addr,
            Policy::new().cut_after(Dir::Up, 5000),
            "conf_cutup",
            2,
        )
        .await;
        let mut s = TcpStream::connect(p.addr()).await.unwrap();
        let _ = s.write_all(b"PUT 20000\n").await;
        let _ = s.write_all(&[7u8; 20_000]).await;
        let mut buf = [0u8; 64];
        let r = tokio::time::timeout(Duration::from_secs(10), s.read(&mut buf)).await;
        assert!(
            matches!(r, Ok(Ok(0)) | Ok(Err(_))),
            "{k:?}: expected EOF/reset, got {r:?}"
        );
        tokio::time::sleep(Duration::from_millis(200)).await;
        let got: u64 = u.received.lock().unwrap()[before..].iter().sum();
        assert_eq!(got, 5000, "{k:?}: upstream must receive exactly 5000 bytes");
        p.stop().await;
    }
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn conformance_latency_down() {
    let u = upstream().await;
    for k in kinds() {
        let p = start(
            k,
            &u.addr,
            Policy::parse("latency_down=300").unwrap(),
            "conf_lat",
            3,
        )
        .await;
        let t = Instant::now();
        let b = get(&p.addr(), 10).await.unwrap();
        let e = t.elapsed();
        assert_eq!(b.len(), 10);
        assert!(
            e >= Duration::from_millis(290) && e < Duration::from_secs(3),
            "{k:?}: {e:?}"
        );
        p.stop().await;
    }
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn conformance_bandwidth_down() {
    let u = upstream().await;
    for k in kinds() {
        let p = start(
            k,
            &u.addr,
            Policy::parse("bw_down=256").unwrap(),
            "conf_bw",
            4,
        )
        .await;
        let t = Instant::now();
        let b = get(&p.addr(), 1 << 20).await.unwrap();
        let e = t.elapsed();
        assert_eq!(b.len(), 1 << 20, "{k:?}");
        assert!(intact(&b), "{k:?}");
        // 1 MiB at 256 KiB/s (or 256 kB/s for toxiproxy) = 4.0-4.1 s
        assert!(
            e >= Duration::from_secs(3) && e < Duration::from_secs(8),
            "{k:?}: {e:?}"
        );
        p.stop().await;
    }
}

// ---- in-process-only policies ----

#[tokio::test]
async fn inprocess_refuse_every_n() {
    let u = upstream().await;
    let p = FaultProxy::start(&u.addr, Policy::parse("refuse_every=2").unwrap()).await;
    assert_eq!(get(&p.addr(), 100).await.unwrap().len(), 100);
    assert!(
        get(&p.addr(), 100)
            .await
            .map(|b| b.is_empty())
            .unwrap_or(true),
        "2nd connection refused"
    );
    assert_eq!(get(&p.addr(), 100).await.unwrap().len(), 100);
    assert_eq!(p.faults(), 1);
    assert_eq!(p.connections(), 3);
}

#[tokio::test]
async fn inprocess_half_close_down_keeps_the_request_direction_open() {
    let u = upstream().await;
    let p = FaultProxy::start(&u.addr, Policy::new().half_close(Dir::Down, 1000)).await;
    let mut s = TcpStream::connect(p.addr()).await.unwrap();
    s.write_all(b"GET 5000\n").await.unwrap();
    let mut got = Vec::new();
    s.read_to_end(&mut got).await.unwrap();
    assert_eq!(got.len(), 1000, "response direction FIN after 1000 bytes");
    // the request direction still works: upstream receives a following PUT
    s.write_all(b"PUT 3\nabc").await.unwrap();
    tokio::time::sleep(Duration::from_millis(200)).await;
    let total: u64 = u.received.lock().unwrap().iter().sum();
    assert_eq!(total, (b"GET 5000\n".len() + b"PUT 3\n".len() + 3) as u64);
    assert_eq!(p.faults(), 1);
}

#[tokio::test]
async fn inprocess_reset_cut_sends_rst() {
    let u = upstream().await;
    let p = FaultProxy::start(&u.addr, Policy::parse("cut_down=1000;reset").unwrap()).await;
    let mut s = TcpStream::connect(p.addr()).await.unwrap();
    s.write_all(b"GET 100000\n").await.unwrap();
    let mut buf = vec![0u8; 200_000];
    let mut n = 0;
    let err = loop {
        match s.read(&mut buf[n..]).await {
            Ok(0) => break None,
            Ok(k) => n += k,
            Err(e) => break Some(e.kind()),
        }
    };
    assert!(n <= 1000, "{n}");
    assert_eq!(
        err,
        Some(std::io::ErrorKind::ConnectionReset),
        "read {n} bytes"
    );
}

#[test]
fn policy_dsl_parses_and_rejects() {
    assert!(
        Policy::parse(
            "refuse_every=5;cut_down=4096,latency_up=20;bw_down=64;half_close_up=10;reset"
        )
        .is_ok()
    );
    assert!(Policy::parse("").is_ok());
    for bad in ["cut_sideways=1", "latency_down=fast", "explode=1"] {
        assert!(Policy::parse(bad).is_err(), "{bad}");
    }
}

//! SIGINT and SIGTERM (TST-BRK-SIG-001). [`install`] runs once per process:
//! the first signal cancels [`token`] so a running transfer can stop in order
//! (close the scroll/PIT, remove staged output) and the run exits with
//! 128 + the signal number (130 / 143). A second signal, or a signal while no
//! transfer is listening ([`graceful`]), exits at once with that status.

use std::sync::OnceLock;
use std::sync::atomic::{AtomicI32, AtomicUsize, Ordering};

use tokio_util::sync::CancellationToken;

static TOKEN: OnceLock<CancellationToken> = OnceLock::new();
static RECEIVED: AtomicI32 = AtomicI32::new(0);
static GRACEFUL: AtomicUsize = AtomicUsize::new(0);

pub const SIGINT: i32 = 2;
pub const SIGTERM: i32 = 15;

/// The process-wide cancellation fired by the first signal.
pub fn token() -> CancellationToken {
    TOKEN.get_or_init(CancellationToken::new).clone()
}

/// Signal number received, if any.
pub fn received() -> Option<i32> {
    match RECEIVED.load(Ordering::SeqCst) {
        0 => None,
        n => Some(n),
    }
}

fn name(sig: i32) -> &'static str {
    match sig {
        SIGTERM => "SIGTERM",
        _ => "SIGINT",
    }
}

/// The run was stopped by a signal; exit status 128 + `0`.
#[derive(Debug)]
pub struct Interrupted(pub i32);

impl std::fmt::Display for Interrupted {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "dump interrupted by user ({})", name(self.0))
    }
}

impl std::error::Error for Interrupted {}

/// The error for a run stopped by the signal received.
pub fn interrupted_error() -> anyhow::Error {
    anyhow::Error::new(Interrupted(received().unwrap_or(SIGINT)))
}

/// While the guard lives, a first signal cancels [`token`] instead of exiting.
pub struct Graceful(());

pub fn graceful() -> Graceful {
    GRACEFUL.fetch_add(1, Ordering::SeqCst);
    Graceful(())
}

impl Drop for Graceful {
    fn drop(&mut self) {
        GRACEFUL.fetch_sub(1, Ordering::SeqCst);
    }
}

async fn next_signal() -> i32 {
    #[cfg(unix)]
    {
        use tokio::signal::unix::{SignalKind, signal};
        match signal(SignalKind::terminate()) {
            Ok(mut term) => tokio::select! {
                r = tokio::signal::ctrl_c() => if r.is_ok() { SIGINT } else { std::future::pending().await },
                _ = term.recv() => SIGTERM,
            },
            Err(_) => {
                let _ = tokio::signal::ctrl_c().await;
                SIGINT
            }
        }
    }
    #[cfg(not(unix))]
    {
        let _ = tokio::signal::ctrl_c().await;
        SIGINT
    }
}

/// Start listening (idempotent).
pub fn install() {
    static STARTED: OnceLock<()> = OnceLock::new();
    if STARTED.set(()).is_err() {
        return;
    }
    let token = token();
    tokio::spawn(async move {
        let sig = next_signal().await;
        RECEIVED.store(sig, Ordering::SeqCst);
        if GRACEFUL.load(Ordering::SeqCst) == 0 {
            eprintln!("Error: dump interrupted by user ({})", name(sig));
            std::process::exit(128 + sig);
        }
        log::warn!(
            "{} received; cancelling and cleaning up (send it again to force-quit)",
            name(sig)
        );
        token.cancel();
        let again = next_signal().await;
        std::process::exit(128 + again);
    });
}

//! SIGINT and SIGTERM (TST-BRK-SIG-001). [`install`] runs once per process:
//! the first signal cancels [`token`] so a running transfer can stop in order
//! (close the scroll/PIT, remove staged output) and the run exits with
//! 128 + the signal number (130 / 143). A second signal, or a signal while no
//! transfer is listening ([`graceful`]), exits at once with that status.
//! Before such a forced exit, every [`note_on_force_quit`] still registered is
//! printed, so what the run could not clean up (a closed index, staged output)
//! is named (TST-BRK-SIG-004).

use std::sync::atomic::{AtomicI32, AtomicU64, AtomicUsize, Ordering};
use std::sync::{Mutex, OnceLock};

use tokio_util::sync::CancellationToken;

static TOKEN: OnceLock<CancellationToken> = OnceLock::new();
static RECEIVED: AtomicI32 = AtomicI32::new(0);
static GRACEFUL: AtomicUsize = AtomicUsize::new(0);
static NOTES: Mutex<Vec<(u64, String)>> = Mutex::new(Vec::new());
static NEXT_NOTE: AtomicU64 = AtomicU64::new(0);

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

/// How an interrupted restore can be picked up again (L-647, TST-BRK-SIG-002).
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Resume {
    /// Rerun with `--offset offset` (and `--size size` when the run had one).
    /// `exact` is false for a cluster input, whose order is only stable while
    /// the index does not change.
    Offset {
        offset: u64,
        size: Option<u64>,
        exact: bool,
    },
    /// `--offset` cannot resume this run; the reason says what to do instead.
    NotByOffset(&'static str),
}

/// A restore into a cluster stopped by a signal AFTER its in-flight bulk
/// requests finished, so everything before the resume point is written (or
/// counted as failed). Exit status 128 + the signal, like [`Interrupted`].
#[derive(Debug)]
pub struct Stopped {
    pub sig: i32,
    pub written: u64,
    pub failed: u64,
    pub overwritten: u64,
    pub resume: Resume,
}

impl std::fmt::Display for Stopped {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(
            f,
            "dump interrupted by user ({}) after the in-flight bulk requests finished; {} document(s) written",
            name(self.sig),
            self.written
        )?;
        match &self.resume {
            Resume::Offset {
                offset,
                size,
                exact,
            } => {
                write!(
                    f,
                    "; to resume, rerun the same command with --offset {offset}"
                )?;
                if let Some(size) = size {
                    write!(f, " --size {size}")?;
                }
                if !exact {
                    write!(
                        f,
                        " (exact only while the source index is unchanged and its search order is stable, e.g. a sort in --searchBody)"
                    )?;
                }
            }
            Resume::NotByOffset(why) => write!(f, "; {why}")?,
        }
        if self.failed > 0 {
            write!(
                f,
                "; {} document(s) before that point failed and are not retried by a resumed run (see the errors above)",
                self.failed
            )?;
        }
        Ok(())
    }
}

impl std::error::Error for Stopped {}

/// The [`Stopped`] error for the signal received.
pub fn stopped_error(written: u64, failed: u64, overwritten: u64, resume: Resume) -> anyhow::Error {
    anyhow::Error::new(Stopped {
        sig: received().unwrap_or(SIGINT),
        written,
        failed,
        overwritten,
        resume,
    })
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

/// While the guard lives, a forced exit (a second signal, or a signal no
/// transfer listens for) prints `msg` first: what the run leaves behind
/// when it cannot finish its cleanup.
pub struct ForceQuitNote(u64);

pub fn note_on_force_quit(msg: impl Into<String>) -> ForceQuitNote {
    let id = NEXT_NOTE.fetch_add(1, Ordering::SeqCst);
    NOTES
        .lock()
        .unwrap_or_else(|e| e.into_inner())
        .push((id, msg.into()));
    ForceQuitNote(id)
}

impl Drop for ForceQuitNote {
    fn drop(&mut self) {
        NOTES
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .retain(|(id, _)| *id != self.0);
    }
}

/// The notes registered now, oldest first.
fn force_quit_notes() -> Vec<String> {
    NOTES
        .lock()
        .unwrap_or_else(|e| e.into_inner())
        .iter()
        .map(|(_, m)| m.clone())
        .collect()
}

fn force_quit(sig: i32, during_cleanup: bool) -> ! {
    if during_cleanup {
        eprintln!(
            "Error: force-quit by user ({}) before the cleanup finished",
            name(sig)
        );
    } else {
        eprintln!("Error: dump interrupted by user ({})", name(sig));
    }
    for note in force_quit_notes() {
        eprintln!("Error: {note}");
    }
    std::process::exit(128 + sig);
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
            force_quit(sig, false);
        }
        log::warn!(
            "{} received; cancelling and cleaning up (send it again to force-quit)",
            name(sig)
        );
        token.cancel();
        let again = next_signal().await;
        force_quit(again, true);
    });
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn force_quit_notes_live_as_long_as_their_guard() {
        let a = note_on_force_quit("FAKE-note-a");
        let b = note_on_force_quit("FAKE-note-b");
        let mine = |v: Vec<String>| -> Vec<String> {
            v.into_iter()
                .filter(|m| m.starts_with("FAKE-note-"))
                .collect()
        };
        assert_eq!(mine(force_quit_notes()), ["FAKE-note-a", "FAKE-note-b"]);
        drop(a);
        assert_eq!(mine(force_quit_notes()), ["FAKE-note-b"]);
        drop(b);
        assert!(mine(force_quit_notes()).is_empty());
    }

    #[test]
    fn stopped_messages_name_the_resume_offset() {
        let m = Stopped {
            sig: SIGINT,
            written: 40,
            failed: 0,
            overwritten: 0,
            resume: Resume::Offset {
                offset: 50,
                size: None,
                exact: true,
            },
        }
        .to_string();
        assert_eq!(
            m,
            "dump interrupted by user (SIGINT) after the in-flight bulk requests finished; 40 document(s) written; to resume, rerun the same command with --offset 50"
        );
        let m = Stopped {
            sig: SIGTERM,
            written: 7,
            failed: 2,
            overwritten: 0,
            resume: Resume::Offset {
                offset: 9,
                size: Some(91),
                exact: false,
            },
        }
        .to_string();
        assert!(m.contains("(SIGTERM)"), "{m}");
        assert!(m.contains("--offset 9 --size 91 (exact only while"), "{m}");
        assert!(m.contains("2 document(s) before that point failed"), "{m}");
        let m = Stopped {
            sig: SIGINT,
            written: 3,
            failed: 0,
            overwritten: 0,
            resume: Resume::NotByOffset("a sliced dump cannot resume with --offset"),
        }
        .to_string();
        assert!(
            m.ends_with("; a sliced dump cannot resume with --offset"),
            "{m}"
        );
        assert!(!m.contains("rerun the same command"), "{m}");
    }
}

//! Request admission control shared by every HTTP request of a run:
//!
//! * `--maxSockets`: at most N requests in flight (semaphore).
//! * `--intervalCap` / `--concurrencyInterval` / `--carryoverConcurrencyCount`:
//!   p-queue style fixed-window rate limiting. At most `cap` requests may start
//!   per window; with carry-over, requests still in flight when a window ends
//!   count against the next one.

use std::sync::{Arc, Mutex};
use std::time::Duration;

use tokio::sync::{OwnedSemaphorePermit, Semaphore};
use tokio::time::Instant;

#[derive(Debug)]
struct WindowState {
    window_start: Instant,
    started: u64,
    in_flight: u64,
}

#[derive(Debug)]
pub struct IntervalLimiter {
    cap: u64,
    interval: Duration,
    carryover: bool,
    state: Mutex<WindowState>,
}

impl IntervalLimiter {
    pub fn new(cap: u64, interval: Duration, carryover: bool) -> Self {
        Self {
            cap: cap.max(1),
            interval: interval.max(Duration::from_millis(1)),
            carryover,
            state: Mutex::new(WindowState {
                window_start: Instant::now(),
                started: 0,
                in_flight: 0,
            }),
        }
    }

    pub async fn acquire(self: &Arc<Self>) -> IntervalGuard {
        loop {
            let wait = {
                let mut st = self.state.lock().unwrap();
                let now = Instant::now();
                let elapsed = now.saturating_duration_since(st.window_start);
                if elapsed >= self.interval {
                    // Align to the window grid so bursts cannot drift.
                    let windows = elapsed.as_nanos() / self.interval.as_nanos();
                    st.window_start += self.interval * windows as u32;
                    st.started = if self.carryover { st.in_flight } else { 0 };
                }
                if st.started < self.cap {
                    st.started += 1;
                    st.in_flight += 1;
                    return IntervalGuard {
                        limiter: Arc::clone(self),
                    };
                }
                (st.window_start + self.interval).saturating_duration_since(now)
            };
            tokio::time::sleep(wait.max(Duration::from_millis(1))).await;
        }
    }
}

pub struct IntervalGuard {
    limiter: Arc<IntervalLimiter>,
}

impl Drop for IntervalGuard {
    fn drop(&mut self) {
        let mut st = self.limiter.state.lock().unwrap();
        st.in_flight = st.in_flight.saturating_sub(1);
    }
}

/// Admission gate applied around every HTTP request.
#[derive(Debug, Default, Clone)]
pub struct RequestGate {
    sockets: Option<Arc<Semaphore>>,
    interval: Option<Arc<IntervalLimiter>>,
}

pub struct GatePermit {
    _socket: Option<OwnedSemaphorePermit>,
    _interval: Option<IntervalGuard>,
}

impl RequestGate {
    pub fn new(max_sockets: Option<usize>, interval: Option<IntervalLimiter>) -> Self {
        Self {
            sockets: max_sockets
                .map(|n| Arc::new(Semaphore::new(n.clamp(1, Semaphore::MAX_PERMITS)))),
            interval: interval.map(Arc::new),
        }
    }

    pub async fn acquire(&self) -> GatePermit {
        let socket = match &self.sockets {
            Some(s) => Some(
                Arc::clone(s)
                    .acquire_owned()
                    .await
                    .expect("semaphore never closed"),
            ),
            None => None,
        };
        let interval = match &self.interval {
            Some(l) => Some(l.acquire().await),
            None => None,
        };
        GatePermit {
            _socket: socket,
            _interval: interval,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[tokio::test(start_paused = true)]
    async fn interval_cap_admits_cap_per_window() {
        let l = Arc::new(IntervalLimiter::new(2, Duration::from_secs(1), false));
        let start = Instant::now();
        let mut guards = Vec::new();
        for _ in 0..6 {
            guards.push(l.acquire().await);
            guards.clear(); // complete immediately
        }
        // 6 requests at 2 per second: windows 0,1,2 -> last starts at t>=2s
        let elapsed = start.elapsed();
        assert!(elapsed >= Duration::from_secs(2), "{elapsed:?}");
        assert!(elapsed < Duration::from_secs(3), "{elapsed:?}");
    }

    #[tokio::test(start_paused = true)]
    async fn carryover_counts_in_flight_requests_against_next_window() {
        let l = Arc::new(IntervalLimiter::new(2, Duration::from_secs(1), true));
        let start = Instant::now();
        let g1 = l.acquire().await;
        let _g2 = l.acquire().await;
        // both still in flight; next window starts with started=2 -> blocked
        let l2 = Arc::clone(&l);
        let h = tokio::spawn(async move {
            let _g = l2.acquire().await;
            Instant::now()
        });
        tokio::time::sleep(Duration::from_millis(1500)).await;
        assert!(
            !h.is_finished(),
            "carry-over must block while requests are in flight"
        );
        drop(g1);
        let admitted = h.await.unwrap();
        // admitted at the window boundary after g1 completed (t = 2s)
        assert!(admitted.duration_since(start) >= Duration::from_secs(2));
    }

    #[tokio::test(start_paused = true)]
    async fn without_carryover_in_flight_requests_do_not_block_next_window() {
        let l = Arc::new(IntervalLimiter::new(1, Duration::from_secs(1), false));
        let start = Instant::now();
        let _g1 = l.acquire().await;
        let _g2 = l.acquire().await; // next window, g1 still in flight
        assert!(start.elapsed() >= Duration::from_secs(1));
        assert!(start.elapsed() < Duration::from_secs(2));
    }

    #[tokio::test]
    async fn max_sockets_limits_concurrency() {
        let gate = RequestGate::new(Some(1), None);
        let p1 = gate.acquire().await;
        let g2 = gate.clone();
        let h = tokio::spawn(async move {
            let _p = g2.acquire().await;
        });
        tokio::time::sleep(Duration::from_millis(50)).await;
        assert!(!h.is_finished());
        drop(p1);
        h.await.unwrap();
    }
}

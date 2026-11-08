//! Per-host request spacing.

use crate::{sleep_or_cancel, FetchError};
use std::collections::HashMap;
use std::sync::Mutex;
use std::time::Duration;
use tokio::time::Instant;
use tokio_util::sync::CancellationToken;

/// Hands out time slots per host: each caller reserves the next free slot
/// (`delay` after the previous reservation) and sleeps until it arrives.
/// Reservation is atomic, so concurrent tasks cannot burst past the limit.
#[derive(Default)]
pub struct DomainLimiter {
    next_slot: Mutex<HashMap<String, Instant>>,
}

impl DomainLimiter {
    pub async fn acquire(&self, host: &str, delay: Duration, cancel: &CancellationToken) -> Result<(), FetchError> {
        let wait = {
            let mut map = self.next_slot.lock().expect("limiter mutex poisoned");
            let now = Instant::now();
            let slot = map.get(host).copied().map_or(now, |t| t.max(now));
            map.insert(host.to_string(), slot + delay);
            slot.saturating_duration_since(now)
        };
        if wait.is_zero() {
            Ok(())
        } else {
            sleep_or_cancel(wait, cancel).await
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[tokio::test(start_paused = true)]
    async fn spaces_requests_per_host_but_not_across_hosts() {
        let l = DomainLimiter::default();
        let c = CancellationToken::new();
        let t0 = Instant::now();
        let d = Duration::from_millis(100);
        l.acquire("a", d, &c).await.unwrap();
        l.acquire("a", d, &c).await.unwrap();
        l.acquire("a", d, &c).await.unwrap();
        assert!(t0.elapsed() >= Duration::from_millis(200));
        let t1 = Instant::now();
        l.acquire("b", d, &c).await.unwrap();
        assert!(t1.elapsed() < Duration::from_millis(10));
    }

    #[tokio::test(start_paused = true)]
    async fn cancellation_interrupts_wait() {
        let l = DomainLimiter::default();
        let c = CancellationToken::new();
        l.acquire("a", Duration::from_secs(60), &c).await.unwrap();
        c.cancel();
        assert!(matches!(l.acquire("a", Duration::from_secs(60), &c).await, Err(FetchError::Cancelled)));
    }
}

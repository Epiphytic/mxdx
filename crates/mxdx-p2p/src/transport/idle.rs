//! Idle-timeout watchdog for the P2P session (T-52).
//!
//! Fires [`IdleTick`] when no I/O has occurred on the data channel for
//! the configured window (default 5 min per storm §3.4). The driver
//! resets the deadline via [`IdleWatchdog::reset`] on every outbound send
//! and every inbound message.
//!
//! Native-only — wasm gets a Phase-8 shim based on `gloo-timers`.

use std::time::Duration;

use tokio::sync::mpsc;

/// Marker emitted on the channel when the idle window elapses without
/// I/O. Driver listens and dispatches `Event::IdleTick`.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct IdleTick;

/// Control message sent by the driver to the watchdog task.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Control {
    Reset,
    Shutdown,
}

/// Handle to the spawned watchdog task. Drop to cancel.
pub struct IdleWatchdog {
    control_tx: mpsc::Sender<Control>,
    join: Option<tokio::task::JoinHandle<()>>,
}

impl IdleWatchdog {
    /// Spawn a new watchdog. Caller provides a tokio mpsc `Sender`
    /// through which the watchdog will push [`IdleTick`] when the window
    /// elapses without a reset.
    pub fn spawn(window: Duration, tick_tx: mpsc::Sender<IdleTick>) -> Self {
        let (control_tx, mut control_rx) = mpsc::channel::<Control>(8);

        let join = tokio::spawn(async move {
            loop {
                tokio::select! {
                    biased;

                    msg = control_rx.recv() => {
                        match msg {
                            Some(Control::Reset) => {
                                // Loop back and re-arm the sleep below.
                                continue;
                            }
                            Some(Control::Shutdown) | None => return,
                        }
                    }

                    _ = tokio::time::sleep(window) => {
                        // Fire and exit: driver will re-spawn the
                        // watchdog on the next Open transition.
                        let _ = tick_tx.send(IdleTick).await;
                        return;
                    }
                }
            }
        });

        Self {
            control_tx,
            join: Some(join),
        }
    }

    /// Reset the deadline. Called by the driver on every send and
    /// receive. Non-blocking; if the channel is full (bogus — depth 8),
    /// the reset is dropped and the watchdog fires eventually.
    pub fn reset(&self) {
        let _ = self.control_tx.try_send(Control::Reset);
    }

    /// Cancel the watchdog cleanly and await its exit.
    pub async fn shutdown(mut self) {
        let _ = self.control_tx.send(Control::Shutdown).await;
        if let Some(h) = self.join.take() {
            let _ = h.await;
        }
    }
}

impl Drop for IdleWatchdog {
    fn drop(&mut self) {
        // Best-effort cancellation.
        let _ = self.control_tx.try_send(Control::Shutdown);
        if let Some(h) = self.join.take() {
            h.abort();
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Give the spawned watchdog task a chance to run so it arms its
    /// `sleep`. Without this the virtual-clock `advance` call runs before
    /// the watchdog ever polls, and the fire is missed.
    async fn yield_many() {
        for _ in 0..8 {
            tokio::task::yield_now().await;
        }
    }

    #[tokio::test(start_paused = true)]
    async fn fires_after_window_with_no_io() {
        let (tx, mut rx) = mpsc::channel(1);
        let _wd = IdleWatchdog::spawn(Duration::from_secs(5), tx);
        yield_many().await;
        tokio::time::advance(Duration::from_secs(6)).await;
        yield_many().await;
        let got = rx.recv().await;
        assert!(matches!(got, Some(IdleTick)), "expected IdleTick, got {got:?}");
    }

    #[tokio::test(start_paused = true)]
    async fn reset_prevents_fire() {
        let (tx, mut rx) = mpsc::channel(1);
        let wd = IdleWatchdog::spawn(Duration::from_secs(5), tx);
        yield_many().await;

        tokio::time::advance(Duration::from_secs(3)).await;
        yield_many().await;
        wd.reset();
        yield_many().await;

        tokio::time::advance(Duration::from_secs(3)).await;
        yield_many().await;
        // Only 3s since reset — should NOT have fired.
        assert!(rx.try_recv().is_err());

        tokio::time::advance(Duration::from_secs(3)).await;
        yield_many().await;
        // Now 6s since reset — should have fired.
        assert!(matches!(rx.recv().await, Some(IdleTick)));
    }

    #[tokio::test(start_paused = true)]
    async fn shutdown_cancels_task() {
        let (tx, mut rx) = mpsc::channel(1);
        let wd = IdleWatchdog::spawn(Duration::from_secs(60), tx);
        yield_many().await;
        wd.shutdown().await;
        tokio::time::advance(Duration::from_secs(120)).await;
        yield_many().await;
        // Watchdog was shut down before the window elapsed — no tick.
        assert!(rx.try_recv().is_err());
    }
}

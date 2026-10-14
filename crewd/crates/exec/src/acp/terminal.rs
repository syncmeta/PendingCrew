//! Bounded native-byte observation. A stalled viewer cannot stall the PTY driver.
//! Lag is explicit and requires an authoritative snapshot; no silent screen splicing.
use super::wire::{MAX_TERMINAL_PAYLOAD, TerminalFrame, TerminalKind};
use std::sync::{
    Arc,
    atomic::{AtomicU64, Ordering},
};
use tokio::sync::broadcast;

/// Implementations must only enqueue bounded bytes, never do IO or wait for consumers.
pub trait TerminalObserver: Send + Sync {
    fn observe(&self, bytes: &[u8]);
    fn begin_incarnation(&self, _epoch: u64) {}
    fn begin_owned_spawn(&self) {}
    fn subscribe_output(&self) -> Option<broadcast::Receiver<TerminalFrame>> {
        None
    }
    fn cut(&self) -> Option<(u64, u64)> {
        None
    }
}
#[derive(Clone)]
pub struct TerminalTap {
    sender: broadcast::Sender<TerminalFrame>,
    epoch: Arc<AtomicU64>,
    sequence: Arc<AtomicU64>,
}
impl std::fmt::Debug for TerminalTap {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("TerminalTap")
            .field("epoch", &self.epoch())
            .finish_non_exhaustive()
    }
}
impl TerminalTap {
    pub fn new(epoch: u64, capacity: usize) -> Result<Self, super::wire::WireError> {
        if epoch == 0 || capacity == 0 || capacity > 64 {
            return Err(super::wire::WireError::Oversize);
        }
        let (sender, _) = broadcast::channel(capacity);
        Ok(Self {
            sender,
            epoch: Arc::new(AtomicU64::new(epoch)),
            sequence: Arc::new(AtomicU64::new(0)),
        })
    }
    pub fn epoch(&self) -> u64 {
        self.epoch.load(Ordering::Acquire)
    }
    pub fn subscribe(&self) -> broadcast::Receiver<TerminalFrame> {
        self.sender.subscribe()
    }
    /// Capture together with the driver's screen snapshot in its single owning task.
    pub fn sequence(&self) -> u64 {
        self.sequence.load(Ordering::Acquire)
    }
}
impl TerminalObserver for TerminalTap {
    fn begin_owned_spawn(&self) {
        if let Some(epoch) = self.epoch().checked_add(1) {
            self.begin_incarnation(epoch);
        }
    }
    fn begin_incarnation(&self, epoch: u64) {
        if epoch != 0 && epoch != self.epoch() {
            // The single owning PTY task calls this before reading any bytes
            // from a newly acknowledged native process. Old queued frames keep
            // their old epoch; a consumer cannot silently splice incarnations.
            self.sequence.store(0, Ordering::Release);
            self.epoch.store(epoch, Ordering::Release);
        }
    }
    fn subscribe_output(&self) -> Option<broadcast::Receiver<TerminalFrame>> {
        Some(self.subscribe())
    }
    fn cut(&self) -> Option<(u64, u64)> {
        Some((self.epoch(), self.sequence()))
    }
    fn observe(&self, bytes: &[u8]) {
        for chunk in bytes.chunks(MAX_TERMINAL_PAYLOAD) {
            let Ok(previous) =
                self.sequence
                    .fetch_update(Ordering::AcqRel, Ordering::Acquire, |v| v.checked_add(1))
            else {
                return;
            };
            let _ = self.sender.send(TerminalFrame {
                epoch: self.epoch(),
                sequence: previous + 1,
                kind: TerminalKind::Data,
                bytes: chunk.to_vec(),
            });
        }
    }
}
#[cfg(test)]
mod tests {
    use super::*;
    #[tokio::test]
    async fn backpressure_is_bounded_and_loss_explicit() {
        let tap = TerminalTap::new(7, 2).unwrap();
        let mut stalled = tap.subscribe();
        for _ in 0..100 {
            tap.observe(b"\x1b[32m\xe4\xb8\xad");
        }
        assert!(matches!(
            stalled.recv().await,
            Err(broadcast::error::RecvError::Lagged(98))
        ));
        let frame = stalled.recv().await.unwrap();
        assert_eq!((frame.epoch, frame.sequence), (7, 99));
        assert_eq!(frame.bytes, b"\x1b[32m\xe4\xb8\xad");
        assert_eq!(tap.sequence(), 100);
    }
}

/// Taken by the single owning driver task, between PTY reads. A snapshot and
/// its byte cursor must never be assembled by two independent async queries.
#[derive(Clone, Debug)]
pub struct TerminalCut {
    pub epoch: u64,
    pub sequence: u64,
    pub screen: crewd_term::Snapshot,
}

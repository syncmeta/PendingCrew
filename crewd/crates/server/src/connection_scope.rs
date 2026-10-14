//! Connection correlation grants no authority. Fences close at the observed
//! connection boundary; they never Stop the remote team or replay unknown input.
use crewd_exec::acp::manual_fence::ManualFence;
use std::{
    collections::BTreeMap,
    sync::{Arc, Mutex},
    time::{Duration, Instant},
};
#[derive(Clone)]
pub(crate) struct ConnectionScope {
    pub id: String,
    live: Arc<Mutex<bool>>,
    output_live: Arc<Mutex<bool>>,
    closed_signal: tokio::sync::watch::Sender<bool>,
    fences: Arc<Mutex<BTreeMap<String, ManualFence>>>,
    watches: Arc<Mutex<BTreeMap<String, tokio::task::AbortHandle>>>,
}
impl ConnectionScope {
    pub fn new() -> Self {
        Self {
            id: uuid::Uuid::new_v4().to_string(),
            live: Arc::new(Mutex::new(true)),
            output_live: Arc::new(Mutex::new(true)),
            closed_signal: tokio::sync::watch::channel(false).0,
            fences: Arc::new(Mutex::new(BTreeMap::new())),
            watches: Arc::new(Mutex::new(BTreeMap::new())),
        }
    }
    pub fn close(&self) {
        if self
            .close_until(Instant::now() + Duration::from_secs(2))
            .is_err()
        {
            eprintln!("connection manual fence closure remains unknown");
        }
    }
    pub(crate) fn closed_signal(&self) -> tokio::sync::watch::Receiver<bool> {
        self.closed_signal.subscribe()
    }
    pub(crate) fn liveness(&self) -> Arc<Mutex<bool>> {
        self.live.clone()
    }
    pub(crate) fn close_until(&self, deadline: Instant) -> std::io::Result<()> {
        *self.live.lock().map_err(|_| closed())? = false;
        *self.output_live.lock().map_err(|_| closed())? = false;
        self.closed_signal.send_replace(true);
        self.fence_input_until(deadline)
    }
    /// EOF/server drain ends every input/custody/watch authority immediately.
    /// Already accepted replies may still flush under the writer's finite budget.
    /// Grant revocation/slow-client closure use close_until and stop output too.
    pub(crate) fn fence_input_until(&self, deadline: Instant) -> std::io::Result<()> {
        *self.live.lock().map_err(|_| closed())? = false;
        for (_, watch) in std::mem::take(&mut *self.watches.lock().map_err(|_| closed())?) {
            watch.abort();
        }
        let mut first_error = None;
        for (_, fence) in std::mem::take(&mut *self.fences.lock().map_err(|_| closed())?) {
            if let Err(error) = fence.revoke(deadline) {
                first_error.get_or_insert(error);
            }
        }
        first_error.map_or(Ok(()), Err)
    }
    pub fn guard(&self) -> ConnectionGuard {
        ConnectionGuard(self.clone())
    }
    pub fn add_watch(&self, id: String, handle: tokio::task::AbortHandle) -> std::io::Result<()> {
        let live = self.live.lock().map_err(|_| closed())?;
        let mut watches = self.watches.lock().map_err(|_| closed())?;
        if !*live || watches.len() >= 16 {
            handle.abort();
            return Err(closed());
        }
        watches.insert(id, handle);
        Ok(())
    }
    pub fn remove_watch(&self, id: &str) {
        if let Ok(mut watches) = self.watches.lock() {
            watches.remove(id);
        }
    }
    pub fn is_live(&self) -> bool {
        self.live.lock().is_ok_and(|live| *live)
    }
    pub(crate) fn output_is_live(&self) -> bool {
        self.output_live.lock().is_ok_and(|live| *live)
    }
    /// Keep the close/enable order atomic: a closed connection can NEVER enable
    /// a fence after Drop already disabled it. Authority commits separately.
    pub fn enable(&self, id: String, fence: ManualFence) -> std::io::Result<()> {
        let live = self.live.lock().map_err(|_| closed())?;
        if !*live {
            return Err(closed());
        }
        let mut fences = self.fences.lock().map_err(|_| closed())?;
        if fences.len() >= 16 || fences.contains_key(&id) {
            return Err(closed());
        }
        fence.enable_after_commit(Instant::now() + Duration::from_secs(2))?;
        fences.insert(id, fence);
        Ok(())
    }
    pub fn remove(&self, id: &str) -> std::io::Result<()> {
        let fence = self.fences.lock().map_err(|_| closed())?.remove(id);
        if let Some(fence) = fence {
            fence.revoke(Instant::now() + Duration::from_secs(2))?;
        }
        Ok(())
    }
}
fn closed() -> std::io::Error {
    std::io::Error::new(
        std::io::ErrorKind::BrokenPipe,
        "original connection custody closed or unknown",
    )
}
pub(crate) struct ConnectionGuard(ConnectionScope);
impl Drop for ConnectionGuard {
    fn drop(&mut self) {
        self.0.close();
    }
}

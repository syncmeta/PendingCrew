//! One factory index, two actual supported transport implementations. The
//! legacy branch is explicitly not an ACP/SDK producer compatibility claim.
use crate::{legacy_terminal::LegacyTerminal, native_helpers::Runtime, public_commands::Actor};
use crewd_exec::{
    acp::{ownership::NativeBinding, wire::TerminalFrame},
    driver::SessionHandle,
};
use serde_json::{Value, json};
use std::{io, sync::Arc};
#[derive(Clone)]
pub(crate) enum TerminalRuntime {
    Native(Arc<Runtime>),
    Legacy(Arc<LegacyTerminal>),
}
impl TerminalRuntime {
    pub(crate) fn generation(&self) -> &str {
        match self {
            Self::Native(r) => &r.generation,
            Self::Legacy(r) => &r.generation,
        }
    }
    pub(crate) fn original_gone(&self) -> bool {
        match self {
            Self::Native(r) => r.original_gone(),
            Self::Legacy(r) => r.original_gone(),
        }
    }
    pub(crate) fn is_live(&self) -> bool {
        match self {
            Self::Native(r) => r.is_live(),
            Self::Legacy(r) => r.is_live(),
        }
    }
    pub(crate) fn binding(&self) -> io::Result<NativeBinding> {
        match self {
            Self::Native(r) => r.binding(),
            Self::Legacy(r) => r.binding(),
        }
    }
    pub(crate) async fn snapshot(&self) -> io::Result<Value> {
        match self {
            Self::Native(r) => {
                r.call(
                    &uuid::Uuid::new_v4().to_string(),
                    "native/terminal.snapshot",
                    json!({}),
                )
                .await
            }
            Self::Legacy(r) => r.snapshot().await,
        }
    }
    pub(crate) async fn claim(
        &self,
        id: &str,
        a: &Actor,
        owner: &Arc<crate::AppState>,
        old: Option<&str>,
    ) -> io::Result<Value> {
        match self {
            Self::Native(r) => r.claim(id, a, old).await,
            Self::Legacy(r) => r.claim(a, owner, old).await,
        }
    }
    pub(crate) async fn manual_hook_allowed(&self) -> bool {
        match self {
            Self::Native(r) => r.manual_hook_allowed().await,
            Self::Legacy(r) => r.manual_hook_allowed().await,
        }
    }
    pub(crate) async fn manual_floor(&self) -> bool {
        match self {
            Self::Native(r) => r.manual_floor().await,
            Self::Legacy(r) => r.manual_floor().await,
        }
    }
    pub(crate) async fn release(&self, id: &str, a: &Actor, lease: &str) -> io::Result<Value> {
        match self {
            Self::Native(r) => r.release(id, a, lease).await,
            Self::Legacy(r) => r.release(a, lease).await,
        }
    }
    pub(crate) async fn write(
        &self,
        id: &str,
        a: &Actor,
        lease: &str,
        bytes: Vec<u8>,
    ) -> io::Result<Value> {
        match self {
            Self::Native(r) => r.write(id, a, lease, bytes).await,
            Self::Legacy(r) => r.write(a, lease, bytes).await,
        }
    }
    pub(crate) async fn resize(
        &self,
        id: &str,
        a: &Actor,
        lease: &str,
        c: u16,
        l: u16,
    ) -> io::Result<Value> {
        match self {
            Self::Native(r) => r.resize(id, a, lease, c, l).await,
            Self::Legacy(r) => r.resize(a, lease, c, l).await,
        }
    }
    pub(crate) fn terminal(&self) -> tokio::sync::broadcast::Receiver<TerminalFrame> {
        match self {
            Self::Native(r) => r.terminal(),
            Self::Legacy(r) => r.terminal(),
        }
    }
    pub(crate) fn oldest_sequence(&self, epoch: u64) -> Option<u64> {
        match self {
            Self::Native(r) => r.oldest_sequence(epoch),
            Self::Legacy(r) => r.oldest_sequence(epoch),
        }
    }
    pub(crate) fn after(&self, epoch: u64, after: u64) -> io::Result<Vec<TerminalFrame>> {
        match self {
            Self::Native(r) => r.after(epoch, after),
            Self::Legacy(r) => r.after(epoch, after),
        }
    }
}
impl crate::native_helpers::NativeFactory {
    pub(crate) async fn terminal_for(
        &self,
        session: &str,
        driver: Option<SessionHandle>,
    ) -> io::Result<TerminalRuntime> {
        if let Some(native) = self.get(session)
            && native.is_live()
        {
            return Ok(TerminalRuntime::Native(native));
        }
        let existing = self
            .legacy
            .lock()
            .map_err(|_| io::Error::other("terminal factory poisoned"))?
            .get(session)
            .cloned();
        if let Some(legacy) = existing
            && legacy.is_live()
        {
            return Ok(TerminalRuntime::Legacy(legacy));
        }
        let driver = driver.ok_or_else(|| {
            io::Error::new(
                io::ErrorKind::Unsupported,
                "backend has no actual manual PTY",
            )
        })?;
        // Serialize ONLY this session's adoption, not other execution lines.
        let gate = {
            let mut gates = self
                .legacy_adoption
                .lock()
                .map_err(|_| io::Error::other("terminal adoption poisoned"))?;
            gates
                .entry(session.to_owned())
                .or_insert_with(|| Arc::new(tokio::sync::Mutex::new(())))
                .clone()
        };
        let _adoption = gate.lock().await;
        let current = self
            .legacy
            .lock()
            .map_err(|_| io::Error::other("terminal factory poisoned"))?
            .get(session)
            .cloned();
        if let Some(current) = current
            && current.is_live()
        {
            return Ok(TerminalRuntime::Legacy(current));
        }
        let legacy =
            LegacyTerminal::adopt(session.to_owned(), driver, self.db.clone(), &self.registry)
                .await?;
        self.legacy
            .lock()
            .map_err(|_| io::Error::other("terminal factory poisoned"))?
            .insert(session.to_owned(), legacy.clone());
        Ok(TerminalRuntime::Legacy(legacy))
    }
}

//! Internal same-runtime authentication bridge. No tool/RPC/JSON caller can
//! choose a source, export a credential, or create a replacement auth owner.
use super::{exec::ExecHandle, port::Port};
use crewd_exec::codex::profile::{AuthFailure, ExternalChatgptAuth, NativeAuthSource};
use std::{sync::Arc, time::Duration};

pub(super) struct Source<P: Port> {
    pub port: Arc<P>,
    pub admission: Arc<crate::lifecycle::Admission>,
    pub admin: String,
    pub source: String,
    /// Handle and native identity belong to one captured Core generation. Never
    /// look up a replacement source handle after a stop or restart.
    pub handle: Arc<dyn ExecHandle>,
    pub native_id: String,
    pub pid: u32,
}
impl<P: Port> Source<P> {
    async fn valid(&self) -> Result<(), AuthFailure> {
        if !self.admission.contains(&self.admin) || !self.admission.contains(&self.source) {
            return Err(AuthFailure::Unavailable);
        }
        let source = self
            .port
            .admin_case_auth_source(&self.admin)
            .await
            .map_err(|_| AuthFailure::Unavailable)?;
        if source.as_deref() != Some(self.source.as_str()) {
            return Err(AuthFailure::IdentityChanged);
        }
        let live = self
            .handle
            .status()
            .await
            .map_err(|_| AuthFailure::Unavailable)?;
        if live.pid != self.pid || live.agent_session_id != self.native_id {
            return Err(AuthFailure::IdentityChanged);
        }
        Ok(())
    }
}
impl<P: Port> NativeAuthSource for Source<P> {
    fn access(
        &self,
        refresh: bool,
        previous: Option<ExternalChatgptAuth>,
    ) -> std::pin::Pin<
        Box<dyn std::future::Future<Output = Result<ExternalChatgptAuth, AuthFailure>> + Send + '_>,
    > {
        Box::pin(async move {
            tokio::time::timeout(Duration::from_secs(8), async {
                self.valid().await?;
                let credential = self.handle.native_auth_access(refresh, previous).await?;
                self.valid().await?;
                Ok(credential)
            })
            .await
            .unwrap_or(Err(AuthFailure::Unknown))
        })
    }
}

#[cfg(test)]
mod tests {
    use super::super::{
        exec::{AnswerSteps, BoxFut, Context, ExecSendError},
        testkit::MemPort,
    };
    use super::*;
    use crewd_exec::{
        driver::{Closed, SessionStatus},
        session::{AnswerOutcome, DerivedState},
    };
    use std::sync::Mutex;
    struct Owner {
        native: Mutex<String>,
        started: tokio::sync::Notify,
        finish: tokio::sync::Notify,
        auth: ExternalChatgptAuth,
    }
    impl ExecHandle for Owner {
        fn native_auth_access(
            &self,
            _: bool,
            _: Option<ExternalChatgptAuth>,
        ) -> BoxFut<'_, Result<ExternalChatgptAuth, AuthFailure>> {
            Box::pin(async {
                self.started.notify_one();
                self.finish.notified().await;
                Ok(self.auth.clone())
            })
        }
        fn status(&self) -> BoxFut<'_, Result<SessionStatus, Closed>> {
            Box::pin(async {
                Ok(SessionStatus {
                    state: DerivedState::Idle,
                    working: false,
                    health: None,
                    pending_decision: None,
                    pid: 44,
                    agent_session_id: self.native.lock().unwrap().clone(),
                })
            })
        }
        fn send(&self, _: String, _: &Context) -> Result<u64, ExecSendError> {
            unreachable!()
        }
        fn raw(&self, _: Vec<u8>) -> Result<(), Closed> {
            unreachable!()
        }
        fn interrupt(&self) -> Result<(), Closed> {
            unreachable!()
        }
        fn answer(&self, _: String, _: AnswerSteps) -> BoxFut<'_, Result<AnswerOutcome, Closed>> {
            unreachable!()
        }
        fn stop(&self) -> Result<(), Closed> {
            unreachable!()
        }
        fn clear_quota_health(&self) -> Result<(), Closed> {
            unreachable!()
        }
        fn screen_text(&self, _: usize) -> BoxFut<'_, Result<String, Closed>> {
            unreachable!()
        }
        fn snapshot(&self) -> BoxFut<'_, Result<Option<crewd_term::Snapshot>, Closed>> {
            unreachable!()
        }
    }
    fn fixture() -> (Arc<Source<MemPort>>, Arc<Owner>) {
        use base64::Engine;
        let claims = serde_json::json!({"exp":4102444800i64,"https://api.openai.com/auth":{"chatgpt_account_id":"a","chatgpt_user_id":"u"}});
        let jwt = format!(
            "synthetic.{}.signature",
            base64::engine::general_purpose::URL_SAFE_NO_PAD
                .encode(serde_json::to_vec(&claims).unwrap())
        );
        let auth = ExternalChatgptAuth::from_native(
            &serde_json::json!({"authMethod":"chatgpt","requiresOpenaiAuth":true,"authToken":jwt}),
            1,
        )
        .unwrap();
        let owner = Arc::new(Owner {
            native: Mutex::new("original-thread".into()),
            started: tokio::sync::Notify::new(),
            finish: tokio::sync::Notify::new(),
            auth,
        });
        let port = Arc::new(MemPort::new());
        port.lock()
            .native_auth_sources
            .insert("admin".into(), "source".into());
        let admission = Arc::new(crate::lifecycle::Admission::default());
        admission.allow("source");
        admission.allow("admin");
        (
            Arc::new(Source {
                port,
                admission,
                source: "source".into(),
                admin: "admin".into(),
                handle: owner.clone(),
                native_id: "original-thread".into(),
                pid: 44,
            }),
            owner,
        )
    }
    #[tokio::test]
    async fn source_or_admin_revocation_while_export_pending_rejects_credential() {
        for who in ["source", "admin"] {
            let (bridge, owner) = fixture();
            let b = bridge.clone();
            let pending = tokio::spawn(async move { b.access(false, None).await });
            owner.started.notified().await;
            bridge.admission.revoke(who);
            owner.finish.notify_one();
            assert_eq!(pending.await.unwrap(), Err(AuthFailure::Unavailable));
        }
    }
    #[tokio::test]
    async fn source_binding_or_native_identity_change_cannot_rebind_pending_export() {
        for binding in [true, false] {
            let (bridge, owner) = fixture();
            let b = bridge.clone();
            let pending = tokio::spawn(async move { b.access(false, None).await });
            owner.started.notified().await;
            if binding {
                bridge
                    .port
                    .lock()
                    .native_auth_sources
                    .insert("admin".into(), "other".into());
            } else {
                *owner.native.lock().unwrap() = "replacement-thread".into();
            }
            owner.finish.notify_one();
            assert_eq!(pending.await.unwrap(), Err(AuthFailure::IdentityChanged));
        }
    }
}

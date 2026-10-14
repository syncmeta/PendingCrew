use super::Manager;
use std::sync::{Arc, Weak, atomic::Ordering};
use tokio::{net::TcpListener, sync::Semaphore};
use tokio_rustls::TlsAcceptor;

/// Only explicit enabled configuration calls this. Normal defaults never bind.
pub(super) fn listen(
    manager: Weak<Manager>,
    socket: Arc<TcpListener>,
    config: Arc<rustls::ServerConfig>,
    revision: String,
    epoch: u64,
) -> std::pin::Pin<Box<dyn std::future::Future<Output = ()> + Send>> {
    // Erase the listener future at the recursive dispatch/configure boundary.
    Box::pin(async move {
        let acceptor = TlsAcceptor::from(config);
        let admissions = Arc::new(Semaphore::new(64));
        loop {
            let accepted = socket.accept().await;
            let Ok((stream, _)) = accepted else {
                break;
            };
            let Ok(permit) = admissions.clone().try_acquire_owned() else {
                continue;
            };
            let Some(owner) = manager.upgrade() else {
                break;
            };
            if owner.listener_epoch.load(Ordering::SeqCst) != epoch || *owner.drain.borrow() {
                break;
            }
            let acceptor = acceptor.clone();
            let revision = revision.clone();
            let mut connections = owner.connections.lock().await;
            while connections.try_join_next().is_some() {}
            let connection_owner = owner.clone();
            connections.spawn(async move {
                let owner = connection_owner;
                let _permit = permit;
                let Ok(Ok(stream)) = tokio::time::timeout(
                    std::time::Duration::from_secs(5),
                    acceptor.accept(stream),
                )
                .await
                else {
                    return;
                };
                if stream.get_ref().1.protocol_version() != Some(rustls::ProtocolVersion::TLSv1_3) {
                    return;
                }
                let Some(certificate) = stream
                    .get_ref()
                    .1
                    .peer_certificates()
                    .and_then(|c| c.first())
                else {
                    return;
                };
                let Ok(principal) = owner.qualify(certificate.as_ref(), &revision, epoch).await
                else {
                    return;
                };
                let Ok(state) = owner.state() else {
                    return;
                };
                let drain = owner.drain.clone();
                let _ = crate::conn::run_remote(state, stream, drain, principal).await;
            });
        }
        // The manager owns admitted connection futures independently of listener
        // intake, so reconfigure/revoke cannot abort an already admitted request.
    })
}

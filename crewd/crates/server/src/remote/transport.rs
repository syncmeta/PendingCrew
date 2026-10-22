use super::Manager;
use std::sync::{Arc, Weak, atomic::Ordering};
use tokio::{net::TcpListener, sync::Semaphore};
use tokio_rustls::LazyConfigAcceptor;

/// Only explicit enabled configuration calls this. Normal defaults never bind.
pub(super) fn listen(
    manager: Weak<Manager>,
    socket: Arc<TcpListener>,
    config: Arc<rustls::ServerConfig>,
    enrollment_config: Arc<rustls::ServerConfig>,
    revision: String,
    epoch: u64,
) -> std::pin::Pin<Box<dyn std::future::Future<Output = ()> + Send>> {
    // Erase the listener future at the recursive dispatch/configure boundary.
    Box::pin(async move {
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
            let config = config.clone();
            let enrollment_config = enrollment_config.clone();
            let revision = revision.clone();
            let mut connections = owner.connections.lock().await;
            while connections.try_join_next().is_some() {}
            let connection_owner = owner.clone();
            connections.spawn(async move {
                let owner = connection_owner;
                let _permit = permit;
                let started = tokio::time::Instant::now();
                let handshake = async {
                    let hello =
                        LazyConfigAcceptor::new(rustls::server::Acceptor::default(), stream)
                            .await?;
                    let route = crewd_remote::remote_tls_route(hello.client_hello().alpn())
                        .map_err(std::io::Error::other)?;
                    let selected = match route {
                        crewd_remote::RemoteTlsRoute::Work => config,
                        crewd_remote::RemoteTlsRoute::Enrollment => enrollment_config,
                    };
                    let stream = hello.into_stream(selected).await?;
                    Ok::<_, std::io::Error>((stream, route))
                };
                let Ok(Ok((stream, route))) =
                    tokio::time::timeout_at(started + std::time::Duration::from_secs(5), handshake)
                        .await
                else {
                    return;
                };
                if stream.get_ref().1.protocol_version() != Some(rustls::ProtocolVersion::TLSv1_3) {
                    return;
                }
                if route == crewd_remote::RemoteTlsRoute::Enrollment {
                    if stream.get_ref().1.alpn_protocol() != Some(crewd_proto::ENROLLMENT_ALPN) {
                        return;
                    }
                    let _ = tokio::time::timeout_at(
                        started + std::time::Duration::from_secs(15),
                        run_enrollment(owner, stream, revision, epoch),
                    )
                    .await;
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

async fn run_enrollment(
    owner: Arc<Manager>,
    stream: tokio_rustls::server::TlsStream<tokio::net::TcpStream>,
    revision: String,
    epoch: u64,
) -> std::io::Result<()> {
    use crewd_proto::{MAX_ENROLLMENT_FRAME_BYTES, Response};
    use tokio::io::{AsyncBufReadExt, AsyncReadExt, AsyncWriteExt, BufReader};
    // Keep this strict wire shape separate from the extensible ordinary RPC envelope.
    #[derive(serde::Deserialize)]
    #[serde(deny_unknown_fields)]
    struct Wire {
        id: u64,
        method: String,
        params: serde_json::Value,
    }
    let (rd, mut wr) = tokio::io::split(stream);
    let mut rd = BufReader::new(rd);
    for _ in 0..3 {
        let mut line = Vec::new();
        let mut bounded = (&mut rd).take(MAX_ENROLLMENT_FRAME_BYTES as u64 + 1);
        let count = tokio::time::timeout(
            std::time::Duration::from_secs(5),
            bounded.read_until(b'\n', &mut line),
        )
        .await
        .map_err(std::io::Error::other)??;
        if count == 0 {
            return Ok(());
        }
        if line.len() > MAX_ENROLLMENT_FRAME_BYTES || line.last() != Some(&b'\n') {
            return Err(std::io::Error::other("enrollment frame bound"));
        }
        let wire: Wire = serde_json::from_slice(&line)
            .map_err(|_| std::io::Error::other("invalid enrollment frame"))?;
        if wire.id == 0
            || owner.listener_epoch.load(Ordering::SeqCst) != epoch
            || *owner.drain.borrow()
        {
            return Ok(());
        }
        let outcome = owner
            .enrollment_route(&wire.method, wire.params, &revision, epoch)
            .await;
        let response = match outcome {
            Ok(value) => Response::ok(wire.id, value),
            Err(error) => Response::err(wire.id, error),
        };
        let mut bytes = serde_json::to_vec(&response).map_err(std::io::Error::other)?;
        if bytes.len() > MAX_ENROLLMENT_FRAME_BYTES {
            return Err(std::io::Error::other("enrollment reply bound"));
        }
        bytes.push(b'\n');
        tokio::time::timeout(std::time::Duration::from_secs(5), wr.write_all(&bytes))
            .await
            .map_err(std::io::Error::other)??;
    }
    Ok(())
}

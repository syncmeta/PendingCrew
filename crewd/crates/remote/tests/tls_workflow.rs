//! Real TLS over ephemeral loopback sockets; this is not durable RPC authorization proof.
use crewd_remote::{
    CertificateIdentity, LiveClientPins, client_configuration, server_configuration,
};
use rustls::{ClientConnection, ServerConnection, StreamOwned, pki_types::ServerName};
use std::{
    collections::BTreeSet,
    io::{Read, Write},
    net::{TcpListener, TcpStream},
    sync::Arc,
    time::Duration,
};

fn exchange(server: Arc<rustls::ServerConfig>, client: Arc<rustls::ClientConfig>) -> (bool, bool) {
    let listener = TcpListener::bind("127.0.0.1:0").unwrap();
    let address = listener.local_addr().unwrap();
    let worker = std::thread::spawn(move || -> std::io::Result<()> {
        let (socket, _) = listener.accept()?;
        socket.set_read_timeout(Some(Duration::from_secs(2)))?;
        socket.set_write_timeout(Some(Duration::from_secs(2)))?;
        let connection = ServerConnection::new(server).map_err(std::io::Error::other)?;
        let mut stream = StreamOwned::new(connection, socket);
        let mut byte = [0];
        stream.read_exact(&mut byte)?;
        assert_eq!(byte, [b'Q']);
        assert_eq!(
            stream.conn.protocol_version(),
            Some(rustls::ProtocolVersion::TLSv1_3)
        );
        stream.write_all(b"R")?;
        stream.flush()?;
        Ok(())
    });
    let accepted = (|| -> std::io::Result<()> {
        let socket = TcpStream::connect(address)?;
        socket.set_read_timeout(Some(Duration::from_secs(2)))?;
        socket.set_write_timeout(Some(Duration::from_secs(2)))?;
        let connection =
            ClientConnection::new(client, ServerName::try_from("pendingcrew.invalid").unwrap())
                .map_err(std::io::Error::other)?;
        let mut stream = StreamOwned::new(connection, socket);
        stream.write_all(b"Q")?;
        stream.flush()?;
        let mut byte = [0];
        stream.read_exact(&mut byte)?;
        assert_eq!(byte, [b'R']);
        assert_eq!(
            stream.conn.protocol_version(),
            Some(rustls::ProtocolVersion::TLSv1_3)
        );
        Ok(())
    })()
    .is_ok();
    (accepted, worker.join().unwrap().is_ok())
}

#[test]
fn actual_mutual_tls_requires_both_confirmed_pins_and_rechecks_new_handshakes() {
    let server_identity = CertificateIdentity::generate().unwrap();
    let client_identity = CertificateIdentity::generate().unwrap();
    let stranger = CertificateIdentity::generate().unwrap();
    let pins = Arc::new(LiveClientPins::default());
    pins.replace(BTreeSet::from([client_identity.pin().to_owned()]))
        .unwrap();
    let server = Arc::new(server_configuration(&server_identity, pins.clone()).unwrap());
    assert_eq!(server.max_early_data_size, 0);
    assert_eq!(server.send_tls13_tickets, 0);
    let client = Arc::new(client_configuration(&client_identity, server_identity.pin()).unwrap());
    assert!(!client.enable_early_data);
    assert_eq!(exchange(server.clone(), client.clone()), (true, true));
    let wrong_server = Arc::new(client_configuration(&client_identity, stranger.pin()).unwrap());
    assert_eq!(exchange(server.clone(), wrong_server), (false, false));
    let ungranted_client =
        Arc::new(client_configuration(&stranger, server_identity.pin()).unwrap());
    assert_eq!(exchange(server.clone(), ungranted_client), (false, false));
    // Reuse the same TLS configurations: no fresh credential construction masks stale cache state.
    pins.replace(BTreeSet::new()).unwrap();
    assert_eq!(exchange(server.clone(), client.clone()), (false, false));
    pins.replace(BTreeSet::from([client_identity.pin().to_owned()]))
        .unwrap();
    assert_eq!(exchange(server, client), (true, true));
}

#[test]
fn signed_pairing_bytes_cannot_be_tampered_or_rebound_to_another_identity() {
    use base64::{Engine, engine::general_purpose::STANDARD};
    use crewd_remote::{CertificateRole, verify_envelope};
    let identity = CertificateIdentity::generate().unwrap();
    let stranger = CertificateIdentity::generate().unwrap();
    let payload = serde_json::json!({"challenge":"original-challenge","authorityId":"11111111-1111-4111-8111-111111111111"});
    let envelope = identity.sign(&payload).unwrap();
    let verify = |value: &crewd_proto::RemoteSignedEnvelope| {
        verify_envelope::<serde_json::Value>(
            value,
            identity.certificate_der(),
            CertificateRole::Server,
            identity.pin(),
        )
    };
    assert_eq!(verify(&envelope).unwrap(), payload);
    let mut altered = envelope.clone();
    altered.payload = STANDARD.encode(
        br#"{"challenge":"other-challenge","authorityId":"11111111-1111-4111-8111-111111111111"}"#,
    );
    assert!(verify(&altered).is_err());
    altered = envelope.clone();
    altered.signature = stranger.sign(&payload).unwrap().signature;
    assert!(verify(&altered).is_err());
    assert!(
        verify_envelope::<serde_json::Value>(
            &envelope,
            stranger.certificate_der(),
            CertificateRole::Server,
            stranger.pin()
        )
        .is_err()
    );
    assert!(
        verify_envelope::<serde_json::Value>(
            &envelope,
            identity.certificate_der(),
            CertificateRole::Server,
            stranger.pin()
        )
        .is_err()
    );
    altered = envelope.clone();
    altered.format = 0;
    assert!(verify(&altered).is_err());
    // Signature integrity is not invitation consumption, Human approval, grant or RPC authority.
}

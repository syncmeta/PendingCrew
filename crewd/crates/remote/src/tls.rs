use crate::{CertificateIdentity, CertificateRole, certificate_pin};
use anyhow::Result;
use rustls::client::danger::{HandshakeSignatureValid, ServerCertVerified, ServerCertVerifier};
use rustls::server::danger::{ClientCertVerified, ClientCertVerifier};
use rustls::{
    CertificateError, DigitallySignedStruct, DistinguishedName, Error, SignatureScheme,
    crypto::{CryptoProvider, verify_tls12_signature, verify_tls13_signature},
    pki_types::{CertificateDer, PrivatePkcs8KeyDer, ServerName, UnixTime},
};
use std::{
    collections::BTreeSet,
    sync::{Arc, RwLock},
};

/// This handshake cache can only refuse or preselect candidates. The production
/// server must still qualify the certificate against the durable grant before RPC.
#[derive(Debug, Default)]
pub struct LiveClientPins(RwLock<BTreeSet<String>>);
impl LiveClientPins {
    pub fn replace(&self, pins: BTreeSet<String>) -> Result<()> {
        anyhow::ensure!(
            pins.len() <= crewd_proto::MAX_REMOTE_GRANTS
                && pins.iter().all(|p| crewd_proto::valid_spki_pin(p)),
            "invalid active client pins"
        );
        *self
            .0
            .write()
            .map_err(|_| anyhow::anyhow!("client pin state unavailable"))? = pins;
        Ok(())
    }
    fn allows(&self, pin: &str) -> bool {
        self.0
            .read()
            .map(|pins| pins.contains(pin))
            .unwrap_or(false)
    }
}
#[derive(Debug)]
struct PinnedClients {
    pins: Arc<LiveClientPins>,
    provider: Arc<CryptoProvider>,
}
fn invalid() -> Error {
    Error::InvalidCertificate(CertificateError::ApplicationVerificationFailure)
}
impl ClientCertVerifier for PinnedClients {
    fn root_hint_subjects(&self) -> &[DistinguishedName] {
        &[]
    }
    fn verify_client_cert(
        &self,
        end: &CertificateDer<'_>,
        intermediates: &[CertificateDer<'_>],
        now: UnixTime,
    ) -> std::result::Result<ClientCertVerified, Error> {
        if !intermediates.is_empty() {
            return Err(invalid());
        }
        let pin = certificate_pin(end.as_ref(), CertificateRole::Client, now.as_secs())
            .map_err(|_| invalid())?;
        if !self.pins.allows(&pin) {
            return Err(invalid());
        }
        Ok(ClientCertVerified::assertion())
    }
    fn verify_tls12_signature(
        &self,
        message: &[u8],
        cert: &CertificateDer<'_>,
        dss: &DigitallySignedStruct,
    ) -> std::result::Result<HandshakeSignatureValid, Error> {
        verify_tls12_signature(
            message,
            cert,
            dss,
            &self.provider.signature_verification_algorithms,
        )
    }
    fn verify_tls13_signature(
        &self,
        message: &[u8],
        cert: &CertificateDer<'_>,
        dss: &DigitallySignedStruct,
    ) -> std::result::Result<HandshakeSignatureValid, Error> {
        verify_tls13_signature(
            message,
            cert,
            dss,
            &self.provider.signature_verification_algorithms,
        )
    }
    fn supported_verify_schemes(&self) -> Vec<SignatureScheme> {
        self.provider
            .signature_verification_algorithms
            .supported_schemes()
    }
}
#[derive(Debug)]
struct PinnedServer {
    pin: String,
    provider: Arc<CryptoProvider>,
}
impl ServerCertVerifier for PinnedServer {
    fn verify_server_cert(
        &self,
        end: &CertificateDer<'_>,
        intermediates: &[CertificateDer<'_>],
        _name: &ServerName<'_>,
        _ocsp: &[u8],
        now: UnixTime,
    ) -> std::result::Result<ServerCertVerified, Error> {
        if !intermediates.is_empty() {
            return Err(invalid());
        }
        let pin = certificate_pin(end.as_ref(), CertificateRole::Server, now.as_secs())
            .map_err(|_| invalid())?;
        if pin != self.pin {
            return Err(invalid());
        }
        Ok(ServerCertVerified::assertion())
    }
    fn verify_tls12_signature(
        &self,
        message: &[u8],
        cert: &CertificateDer<'_>,
        dss: &DigitallySignedStruct,
    ) -> std::result::Result<HandshakeSignatureValid, Error> {
        verify_tls12_signature(
            message,
            cert,
            dss,
            &self.provider.signature_verification_algorithms,
        )
    }
    fn verify_tls13_signature(
        &self,
        message: &[u8],
        cert: &CertificateDer<'_>,
        dss: &DigitallySignedStruct,
    ) -> std::result::Result<HandshakeSignatureValid, Error> {
        verify_tls13_signature(
            message,
            cert,
            dss,
            &self.provider.signature_verification_algorithms,
        )
    }
    fn supported_verify_schemes(&self) -> Vec<SignatureScheme> {
        self.provider
            .signature_verification_algorithms
            .supported_schemes()
    }
}

pub fn server_configuration(
    identity: &CertificateIdentity,
    pins: Arc<LiveClientPins>,
) -> Result<rustls::ServerConfig> {
    let provider = Arc::new(rustls::crypto::ring::default_provider());
    let verifier = Arc::new(PinnedClients {
        pins,
        provider: provider.clone(),
    });
    let mut config = rustls::ServerConfig::builder_with_provider(provider)
        .with_protocol_versions(&[&rustls::version::TLS13])?
        .with_client_cert_verifier(verifier)
        .with_single_cert(
            vec![CertificateDer::from(identity.certificate_der().to_vec())],
            PrivatePkcs8KeyDer::from(identity.private_key_der().to_vec()).into(),
        )?;
    // No early work writes and no ticket that could avoid fresh grant qualification.
    config.max_early_data_size = 0;
    config.send_tls13_tickets = 0;
    Ok(config)
}
pub fn client_configuration(
    identity: &CertificateIdentity,
    expected_server_pin: &str,
) -> Result<rustls::ClientConfig> {
    anyhow::ensure!(
        crewd_proto::valid_spki_pin(expected_server_pin),
        "invalid confirmed server pin"
    );
    let provider = Arc::new(rustls::crypto::ring::default_provider());
    let verifier = Arc::new(PinnedServer {
        pin: expected_server_pin.into(),
        provider: provider.clone(),
    });
    let mut config = rustls::ClientConfig::builder_with_provider(provider)
        .with_protocol_versions(&[&rustls::version::TLS13])?
        .dangerous()
        .with_custom_certificate_verifier(verifier)
        .with_client_auth_cert(
            vec![CertificateDer::from(identity.certificate_der().to_vec())],
            PrivatePkcs8KeyDer::from(identity.private_key_der().to_vec()).into(),
        )?;
    config.enable_early_data = false;
    Ok(config)
}

/// Enrollment has a different complete configuration, never optional authentication
/// on the work configuration. The ClientHello selector must enforce this ALPN.
pub fn enrollment_server_configuration(
    identity: &CertificateIdentity,
) -> Result<rustls::ServerConfig> {
    let provider = Arc::new(rustls::crypto::ring::default_provider());
    let mut config = rustls::ServerConfig::builder_with_provider(provider)
        .with_protocol_versions(&[&rustls::version::TLS13])?
        .with_no_client_auth()
        .with_single_cert(
            vec![CertificateDer::from(identity.certificate_der().to_vec())],
            PrivatePkcs8KeyDer::from(identity.private_key_der().to_vec()).into(),
        )?;
    config.alpn_protocols = vec![crewd_proto::ENROLLMENT_ALPN.to_vec()];
    config.max_early_data_size = 0;
    config.send_tls13_tickets = 0;
    Ok(config)
}
pub fn enrollment_client_configuration(expected_server_pin: &str) -> Result<rustls::ClientConfig> {
    anyhow::ensure!(
        crewd_proto::valid_spki_pin(expected_server_pin),
        "invalid confirmed server pin"
    );
    let provider = Arc::new(rustls::crypto::ring::default_provider());
    let verifier = Arc::new(PinnedServer {
        pin: expected_server_pin.into(),
        provider: provider.clone(),
    });
    let mut config = rustls::ClientConfig::builder_with_provider(provider)
        .with_protocol_versions(&[&rustls::version::TLS13])?
        .dangerous()
        .with_custom_certificate_verifier(verifier)
        .with_no_client_auth();
    config.alpn_protocols = vec![crewd_proto::ENROLLMENT_ALPN.to_vec()];
    config.enable_early_data = false;
    Ok(config)
}
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum RemoteTlsRoute {
    Work,
    Enrollment,
}
/// No list means the original mTLS work route. Mixed/unknown lists fail closed.
pub fn remote_tls_route<'a>(
    offered: Option<impl Iterator<Item = &'a [u8]>>,
) -> Result<RemoteTlsRoute> {
    match offered {
        None => Ok(RemoteTlsRoute::Work),
        Some(mut protocols) => {
            anyhow::ensure!(
                protocols.next() == Some(crewd_proto::ENROLLMENT_ALPN)
                    && protocols.next().is_none(),
                "unsupported remote TLS protocol"
            );
            Ok(RemoteTlsRoute::Enrollment)
        }
    }
}

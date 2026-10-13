//! Certificate, signature and TLS primitives only. Neither a pin nor a successful
//! handshake grants work permission; the server rechecks durable grants per request.
mod tls;
use anyhow::{Context, Result, ensure};
use base64::{Engine, engine::general_purpose::STANDARD};
use crewd_proto::{
    MAX_REMOTE_CERTIFICATE_BYTES, MAX_REMOTE_ENVELOPE_BYTES, REMOTE_FORMAT, RemoteSignedEnvelope,
};
use rcgen::{CertificateParams, ExtendedKeyUsagePurpose, KeyPair, KeyUsagePurpose};
use ring::{
    digest,
    rand::SystemRandom,
    signature::{self, KeyPair as _},
};
use serde::{Serialize, de::DeserializeOwned};
pub use tls::{LiveClientPins, client_configuration, server_configuration};
use x509_parser::{parse_x509_certificate, prelude::ASN1Time};

/// Private credential has intentionally no Serialize/Debug implementation.
pub struct CertificateIdentity {
    certificate_der: Vec<u8>,
    key_pkcs8_der: Vec<u8>,
    signer: signature::EcdsaKeyPair,
    pin: String,
}
#[derive(Clone, Copy)]
pub enum CertificateRole {
    Server,
    Client,
}

pub fn decode_certificate(encoded: &str) -> Result<Vec<u8>> {
    ensure!(
        encoded.len() <= MAX_REMOTE_CERTIFICATE_BYTES * 2,
        "certificate exceeds bound"
    );
    let der = STANDARD
        .decode(encoded)
        .context("invalid certificate base64")?;
    ensure!(
        !der.is_empty() && der.len() <= MAX_REMOTE_CERTIFICATE_BYTES,
        "certificate exceeds bound"
    );
    Ok(der)
}

/// Validate the accepted self-signed P256 profile and hash its complete DER SPKI.
/// This does not approve the resulting identity or verify a TLS handshake signature.
pub fn certificate_pin(der: &[u8], role: CertificateRole, now_unix: u64) -> Result<String> {
    ensure!(
        !der.is_empty() && der.len() <= MAX_REMOTE_CERTIFICATE_BYTES,
        "certificate exceeds bound"
    );
    let (remainder, cert) =
        parse_x509_certificate(der).map_err(|_| anyhow::anyhow!("invalid certificate DER"))?;
    ensure!(remainder.is_empty(), "trailing certificate data");
    let now = i64::try_from(now_unix).context("certificate time out of range")?;
    ensure!(
        cert.validity().is_valid_at(ASN1Time::from_timestamp(now)?),
        "certificate expired or not yet valid"
    );
    ensure!(
        cert.issuer() == cert.subject(),
        "expected a self-signed identity"
    );
    cert.verify_signature(None)
        .map_err(|_| anyhow::anyhow!("invalid self-signature"))?;
    let public = cert.public_key();
    ensure!(
        public.algorithm.algorithm.to_id_string() == "1.2.840.10045.2.1",
        "expected an EC identity"
    );
    let curve = public
        .algorithm
        .parameters
        .as_ref()
        .and_then(|p| p.as_oid().ok());
    ensure!(
        curve.map(|oid| oid.to_id_string()).as_deref() == Some("1.2.840.10045.3.1.7"),
        "expected P256 identity"
    );
    ensure!(
        public.subject_public_key.unused_bits == 0,
        "invalid public-key bits"
    );
    if let Some(basic) = cert
        .basic_constraints()
        .map_err(|_| anyhow::anyhow!("invalid basic constraints"))?
    {
        ensure!(!basic.value.ca, "identity must not be a CA");
    }
    let usage = cert
        .extended_key_usage()
        .map_err(|_| anyhow::anyhow!("invalid key usage"))?
        .context("missing identity key usage")?;
    ensure!(
        match role {
            CertificateRole::Server => usage.value.server_auth,
            CertificateRole::Client => usage.value.client_auth,
        },
        "wrong certificate role"
    );
    Ok(hex(digest::digest(&digest::SHA256, public.raw).as_ref()))
}
fn hex(bytes: &[u8]) -> String {
    bytes.iter().map(|b| format!("{b:02x}")).collect()
}
pub fn unix_now() -> Result<u64> {
    Ok(std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)?
        .as_secs())
}

impl CertificateIdentity {
    pub fn generate() -> Result<Self> {
        let key = KeyPair::generate_for(&rcgen::PKCS_ECDSA_P256_SHA256)?;
        let mut params = CertificateParams::new(vec!["pendingcrew.invalid".into()])?;
        params.key_usages = vec![KeyUsagePurpose::DigitalSignature];
        params.extended_key_usages = vec![
            ExtendedKeyUsagePurpose::ServerAuth,
            ExtendedKeyUsagePurpose::ClientAuth,
        ];
        let certificate = params.self_signed(&key)?;
        Self::from_der(certificate.der().to_vec(), key.serialize_der())
    }
    pub fn from_der(certificate_der: Vec<u8>, key_pkcs8_der: Vec<u8>) -> Result<Self> {
        ensure!(
            key_pkcs8_der.len() <= 4096,
            "private credential exceeds bound"
        );
        let pin = certificate_pin(&certificate_der, CertificateRole::Server, unix_now()?)?;
        let signer = signature::EcdsaKeyPair::from_pkcs8(
            &signature::ECDSA_P256_SHA256_ASN1_SIGNING,
            &key_pkcs8_der,
            &SystemRandom::new(),
        )
        .map_err(|_| anyhow::anyhow!("invalid private credential"))?;
        let (_, cert) = parse_x509_certificate(&certificate_der)
            .map_err(|_| anyhow::anyhow!("invalid certificate DER"))?;
        ensure!(
            cert.public_key().subject_public_key.data.as_ref() == signer.public_key().as_ref(),
            "certificate/private-key mismatch"
        );
        Ok(Self {
            certificate_der,
            key_pkcs8_der,
            signer,
            pin,
        })
    }
    pub fn certificate_der(&self) -> &[u8] {
        &self.certificate_der
    }
    pub fn certificate_base64(&self) -> String {
        STANDARD.encode(&self.certificate_der)
    }
    pub fn pin(&self) -> &str {
        &self.pin
    }
    /// Only for the secure identity-file writer or TLS local-identity adapter.
    pub fn private_key_der(&self) -> &[u8] {
        &self.key_pkcs8_der
    }
    pub fn sign<T: Serialize>(&self, payload: &T) -> Result<RemoteSignedEnvelope> {
        let bytes = serde_json::to_vec(payload)?;
        ensure!(
            bytes.len() <= MAX_REMOTE_ENVELOPE_BYTES / 2,
            "pairing payload exceeds bound"
        );
        let signature = self
            .signer
            .sign(&SystemRandom::new(), &bytes)
            .map_err(|_| anyhow::anyhow!("cannot sign pairing payload"))?;
        Ok(RemoteSignedEnvelope {
            format: REMOTE_FORMAT,
            payload: STANDARD.encode(bytes),
            signature: STANDARD.encode(signature.as_ref()),
        })
    }
}

pub fn verify_envelope<T: DeserializeOwned>(
    envelope: &RemoteSignedEnvelope,
    certificate_der: &[u8],
    role: CertificateRole,
    expected_pin: &str,
) -> Result<T> {
    ensure!(
        envelope.is_bounded(),
        "unsupported or oversized pairing envelope"
    );
    let actual_pin = certificate_pin(certificate_der, role, unix_now()?)?;
    ensure!(actual_pin == expected_pin, "pairing identity mismatch");
    let payload = STANDARD
        .decode(&envelope.payload)
        .context("invalid pairing payload")?;
    ensure!(
        payload.len() <= MAX_REMOTE_ENVELOPE_BYTES / 2,
        "pairing payload exceeds bound"
    );
    let signed = STANDARD
        .decode(&envelope.signature)
        .context("invalid pairing signature")?;
    let (_, cert) = parse_x509_certificate(certificate_der)
        .map_err(|_| anyhow::anyhow!("invalid certificate DER"))?;
    signature::UnparsedPublicKey::new(
        &signature::ECDSA_P256_SHA256_ASN1,
        cert.public_key().subject_public_key.data.as_ref(),
    )
    .verify(&payload, &signed)
    .map_err(|_| anyhow::anyhow!("invalid pairing signature"))?;
    Ok(serde_json::from_slice(&payload).context("invalid signed pairing fields")?)
}

/// Random challenge is not a Human capability or a work authentication token.
pub fn challenge() -> Result<String> {
    use ring::rand::SecureRandom;
    let mut bytes = [0u8; 32];
    SystemRandom::new()
        .fill(&mut bytes)
        .map_err(|_| anyhow::anyhow!("cannot generate pairing challenge"))?;
    Ok(STANDARD.encode(bytes))
}

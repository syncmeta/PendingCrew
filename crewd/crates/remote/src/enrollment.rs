//! Version1 link and proof primitives; they never grant work authority.
use crate::{CertificateRole, certificate_pin, decode_certificate, unix_now};
use anyhow::{Result, ensure};
use base64::{Engine, engine::general_purpose::URL_SAFE_NO_PAD};
use crewd_proto::*;
use ring::{
    digest,
    rand::{SecureRandom, SystemRandom},
};

/// Random short-lived secret. Intentionally no Display/Debug or argv transport.
pub fn enrollment_token() -> Result<String> {
    let mut bytes = [0u8; 32];
    SystemRandom::new()
        .fill(&mut bytes)
        .map_err(|_| anyhow::anyhow!("enrollment randomness unavailable"))?;
    Ok(URL_SAFE_NO_PAD.encode(bytes))
}
fn token_bytes(token: &str) -> Result<Vec<u8>> {
    ensure!(token.len() == 43, "invalid enrollment secret");
    let bytes = URL_SAFE_NO_PAD
        .decode(token)
        .map_err(|_| anyhow::anyhow!("invalid enrollment secret"))?;
    ensure!(
        bytes.len() == 32 && URL_SAFE_NO_PAD.encode(&bytes) == token,
        "invalid enrollment secret"
    );
    Ok(bytes)
}
pub fn enrollment_token_hash(token: &str) -> Result<String> {
    let bytes = token_bytes(token)?;
    Ok(digest::digest(&digest::SHA256, &bytes)
        .as_ref()
        .iter()
        .map(|x| format!("{x:02x}"))
        .collect())
}
pub fn enrollment_token_matches(token: &str, expected_hash: &str) -> bool {
    let Ok(actual) = enrollment_token_hash(token) else {
        return false;
    };
    use subtle::ConstantTimeEq;
    bool::from(actual.as_bytes().ct_eq(expected_hash.as_bytes()))
}
fn validate_link(link: &EnrollmentLink) -> Result<()> {
    ensure!(
        link.version == ENROLLMENT_FORMAT
            && valid_remote_uuid(&link.enrollment_id)
            && valid_remote_uuid(&link.server.authority_id)
            && valid_remote_uuid(&link.server.db_id)
            && link.endpoint.is_valid()
            && valid_spki_pin(&link.server.spki_sha256),
        "invalid enrollment identity"
    );
    token_bytes(&link.token)?;
    ensure!(
        !link.computer_label.trim().is_empty()
            && link.computer_label.len() <= MAX_REMOTE_LABEL_BYTES
            && !link.computer_label.chars().any(char::is_control)
            && !link.expires_at.is_empty()
            && link.expires_at.len() <= 64,
        "invalid enrollment metadata"
    );
    let der = decode_certificate(&link.server.certificate_der_base64)?;
    ensure!(
        certificate_pin(&der, CertificateRole::Server, unix_now()?)? == link.server.spki_sha256,
        "enrollment certificate does not match confirmed identity"
    );
    Ok(())
}
pub fn encode_enrollment_link(link: &EnrollmentLink) -> Result<String> {
    validate_link(link)?;
    let uri = format!(
        "{ENROLLMENT_URI_PREFIX}{}",
        URL_SAFE_NO_PAD.encode(serde_json::to_vec(link)?)
    );
    ensure!(
        uri.len() <= MAX_ENROLLMENT_URI_BYTES,
        "enrollment link exceeds bound"
    );
    Ok(uri)
}
pub fn decode_enrollment_link(uri: &str) -> Result<EnrollmentLink> {
    ensure!(
        uri.len() <= MAX_ENROLLMENT_URI_BYTES,
        "enrollment link exceeds bound"
    );
    let encoded = uri
        .strip_prefix(ENROLLMENT_URI_PREFIX)
        .ok_or_else(|| anyhow::anyhow!("invalid enrollment route"))?;
    let bytes = URL_SAFE_NO_PAD
        .decode(encoded)
        .map_err(|_| anyhow::anyhow!("invalid enrollment encoding"))?;
    ensure!(
        URL_SAFE_NO_PAD.encode(&bytes) == encoded,
        "noncanonical enrollment encoding"
    );
    let link: EnrollmentLink =
        serde_json::from_slice(&bytes).map_err(|_| anyhow::anyhow!("invalid enrollment fields"))?;
    validate_link(&link)?;
    Ok(link)
}

#[cfg(test)]
mod tests {
    use super::*;
    fn link() -> EnrollmentLink {
        let certificate = crate::CertificateIdentity::generate().unwrap();
        EnrollmentLink {
            version: 1,
            enrollment_id: "11111111-1111-4111-8111-111111111111".into(),
            server: RemotePublicIdentity {
                authority_id: "b0150000-0000-4000-8000-000000000001".into(),
                db_id: "5ad89300-0000-4000-8000-000000000001".into(),
                certificate_der_base64: certificate.certificate_base64(),
                spki_sha256: certificate.pin().into(),
            },
            endpoint: RemoteEndpoint {
                host: "127.0.0.1".into(),
                port: 49381,
            },
            expires_at: "2030-01-01T00:00:00Z".into(),
            computer_label: "Private host".into(),
            token: enrollment_token().unwrap(),
        }
    }
    #[test]
    fn enrollment_link_preserves_exact_authority_and_secret_without_relaxing_parser() {
        let original = link();
        let uri = encode_enrollment_link(&original).unwrap();
        let decoded = decode_enrollment_link(&uri).unwrap();
        assert_eq!(
            serde_json::to_value(&decoded).unwrap(),
            serde_json::to_value(&original).unwrap()
        );
        for invalid in [
            format!("{uri}="),
            format!("{uri}?debug=true"),
            uri.replace("enroll/v1/", "enroll/v2/"),
            "x".repeat(MAX_ENROLLMENT_URI_BYTES + 1),
        ] {
            assert!(decode_enrollment_link(&invalid).is_err());
        }
        let mut value = serde_json::to_value(&original).unwrap();
        value["extra"] = true.into();
        assert!(
            decode_enrollment_link(&format!(
                "{ENROLLMENT_URI_PREFIX}{}",
                URL_SAFE_NO_PAD.encode(serde_json::to_vec(&value).unwrap())
            ))
            .is_err()
        );
        // Duplicate fields are rejected before they can choose one identity silently.
        let text = serde_json::to_string(&original)
            .unwrap()
            .replacen("{", "{\"version\":1,", 1);
        assert!(
            decode_enrollment_link(&format!(
                "{ENROLLMENT_URI_PREFIX}{}",
                URL_SAFE_NO_PAD.encode(text)
            ))
            .is_err()
        );
        let mut wrong = original.clone();
        wrong.server.spki_sha256 = "0".repeat(64);
        assert!(encode_enrollment_link(&wrong).is_err());
        wrong = original.clone();
        wrong.endpoint.port = 0;
        assert!(encode_enrollment_link(&wrong).is_err());
        wrong = original;
        wrong.version = 2;
        assert!(encode_enrollment_link(&wrong).is_err());
    }
    #[test]
    fn enrollment_token_hash_is_fixed_and_changed_or_noncanonical_secret_never_matches() {
        let token = enrollment_token().unwrap();
        let hash = enrollment_token_hash(&token).unwrap();
        assert!(valid_spki_pin(&hash));
        assert!(enrollment_token_matches(&token, &hash));
        assert!(!enrollment_token_matches(
            &enrollment_token().unwrap(),
            &hash
        ));
        assert!(!enrollment_token_matches(&(token + "="), &hash));
        assert!(!enrollment_token_matches("not-a-secret", &hash));
    }
}

//! M6 authenticated remote identities and out-of-band pairing. No private key is wire data.
use schemars::JsonSchema;
use serde::{Deserialize, Serialize};

pub const REMOTE_FORMAT: u32 = 1;
pub const MAX_REMOTE_LABEL_BYTES: usize = 256;
pub const MAX_REMOTE_HOST_BYTES: usize = 255;
pub const MAX_REMOTE_ENVELOPE_BYTES: usize = 131072;
pub const MAX_REMOTE_CERTIFICATE_BYTES: usize = 65536;
pub const MAX_REMOTE_GRANTS: usize = 256;
pub const MAX_REMOTE_PENDING_INVITATIONS: usize = 32;

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct RemoteEndpoint {
    pub host: String,
    pub port: u16,
}
impl RemoteEndpoint {
    pub fn is_valid(&self) -> bool {
        self.port != 0
            && !self.host.is_empty()
            && self.host.len() <= MAX_REMOTE_HOST_BYTES
            && self.host.trim() == self.host
            && !self.host.chars().any(char::is_control)
    }
}

pub fn valid_remote_uuid(id: &str) -> bool {
    id.len() == 36
        && id.bytes().enumerate().all(|(i, b)| {
            if matches!(i, 8 | 13 | 18 | 23) {
                b == b'-'
            } else {
                b.is_ascii_digit() || (b'a'..=b'f').contains(&b)
            }
        })
}

/// The DER SPKI SHA256 pin includes its ASN.1 algorithm identifier, not just EC point bytes.
pub fn valid_spki_pin(pin: &str) -> bool {
    pin.len() == 64
        && pin
            .bytes()
            .all(|b| b.is_ascii_digit() || (b'a'..=b'f').contains(&b))
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct RemotePublicIdentity {
    pub authority_id: String,
    pub db_id: String,
    pub certificate_der_base64: String,
    pub spki_sha256: String,
}

/// Signed exact decoded payload bytes. A signature never supplies caller permission.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct RemoteSignedEnvelope {
    pub format: u32,
    pub payload: String,
    pub signature: String,
}
impl RemoteSignedEnvelope {
    pub fn is_bounded(&self) -> bool {
        self.format == REMOTE_FORMAT
            && !self.payload.is_empty()
            && self.payload.len() <= MAX_REMOTE_ENVELOPE_BYTES
            && !self.signature.is_empty()
            && self.signature.len() <= 256
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct RemoteInvitation {
    pub invitation_id: String,
    pub server: RemotePublicIdentity,
    pub endpoint: RemoteEndpoint,
    pub challenge: String,
    pub expires_at: String,
    pub computer_label: String,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct RemotePairingResponse {
    pub invitation_id: String,
    pub authority_id: String,
    pub db_id: String,
    pub challenge: String,
    pub client_certificate_der_base64: String,
    pub client_spki_sha256: String,
    pub device_label: String,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct RemoteGrantReceipt {
    pub invitation_id: String,
    pub grant_id: String,
    pub authority_id: String,
    pub db_id: String,
    pub server_spki_sha256: String,
    pub client_spki_sha256: String,
    pub grant_revision: String,
    pub accepted_at: String,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "camelCase")]
pub enum RemoteGrantScope {
    OwnerWorkspace,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "camelCase")]
pub struct RemoteGrantSummary {
    pub id: String,
    pub scope: RemoteGrantScope,
    pub device_label: String,
    pub client_spki_sha256: String,
    pub revision: String,
    pub created_at: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub revoked_at: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub revoke_note: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "camelCase")]
pub struct RemoteConfiguration {
    pub revision: String,
    pub enabled: bool,
    pub listen_host: String,
    pub port: u16,
}

/// Actual listener outcome; persisted enabled intent is not proof of a bound socket.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "camelCase")]
pub struct RemoteStatus {
    pub configuration: RemoteConfiguration,
    pub listening: bool,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub endpoint: Option<RemoteEndpoint>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub identity: Option<RemotePublicIdentity>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub failure: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct RemoteConfigureParams {
    pub request_id: String,
    pub expected_revision: String,
    pub enabled: bool,
    pub listen_host: String,
    pub port: u16,
}
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct RemoteInviteParams {
    pub request_id: String,
    #[serde(default)]
    pub device_label: Option<String>,
}
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct RemoteAcceptParams {
    pub request_id: String,
    pub response_envelope: RemoteSignedEnvelope,
}
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct RemoteRevokeParams {
    pub grant_id: String,
    pub expected_revision: String,
    pub note: String,
}
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "camelCase")]
pub struct RemoteAcceptResult {
    pub request_id: String,
    pub reused: bool,
    pub receipt: RemoteSignedEnvelope,
    pub grant: RemoteGrantSummary,
}

/// Constructed by the authenticated server adapter. Never accepted as request identity.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "camelCase")]
pub struct RemoteHelloBinding {
    pub authority_id: String,
    pub db_id: String,
    pub runtime_binding_id: String,
    pub grant_id: String,
    pub grant_revision: String,
}

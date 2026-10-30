//! Closed, independently versioned enrollment surface. Never a work RPC capability.
use crate::{
    RemoteAcceptResult, RemoteEndpoint, RemoteGrantSummary, RemotePublicIdentity,
    RemoteSignedEnvelope,
};
use schemars::JsonSchema;
use serde::{Deserialize, Serialize};

pub const ENROLLMENT_FORMAT: u32 = 1;
pub const ENROLLMENT_ALPN: &[u8] = b"pcw-enroll/1";
pub const ENROLLMENT_URI_PREFIX: &str = "pendingcrew://enroll/v1/";
pub const MAX_ENROLLMENT_URI_BYTES: usize = 8192;
pub const MAX_ENROLLMENT_FRAME_BYTES: usize = 262144;
pub const ENROLLMENT_TTL_SECONDS: i64 = 300;
pub const ENROLLMENT_LOOKUP_DOMAIN: &str = "pendingcrew.enrollment.lookup.v1";

// Secret-bearing structs intentionally have no Debug implementation.
#[derive(Clone, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct EnrollmentLink {
    pub version: u32,
    pub enrollment_id: String,
    pub server: RemotePublicIdentity,
    pub endpoint: RemoteEndpoint,
    pub expires_at: String,
    pub computer_label: String,
    pub token: String,
}
#[derive(Clone, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct EnrollmentBeginParams {
    pub enrollment_id: String,
    pub request_id: String,
    pub token: String,
    pub client_certificate_der_base64: String,
    pub client_spki_sha256: String,
    pub device_label: String,
}
#[derive(Debug, Clone, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct EnrollmentBeginResult {
    pub request_id: String,
    pub invitation: RemoteSignedEnvelope,
}
#[derive(Clone, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct EnrollmentCommitParams {
    pub enrollment_id: String,
    pub request_id: String,
    pub response_envelope: RemoteSignedEnvelope,
}
#[derive(Clone, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct EnrollmentLookupParams {
    pub enrollment_id: String,
    pub request_id: String,
    pub proof: RemoteSignedEnvelope,
}
#[derive(Clone, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct EnrollmentLookupProof {
    pub format: u32,
    pub domain: String,
    pub enrollment_id: String,
    pub request_id: String,
    pub authority_id: String,
    pub db_id: String,
    pub client_certificate_der_base64: String,
    pub client_spki_sha256: String,
    // None allows recovery of the original challenge when the first response was lost.
    pub begin_digest: Option<String>,
}
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "camelCase")]
pub enum EnrollmentLookupState {
    Pending,
    Accepted,
    Expired,
    Revoked,
    NotFound,
}
#[derive(Debug, Clone, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct EnrollmentLookupResult {
    pub request_id: String,
    pub state: EnrollmentLookupState,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub pending_invitation: Option<RemoteSignedEnvelope>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub accepted: Option<RemoteAcceptResult>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub grant_state: Option<RemoteGrantSummary>,
}

/// Local root management only, never accepted by the anonymous enrollment router.
#[derive(Clone, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct HostEnrollmentCreateParams {
    pub request_id: String,
    pub enrollment_id: String,
    pub token_hash: String,
    pub computer_label: String,
    pub endpoint: RemoteEndpoint,
}
#[derive(Debug, Clone, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct HostEnrollmentCreated {
    pub request_id: String,
    pub enrollment_id: String,
    pub server: RemotePublicIdentity,
    pub endpoint: RemoteEndpoint,
    pub expires_at: String,
    pub computer_label: String,
}
#[derive(Clone, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct HostEnrollmentLookupParams {
    pub request_id: String,
    pub enrollment_id: String,
}

#[derive(Debug, Clone, Serialize, Deserialize, JsonSchema)]
#[serde(tag = "state", rename_all = "camelCase", deny_unknown_fields)]
pub enum HostEnrollmentLookupResult {
    Created { created: Box<HostEnrollmentCreated> },
    NotFound,
}

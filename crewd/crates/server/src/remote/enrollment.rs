//! Narrow registration phases. Never dispatches work or manufactures a Human peer.
use super::*;
use base64::Engine;
use crewd_store::enrollment as records;

pub(super) struct CommitGuard {
    pub(super) enrollment_id: String,
    pub(super) digest: String,
    pub(super) revision: String,
    pub(super) epoch: u64,
}
fn invalid_record() -> crewd_store::StoreError {
    crewd_store::StoreError::Invalid("enrollment authority or original request changed".into())
}
fn digest(bytes: &[u8]) -> String {
    ring::digest::digest(&ring::digest::SHA256, bytes)
        .as_ref()
        .iter()
        .map(|v| format!("{v:02x}"))
        .collect()
}
impl CommitGuard {
    pub(super) fn check_tx(
        &self,
        owner: &Manager,
        conn: &rusqlite::Connection,
        response: &RemotePairingResponse,
        request: &str,
    ) -> crewd_store::Result<()> {
        owner.enrollment_live_tx(conn, &self.revision, self.epoch)?;
        let original = records::get(conn, &response.authority_id, &self.enrollment_id)?
            .ok_or_else(invalid_record)?;
        let reserved = original.reservation.as_ref().ok_or_else(invalid_record)?;
        if original.committed.is_some()
            || original.expires_unix <= chrono::Utc::now().timestamp()
            || response.invitation_id != self.enrollment_id
            || reserved.request_id != request
            || reserved.client_certificate_der_base64 != response.client_certificate_der_base64
            || reserved.client_spki_sha256 != response.client_spki_sha256
            || reserved.device_label != response.device_label
        {
            return Err(invalid_record());
        }
        Ok(())
    }
}
impl Manager {
    fn enrollment_live_tx(
        &self,
        conn: &rusqlite::Connection,
        revision: &str,
        epoch: u64,
    ) -> crewd_store::Result<()> {
        let config = facts::configuration(conn)?;
        if *self.drain.borrow()
            || self.listener_epoch.load(Ordering::SeqCst) != epoch
            || !config.enabled
            || config.revision != revision
        {
            return Err(invalid_record());
        }
        Ok(())
    }
    pub(super) async fn enrollment_route(
        self: &Arc<Self>,
        method: &str,
        params: Value,
        revision: &str,
        epoch: u64,
    ) -> std::result::Result<Value, RpcError> {
        match method {
            "enrollment.begin" => {
                self.enrollment_begin(parse_params(params)?, revision, epoch)
                    .await
            }
            "enrollment.commit" => {
                self.enrollment_commit(parse_params(params)?, revision, epoch)
                    .await
            }
            "enrollment.lookup" => {
                self.enrollment_lookup(parse_params(params)?, revision, epoch)
                    .await
            }
            _ => Err(denied()),
        }
    }
    async fn enrollment_begin(
        self: &Arc<Self>,
        p: EnrollmentBeginParams,
        revision: &str,
        epoch: u64,
    ) -> std::result::Result<Value, RpcError> {
        request_id(&p.enrollment_id)?;
        request_id(&p.request_id)?;
        if p.device_label.trim().is_empty()
            || p.device_label.len() > MAX_REMOTE_LABEL_BYTES
            || p.device_label.chars().any(char::is_control)
        {
            return Err(bad("设备名称无效"));
        }
        let der = crewd_remote::decode_certificate(&p.client_certificate_der_base64)
            .map_err(|_| denied())?;
        let pin = crewd_remote::certificate_pin(
            &der,
            crewd_remote::CertificateRole::Client,
            crewd_remote::unix_now().map_err(dispatch::internal)?,
        )
        .map_err(|_| denied())?;
        if pin != p.client_spki_sha256 {
            return Err(denied());
        }
        let token_hash = crewd_remote::enrollment_token_hash(&p.token).map_err(|_| denied())?;
        // Only a digest sees the secret; raw params never enter witnesses or errors.
        let input_digest = digest(&serde_json::to_vec(&p).map_err(dispatch::internal)?);
        let _guard = self.mutation.lock().await;
        let identity = self.ensure_identity().await?;
        let id = p.enrollment_id.clone();
        let authority = identity.public.authority_id.clone();
        let record = self
            .db
            .call(move |s| records::get(s.conn(), &authority, &id))
            .await
            .map_err(store_error)?
            .map_err(store_error)?
            .ok_or_else(denied)?;
        if !crewd_remote::enrollment_token_matches(&p.token, &record.token_hash)
            || record.token_hash != token_hash
        {
            return Err(denied());
        }
        if let Some(reserved) = record.reservation {
            if reserved.request_id != p.request_id || reserved.input_digest != input_digest {
                return Err(conflict());
            }
            return serde_json::to_value(EnrollmentBeginResult {
                request_id: p.request_id,
                invitation: reserved.invitation,
            })
            .map_err(dispatch::internal);
        }
        let now = chrono::Utc::now();
        if record.expires_unix <= now.timestamp() {
            return Err(denied());
        }
        let challenge = crewd_remote::challenge().map_err(dispatch::internal)?;
        let invitation = identity
            .certificate
            .sign(&RemoteInvitation {
                invitation_id: p.enrollment_id.clone(),
                server: identity.public.clone(),
                endpoint: record.endpoint.clone(),
                challenge: challenge.clone(),
                expires_at: chrono::DateTime::from_timestamp(record.expires_unix, 0)
                    .ok_or_else(denied)?
                    .to_rfc3339(),
                computer_label: record.computer_label.clone(),
            })
            .map_err(dispatch::internal)?;
        let invitation_digest = digest(
            &base64::engine::general_purpose::STANDARD
                .decode(&invitation.payload)
                .map_err(|_| denied())?,
        );
        let reserved = records::Reservation {
            request_id: p.request_id.clone(),
            input_digest,
            client_certificate_der_base64: p.client_certificate_der_base64,
            client_spki_sha256: p.client_spki_sha256,
            device_label: p.device_label,
            invitation: invitation.clone(),
            invitation_digest,
        };
        let owner = self.clone();
        let rev = revision.to_string();
        let authority = identity.public.authority_id.clone();
        let id = p.enrollment_id;
        let envelope = serde_json::to_string(&invitation).map_err(dispatch::internal)?;
        self.db
            .call(move |s| {
                s.with_tx(|tx| {
                    owner.enrollment_live_tx(tx, &rev, epoch)?;
                    if facts::request_witness(tx, &reserved.request_id, &authority)?.is_some() {
                        return Err(invalid_record());
                    }
                    let current = records::get(tx, &authority, &id)?.ok_or_else(invalid_record)?;
                    if current.expires_unix <= chrono::Utc::now().timestamp() {
                        return Err(invalid_record());
                    }
                    facts::issue_invitation(
                        tx,
                        facts::NewInvitation {
                            id: &id,
                            authority: &authority,
                            challenge: &challenge,
                            envelope_json: &envelope,
                            expires_unix: current.expires_unix,
                            now_unix: chrono::Utc::now().timestamp(),
                            at: &chrono::Utc::now().to_rfc3339(),
                        },
                    )?;
                    records::reserve(
                        tx,
                        &authority,
                        &id,
                        reserved,
                        chrono::Utc::now().timestamp(),
                    )?;
                    Ok(())
                })
            })
            .await
            .map_err(store_error)?
            .map_err(write_error)?;
        serde_json::to_value(EnrollmentBeginResult {
            request_id: p.request_id,
            invitation,
        })
        .map_err(dispatch::internal)
    }
    async fn enrollment_commit(
        self: &Arc<Self>,
        p: EnrollmentCommitParams,
        revision: &str,
        epoch: u64,
    ) -> std::result::Result<Value, RpcError> {
        request_id(&p.enrollment_id)?;
        request_id(&p.request_id)?;
        let digest = digest(&serde_json::to_vec(&p).map_err(dispatch::internal)?);
        let _guard = self.mutation.lock().await;
        let identity = self.ensure_identity().await?;
        let id = p.enrollment_id.clone();
        let authority = identity.public.authority_id.clone();
        let record = self
            .db
            .call(move |s| records::get(s.conn(), &authority, &id))
            .await
            .map_err(store_error)?
            .map_err(store_error)?
            .ok_or_else(denied)?;
        let original = record.reservation.ok_or_else(denied)?;
        if original.request_id != p.request_id {
            return Err(denied());
        }
        if let Some(old) = record.committed {
            if old.input_digest != digest {
                return Err(conflict());
            }
            // Authenticate the original signed response even for idempotent reads.
            let certificate =
                crewd_remote::decode_certificate(&original.client_certificate_der_base64)
                    .map_err(|_| denied())?;
            let _: RemotePairingResponse = crewd_remote::verify_envelope(
                &p.response_envelope,
                &certificate,
                crewd_remote::CertificateRole::Client,
                &original.client_spki_sha256,
            )
            .map_err(|_| denied())?;
            return serde_json::to_value(old.result).map_err(dispatch::internal);
        }
        self.accept_locked(
            RemoteAcceptParams {
                request_id: p.request_id,
                response_envelope: p.response_envelope,
            },
            Some(CommitGuard {
                enrollment_id: p.enrollment_id,
                digest,
                revision: revision.into(),
                epoch,
            }),
        )
        .await
    }
    async fn enrollment_lookup(
        self: &Arc<Self>,
        p: EnrollmentLookupParams,
        revision: &str,
        epoch: u64,
    ) -> std::result::Result<Value, RpcError> {
        request_id(&p.enrollment_id)?;
        request_id(&p.request_id)?;
        if !p.proof.is_bounded() {
            return Err(denied());
        }
        let bytes = base64::engine::general_purpose::STANDARD
            .decode(&p.proof.payload)
            .map_err(|_| denied())?;
        let untrusted: EnrollmentLookupProof =
            serde_json::from_slice(&bytes).map_err(|_| denied())?;
        let certificate =
            crewd_remote::decode_certificate(&untrusted.client_certificate_der_base64)
                .map_err(|_| denied())?;
        let proof: EnrollmentLookupProof = crewd_remote::verify_envelope(
            &p.proof,
            &certificate,
            crewd_remote::CertificateRole::Client,
            &untrusted.client_spki_sha256,
        )
        .map_err(|_| denied())?;
        let identity = self.ensure_identity().await?;
        if proof.format != 1
            || proof.domain != ENROLLMENT_LOOKUP_DOMAIN
            || proof.enrollment_id != p.enrollment_id
            || proof.request_id != p.request_id
            || proof.authority_id != identity.public.authority_id
            || proof.db_id != identity.public.db_id
        {
            return Err(denied());
        }
        let owner = self.clone();
        let rev = revision.to_string();
        let authority = proof.authority_id.clone();
        let id = p.enrollment_id;
        self.db
            .call(move |s| {
                owner.enrollment_live_tx(s.conn(), &rev, epoch)?;
                let mut result = EnrollmentLookupResult {
                    request_id: proof.request_id.clone(),
                    state: EnrollmentLookupState::NotFound,
                    pending_invitation: None,
                    accepted: None,
                    grant_state: None,
                };
                if let Some(record) = records::get(s.conn(), &authority, &id)? {
                    let Some(reserved) = record.reservation else {
                        return serde_json::to_value(result).map_err(|_| invalid_record());
                    };
                    if reserved.request_id != proof.request_id
                        || reserved.client_spki_sha256 != proof.client_spki_sha256
                        || reserved.client_certificate_der_base64
                            != proof.client_certificate_der_base64
                        || proof
                            .begin_digest
                            .is_some_and(|d| d != reserved.invitation_digest)
                    {
                        return Err(invalid_record());
                    }
                    if let Some(committed) = record.committed {
                        let grant = facts::grant(s.conn(), &committed.result.grant.id, &authority)?
                            .ok_or_else(invalid_record)?;
                        result.state = if grant.revoked_at.is_some() {
                            EnrollmentLookupState::Revoked
                        } else {
                            EnrollmentLookupState::Accepted
                        };
                        result.accepted = Some(committed.result);
                        result.grant_state = Some(grant);
                    } else {
                        result.state = if record.expires_unix <= chrono::Utc::now().timestamp() {
                            EnrollmentLookupState::Expired
                        } else {
                            EnrollmentLookupState::Pending
                        };
                        result.pending_invitation = Some(reserved.invitation)
                    }
                }
                serde_json::to_value(result).map_err(|_| invalid_record())
            })
            .await
            .map_err(store_error)?
            .map_err(write_error)
    }
}

//! Server-private credential persistence. Never serialize this file type as RPC data.
use anyhow::{Context, Result, ensure};
use base64::{Engine, engine::general_purpose::STANDARD};
use crewd_proto::RemotePublicIdentity;
use crewd_remote::CertificateIdentity;
use serde::{Deserialize, Serialize};
use std::{
    io::Read,
    os::unix::fs::{OpenOptionsExt, PermissionsExt},
    path::Path,
};

#[derive(Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
struct SecretFile {
    format: u32,
    authority_id: String,
    db_id: String,
    certificate_der: String,
    key_pkcs8_der: String,
}
/// Credentials are not Debug/Serialize and only expose their public binding.
pub(super) struct Identity {
    pub public: RemotePublicIdentity,
    pub certificate: CertificateIdentity,
}
impl Identity {
    pub fn load_or_create(
        directory: &Path,
        db_id: &str,
        bound: Option<&RemotePublicIdentity>,
        grants_exist: bool,
    ) -> Result<Self> {
        let path = directory.join("identity.json");
        match std::fs::symlink_metadata(directory) {
            Ok(metadata) => ensure!(
                metadata.is_dir() && !metadata.file_type().is_symlink(),
                "remote identity directory is invalid"
            ),
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => {
                std::fs::create_dir(directory)
                    .context("cannot create remote identity directory")?;
            }
            Err(_) => anyhow::bail!("cannot inspect remote identity directory"),
        }
        std::fs::set_permissions(directory, std::fs::Permissions::from_mode(0o700))?;
        let file = match std::fs::OpenOptions::new()
            .read(true)
            .custom_flags(libc::O_NOFOLLOW)
            .open(&path)
        {
            Ok(file) => Some(file),
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => None,
            Err(_) => anyhow::bail!("cannot read remote identity"),
        };
        let saved = if let Some(file) = file {
            ensure!(
                file.metadata()?.is_file(),
                "remote identity is not a regular file"
            );
            // Tighten an existing credential through the open descriptor, not
            // a second path lookup that could follow a substituted symlink.
            file.set_permissions(std::fs::Permissions::from_mode(0o600))?;
            let mut bytes = Vec::new();
            file.take(262145).read_to_end(&mut bytes)?;
            ensure!(bytes.len() <= 262144, "remote identity exceeds bound");
            serde_json::from_slice::<SecretFile>(&bytes)
                .map_err(|_| anyhow::anyhow!("invalid remote identity file"))?
        } else {
            // A missing private credential is not an invitation to regenerate trust.
            ensure!(
                bound.is_none() && !grants_exist,
                "existing remote identity has no private credential; pair again after local recovery"
            );
            let certificate = CertificateIdentity::generate()?;
            let saved = SecretFile {
                format: 1,
                authority_id: uuid::Uuid::new_v4().to_string(),
                db_id: db_id.into(),
                certificate_der: certificate.certificate_base64(),
                key_pkcs8_der: STANDARD.encode(certificate.private_key_der()),
            };
            crewd_store::fsutil::atomic_write(&path, &serde_json::to_vec(&saved)?, 0o600)?;
            saved
        };
        ensure!(
            saved.format == 1
                && crewd_proto::valid_remote_uuid(&saved.authority_id)
                && saved.db_id == db_id,
            "remote identity belongs to another database or format"
        );
        let certificate = CertificateIdentity::from_der(
            crewd_remote::decode_certificate(&saved.certificate_der)?,
            STANDARD
                .decode(&saved.key_pkcs8_der)
                .map_err(|_| anyhow::anyhow!("invalid private credential encoding"))?,
        )?;
        let public = RemotePublicIdentity {
            authority_id: saved.authority_id,
            db_id: saved.db_id,
            certificate_der_base64: certificate.certificate_base64(),
            spki_sha256: certificate.pin().into(),
        };
        if let Some(bound) = bound {
            ensure!(
                *bound == public,
                "remote public and private identity binding differ"
            );
        } else {
            ensure!(!grants_exist, "remote grants have no identity binding");
        }
        Ok(Self {
            public,
            certificate,
        })
    }
}

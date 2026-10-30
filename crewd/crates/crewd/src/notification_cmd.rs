//! Secret-bounded Local Human configuration; never an APNs sender or retry path.
use anyhow::{Result, bail};
use crewd_proto::notifications::{
    NotificationConfigureParams, NotificationRequestLookup, NotificationStatus,
};
use serde::{Deserialize, Serialize};
use serde_json::{Value, json};
use std::fs::{File, OpenOptions};
use std::io::{Read, Write};
use std::os::unix::fs::{MetadataExt, OpenOptionsExt};
use std::path::{Path, PathBuf};

const LIMIT: u64 = 16_384;
const OPERATION: &str = "notification.configure";

#[derive(clap::Args)]
pub struct NotificationArgs {
    #[command(subcommand)]
    action: Action,
}
#[derive(clap::Subcommand)]
enum Action {
    /// Read configuration JSON from bounded stdin or an owned0600 file, never argv.
    Configure {
        #[arg(long, value_name = "PATH")]
        input_file: Option<PathBuf>,
    },
    /// Query only the original UUID with its retained local witness; never resubmit.
    Lookup {
        #[arg(long)]
        request_id: String,
    },
}

/// Same non-secret witness shape as the existing Linux host CLI. It is not a
/// provider configuration backup or execution authority.
#[derive(Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
struct Witness {
    format: u8,
    kind: String,
    request_id: String,
    payload_sha256: String,
}
fn private_file(file: &File) -> Result<()> {
    let m = file.metadata()?;
    if !m.is_file() || m.uid() != unsafe { libc::geteuid() } || m.mode() & 0o7777 != 0o600 {
        bail!("notification input/witness requires an owned0600 regular file");
    }
    Ok(())
}
fn open_private(path: &Path) -> Result<File> {
    let f = OpenOptions::new()
        .read(true)
        .custom_flags(libc::O_NOFOLLOW | libc::O_CLOEXEC | libc::O_NONBLOCK)
        .open(path)
        .map_err(|_| anyhow::anyhow!("notification private file unavailable"))?;
    private_file(&f)?;
    Ok(f)
}
fn bounded(reader: impl Read) -> Result<Vec<u8>> {
    let mut bytes = Vec::new();
    reader
        .take(LIMIT + 1)
        .read_to_end(&mut bytes)
        .map_err(|_| anyhow::anyhow!("notification input could not be read"))?;
    if bytes.is_empty() || bytes.len() > LIMIT as usize {
        bail!("notification input must contain at most16KiB JSON");
    }
    Ok(bytes)
}
fn configuration(input: Option<&Path>) -> Result<Value> {
    let bytes = match input {
        Some(path) => bounded(open_private(path)?)?,
        None => bounded(std::io::stdin().lock())?,
    };
    // Never propagate serde's input-dependent diagnostics or Debug the provider.
    let p: NotificationConfigureParams = serde_json::from_slice(&bytes)
        .map_err(|_| anyhow::anyhow!("invalid typed notification configuration"))?;
    if !crewd_proto::valid_remote_uuid(&p.request_id)
        || crewd_store::notifications::number(&p.expected_revision, true).is_err()
    {
        bail!("canonical notification request UUID and revision required");
    }
    if let Some(provider) = &p.provider {
        let valid_id = |s: &str| {
            s.len() == 10
                && s.bytes()
                    .all(|b| b.is_ascii_uppercase() || b.is_ascii_digit())
        };
        let secret = provider.private_key_pkcs8_base64.as_bytes();
        if !valid_id(&provider.team_id)
            || !valid_id(&provider.key_id)
            || provider.topic.is_empty()
            || provider.topic.len() > 255
            || !provider
                .topic
                .bytes()
                .all(|b| b.is_ascii_alphanumeric() || b".-".contains(&b))
            || secret.is_empty()
            || secret.len() > 10_924
            || secret.len() % 4 != 0
            || !secret
                .iter()
                .all(|b| b.is_ascii_alphanumeric() || b"+/=".contains(b))
        {
            bail!("invalid notification provider fields");
        }
    }
    serde_json::to_value(p).map_err(|_| anyhow::anyhow!("invalid notification configuration"))
}
fn witness_dir(data: &Path, create: bool) -> Result<PathBuf> {
    // Existing canonical Core root only. The original helper also rejects
    // symlinked ancestors, foreign ownership and group/world access.
    let path = data.join("notification-cli");
    for (directory, may_create) in [(data, false), (path.as_path(), create)] {
        if !may_create {
            std::fs::symlink_metadata(directory)
                .map_err(|_| anyhow::anyhow!("notification witness directory unavailable"))?;
        }
        crewd_store::team_data::private_dir(directory).map_err(|_| {
            anyhow::anyhow!("notification witness requires canonical owned0700 directories")
        })?;
        if directory == path && std::fs::symlink_metadata(directory)?.mode() & 0o7777 != 0o700 {
            bail!("notification witness requires canonical owned0700 directories");
        }
    }
    Ok(path)
}
fn save_witness(data: &Path, request: &str, params: &Value) -> Result<()> {
    let directory = witness_dir(data, true)?;
    let path = directory.join(format!("notification-{request}.json"));
    let value = Witness {
        format: 1,
        kind: "notificationConfiguration".into(),
        request_id: request.into(),
        payload_sha256: crewd_store::notifications::hash(&serde_json::to_vec(params)?),
    };
    let mut f = OpenOptions::new().write(true).create_new(true).mode(0o600)
        .custom_flags(libc::O_NOFOLLOW | libc::O_CLOEXEC).open(path)
        .map_err(|_| anyhow::anyhow!("original notification UUID witness exists or is unavailable; use lookup, never overwrite or resend"))?;
    private_file(&f)?;
    f.write_all(&serde_json::to_vec(&value)?)?;
    f.write_all(b"\n")?;
    f.sync_all()?;
    OpenOptions::new()
        .read(true)
        .custom_flags(libc::O_DIRECTORY | libc::O_NOFOLLOW | libc::O_CLOEXEC)
        .open(directory)?
        .sync_all()?;
    Ok(())
}
fn require_witness(data: &Path, request: &str) -> Result<()> {
    let directory = witness_dir(data, false)?;
    let f = open_private(&directory.join(format!("notification-{request}.json")))?;
    let value: Witness = serde_json::from_slice(&bounded(f)?)
        .map_err(|_| anyhow::anyhow!("original notification witness invalid"))?;
    if value.format != 1
        || value.kind != "notificationConfiguration"
        || value.request_id != request
        || value.payload_sha256.len() != 64
        || !value
            .payload_sha256
            .bytes()
            .all(|b| b.is_ascii_digit() || (b'a'..=b'f').contains(&b))
    {
        bail!("original notification witness invalid");
    }
    Ok(())
}
fn safe_result(value: Value) -> Result<NotificationRequestLookup> {
    let mut result: NotificationRequestLookup = serde_json::from_value(value).map_err(|_| {
        anyhow::anyhow!("notification result unconfirmed; query the original request")
    })?;
    match &mut result {
        NotificationRequestLookup::NotFound => {}
        NotificationRequestLookup::Unknown { operation } => {
            if operation != OPERATION {
                bail!("notification result has a different operation");
            }
        }
        NotificationRequestLookup::Known { operation, result } => {
            if operation != OPERATION {
                bail!("notification result has a different operation");
            }
            let status: NotificationStatus = serde_json::from_value(result.take())
                .map_err(|_| anyhow::anyhow!("notification status result unconfirmed"))?;
            if crewd_store::notifications::number(&status.revision, true).is_err()
                || !matches!(
                    status.reason.as_deref(),
                    None | Some(
                        "notConfigured" | "credentialsNotLoaded" | "credentialsUnavailable"
                    )
                )
            {
                bail!("notification status result unconfirmed");
            }
            // Print only fixed status fields. In particular, never echo an
            // arbitrary server string from topic/result or an RPC error body.
            *result = json!({"available":status.available,"enabled":status.enabled,
                "reason":status.reason,"revision":status.revision,"environment":status.environment});
        }
    }
    Ok(result)
}
pub fn run(data: &Path, args: &NotificationArgs) -> Result<()> {
    let (request, method, params) = match &args.action {
        Action::Configure { input_file } => {
            let params = configuration(input_file.as_deref())?;
            let request = params["requestId"].as_str().unwrap().to_owned();
            save_witness(data, &request, &params)?;
            (request, OPERATION, params)
        }
        Action::Lookup { request_id } => {
            if !crewd_proto::valid_remote_uuid(request_id) {
                bail!("canonical notification request UUID required");
            }
            require_witness(data, request_id)?;
            (
                request_id.clone(),
                "notification.requestLookup",
                json!({"requestId":request_id}),
            )
        }
    };
    eprintln!("notification original request: {request}");
    let response = crate::call_notification(data, method, params)
        .map_err(|_| anyhow::anyhow!("notification result not confirmed; retain original request {request} and explicitly lookup; no retry was attempted"))?;
    let result = safe_result(response)?;
    println!(
        "{}",
        serde_json::to_string_pretty(&json!({"requestId":request,"result":result}))?
    );
    Ok(())
}

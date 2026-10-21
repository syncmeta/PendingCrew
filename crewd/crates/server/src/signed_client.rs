//! Dynamic OS code validation is bound to an already kernel-observed PID/birth.
//! Signature metadata/paths alone never establish Human authority.
use crewd_exec::registry::ProcessIdentity;
#[cfg(target_os = "macos")]
use crewd_exec::registry::{self, ParentProbe};
use crewd_store::local_clients::TrustedClient;
use std::io;
fn denied() -> io::Error {
    io::Error::new(
        io::ErrorKind::PermissionDenied,
        "trusted frontend code identity unavailable or mismatched",
    )
}
#[cfg(target_os = "macos")]
fn same(a: &ProcessIdentity, b: &ProcessIdentity) -> bool {
    a.pid == b.pid && a.pgid == b.pgid && a.start_time == b.start_time && a.boot_id == b.boot_id
}
#[cfg(target_os = "macos")]
fn current(expected: &ProcessIdentity) -> io::Result<()> {
    if matches!(registry::parent(expected.pid),ParentProbe::Found{identity,..} if same(expected,&identity))
    {
        Ok(())
    } else {
        Err(denied())
    }
}
#[cfg(target_os = "macos")]
mod mac {
    use super::*;
    use core_foundation::{
        base::{CFType, TCFType},
        boolean::CFBoolean,
        data::CFData,
        dictionary::CFDictionary,
        number::CFNumber,
        string::{CFString, CFStringRef},
    };
    use security_framework::os::macos::code_signing::{
        Flags, GuestAttributes, SecCode, SecRequirement,
    };
    use std::str::FromStr;
    unsafe extern "C" {
        static kSecCodeInfoIdentifier: CFStringRef;
        static kSecCodeInfoUnique: CFStringRef;
        static kSecCodeInfoTeamIdentifier: CFStringRef;
        static kSecCodeInfoFlags: CFStringRef;
        static kSecCodeInfoEntitlementsDict: CFStringRef;
        fn SecCodeCopySigningInformation(
            code: *const std::ffi::c_void,
            flags: u32,
            out: *mut core_foundation::dictionary::CFDictionaryRef,
        ) -> i32;
    }
    fn key(value: CFStringRef) -> CFString {
        unsafe { CFString::wrap_under_get_rule(value) }
    }
    fn code(birth: &ProcessIdentity) -> io::Result<SecCode> {
        current(birth)?;
        let mut attrs = GuestAttributes::new();
        attrs.set_pid(birth.pid);
        let code =
            SecCode::copy_guest_with_attribues(None, &attrs, Flags::NONE).map_err(|_| denied())?;
        current(birth)?;
        Ok(code)
    }
    fn validate(code: &SecCode, requirement: &str) -> io::Result<()> {
        let req = SecRequirement::from_str(requirement).map_err(|_| denied())?;
        code.check_validity(Flags::STRICT_VALIDATE | Flags::NO_NETWORK_ACCESS, &req)
            .map_err(|_| denied())
    }
    pub(super) fn verify(birth: &ProcessIdentity, trust: &TrustedClient) -> io::Result<()> {
        if ![
            "com.pendingname.pendingcrew",
            "com.pendingname.pendingcrew.native-candidate",
            "com.pendingname.pendingcrew.remote-preview",
        ]
        .contains(&trust.signing_identifier.as_str())
            || ![40, 64].contains(&trust.cd_hash.len())
            || !trust
                .cd_hash
                .bytes()
                .all(|b| b.is_ascii_hexdigit() && !b.is_ascii_uppercase())
        {
            return Err(denied());
        }
        let expected = match trust.mode.as_str() {
            "exactCode" => format!(
                "identifier \"{}\" and cdhash H\"{}\"",
                trust.signing_identifier, trust.cd_hash
            ),
            "publisher" => {
                let team = trust.team.as_deref().ok_or_else(denied)?;
                if trust.signing_identifier != "com.pendingname.pendingcrew"
                    || team.len() != 10
                    || !team.bytes().all(|b| b.is_ascii_alphanumeric())
                {
                    return Err(denied());
                }
                format!(
                    "anchor apple generic and identifier \"{}\" and certificate leaf[subject.OU] = \"{}\" and certificate 1[field.1.2.840.113635.100.6.2.6] exists and certificate leaf[field.1.2.840.113635.100.6.1.13] exists",
                    trust.signing_identifier, team
                )
            }
            _ => return Err(denied()),
        };
        if trust.requirement != expected {
            return Err(denied());
        }

        let code = code(birth)?;
        validate(&code, &trust.requirement)?;
        // Publisher admission must continue to require hardened runtime and
        // no debug entitlement; these cannot be inferred from a Team string.
        if trust.mode == "publisher" {
            let info = info(&code)?;
            if flags(&info)? & 0x10000 == 0 || debugging(&info) {
                return Err(denied());
            }
        }
        current(birth)
    }
    fn info(code: &SecCode) -> io::Result<CFDictionary<CFString, CFType>> {
        let mut out = std::ptr::null();
        let status = unsafe { SecCodeCopySigningInformation(code.as_CFTypeRef(), 2, &mut out) };
        if status != 0 || out.is_null() {
            return Err(denied());
        }
        Ok(unsafe { CFDictionary::wrap_under_create_rule(out) })
    }
    fn string(info: &CFDictionary<CFString, CFType>, key: CFStringRef) -> Option<String> {
        info.find(self::key(key))?
            .downcast::<CFString>()
            .map(|s| s.to_string())
    }
    fn flags(info: &CFDictionary<CFString, CFType>) -> io::Result<i64> {
        info.find(key(unsafe { kSecCodeInfoFlags }))
            .and_then(|v| v.downcast::<CFNumber>())
            .and_then(|n| n.to_i64())
            .ok_or_else(denied)
    }
    fn debugging(info: &CFDictionary<CFString, CFType>) -> bool {
        let Some(value) = info.find(key(unsafe { kSecCodeInfoEntitlementsDict })) else {
            return false;
        };
        // CFDictionary cannot use unchecked downcast; use type-ID before wrap.
        if value.type_of() != CFDictionary::<CFString, CFType>::type_id() {
            return true;
        }
        let dict = unsafe {
            CFDictionary::<CFString, CFType>::wrap_under_get_rule(value.as_CFTypeRef() as _)
        };
        dict.find(CFString::new("com.apple.security.get-task-allow"))
            .is_some_and(|v| v.downcast::<CFBoolean>().is_none_or(|v| v.into()))
    }
    pub(super) fn grant(
        birth: &ProcessIdentity,
        mode: &str,
        expected_hash: &str,
        expected_team: Option<&str>,
        db: &str,
        at: &str,
    ) -> io::Result<TrustedClient> {
        let code = code(birth)?;
        let info = info(&code)?;
        let identifier = string(&info, unsafe { kSecCodeInfoIdentifier }).ok_or_else(denied)?;
        if ![
            "com.pendingname.pendingcrew",
            "com.pendingname.pendingcrew.native-candidate",
            "com.pendingname.pendingcrew.remote-preview",
        ]
        .contains(&identifier.as_str())
        {
            return Err(denied());
        }
        let hash = info
            .find(key(unsafe { kSecCodeInfoUnique }))
            .and_then(|v| v.downcast::<CFData>())
            .ok_or_else(denied)?
            .bytes()
            .iter()
            .map(|b| format!("{b:02x}"))
            .collect::<String>();
        if expected_hash != hash {
            return Err(denied());
        }
        let team = string(&info, unsafe { kSecCodeInfoTeamIdentifier });
        let requirement = match mode {
            "exactCode" => format!("identifier \"{identifier}\" and cdhash H\"{hash}\""),
            "publisher" => {
                let team = team.as_deref().ok_or_else(denied)?;
                if identifier != "com.pendingname.pendingcrew"
                    || Some(team) != expected_team
                    || team.len() != 10
                    || !team.bytes().all(|b| b.is_ascii_alphanumeric())
                    || flags(&info)? & 0x10000 == 0
                    || debugging(&info)
                {
                    return Err(denied());
                }
                format!(
                    "anchor apple generic and identifier \"{identifier}\" and certificate leaf[subject.OU] = \"{team}\" and certificate 1[field.1.2.840.113635.100.6.2.6] exists and certificate leaf[field.1.2.840.113635.100.6.1.13] exists"
                )
            }
            _ => return Err(denied()),
        };
        validate(&code, &requirement)?;
        current(birth)?;
        Ok(TrustedClient {
            id: uuid::Uuid::new_v4().to_string(),
            db_id: db.into(),
            mode: mode.into(),
            signing_identifier: identifier,
            cd_hash: hash,
            team,
            requirement,
            created_at: at.into(),
        })
    }
}
pub(crate) fn verify(birth: &ProcessIdentity, trust: &TrustedClient) -> io::Result<()> {
    #[cfg(target_os = "macos")]
    {
        mac::verify(birth, trust)
    }
    #[cfg(not(target_os = "macos"))]
    {
        let _ = (birth, trust);
        Err(denied())
    }
}
pub(crate) fn grant(
    birth: &ProcessIdentity,
    mode: &str,
    expected_hash: &str,
    expected_team: Option<&str>,
    db: &str,
    at: &str,
) -> io::Result<TrustedClient> {
    #[cfg(target_os = "macos")]
    {
        mac::grant(birth, mode, expected_hash, expected_team, db, at)
    }
    #[cfg(not(target_os = "macos"))]
    {
        let _ = (birth, mode, expected_hash, expected_team, db, at);
        Err(denied())
    }
}

#[cfg(all(test, target_os = "macos"))]
mod tests {
    use super::*;
    use std::process::{Child, Command, Stdio};

    struct OwnedChild(Child);
    impl Drop for OwnedChild {
        fn drop(&mut self) {
            let _ = self.0.kill();
            let _ = self.0.wait();
        }
    }

    fn signed_child(identifier: &str) -> (tempfile::TempDir, OwnedChild, ProcessIdentity, String) {
        let root = tempfile::tempdir().unwrap();
        let executable = root.path().join("owned-signing-probe");
        std::fs::copy("/bin/sleep", &executable).unwrap();
        let signed = Command::new("/usr/bin/codesign")
            .args(["--force", "--sign", "-", "--identifier", identifier])
            .arg(&executable)
            .output()
            .unwrap();
        assert!(signed.status.success(), "private fixture signing failed");
        let metadata = Command::new("/usr/bin/codesign")
            .args([
                "--display",
                "--verbose=4",
                "--arch",
                if cfg!(target_arch = "aarch64") {
                    "arm64"
                } else {
                    "x86_64"
                },
            ])
            .arg(&executable)
            .output()
            .unwrap();
        assert!(metadata.status.success());
        let metadata = String::from_utf8(metadata.stderr).unwrap();
        let hash = metadata
            .lines()
            .find_map(|line| line.strip_prefix("CDHash="))
            .unwrap()
            .to_owned();
        let child = OwnedChild(
            Command::new(&executable)
                .arg("30")
                .stdin(Stdio::null())
                .stdout(Stdio::null())
                .stderr(Stdio::null())
                .spawn()
                .unwrap(),
        );
        let birth = match registry::parent(i32::try_from(child.0.id()).unwrap()) {
            ParentProbe::Found { identity, .. } => identity,
            other => panic!("owned fixture birth unavailable: {other:?}"),
        };
        (root, child, birth, hash)
    }

    #[test]
    fn remote_preview_exact_code_keeps_os_identity_and_closed_identifier_guards() {
        let (_root, _child, birth, hash) =
            signed_child("com.pendingname.pendingcrew.remote-preview");
        let trust = grant(&birth, "exactCode", &hash, None, "synthetic-db", "now")
            .expect("the single remote-preview signing ID must be admitted");
        verify(&birth, &trust).unwrap();
        assert!(
            grant(
                &birth,
                "exactCode",
                &"0".repeat(hash.len()),
                None,
                "synthetic-db",
                "now"
            )
            .is_err()
        );
        let mut changed = trust.clone();
        changed.cd_hash = "0".repeat(hash.len());
        assert!(verify(&birth, &changed).is_err());
        let mut replaced_birth = birth.clone();
        replaced_birth.start_time += 1;
        assert!(verify(&replaced_birth, &trust).is_err());
        // Adding an exact-code preview ID does not grant publisher authority to
        // an ad-hoc binary, a made-up Team, or a merely similar identifier.
        assert!(
            grant(
                &birth,
                "publisher",
                &hash,
                Some("M42BKJN82S"),
                "synthetic-db",
                "now"
            )
            .is_err()
        );
        let (_other_root, _other_child, other_birth, other_hash) =
            signed_child("com.pendingname.pendingcrew.remote-preview.other");
        assert!(
            grant(
                &other_birth,
                "exactCode",
                &other_hash,
                None,
                "synthetic-db",
                "now"
            )
            .is_err()
        );
    }
}

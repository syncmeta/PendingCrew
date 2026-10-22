//! One-shot private native input representation; never accepts client filesystem paths.
use crewd_proto::codex_inputs::*;
use serde_json::{Value, json};
use std::{
    io::Write,
    os::unix::fs::{DirBuilderExt, MetadataExt, OpenOptionsExt, PermissionsExt},
    path::{Path, PathBuf},
    sync::Arc,
};
#[derive(Clone)]
pub struct Payload {
    pub kind: CodexInputKind,
    pub filename: String,
    pub mime_type: String,
    pub sha256: String,
    pub body: Vec<u8>,
}
impl std::fmt::Debug for Payload {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str("Payload([private native input])")
    }
}
#[derive(Debug, Clone, Default)]
pub struct Submission {
    pub payloads: Vec<Payload>,
    pub skills: Vec<CodexSkillSelection>,
}
struct Files(PathBuf);
impl Drop for Files {
    fn drop(&mut self) {
        let _ = std::fs::remove_dir_all(&self.0);
    }
}
#[derive(Clone, Default)]
pub struct Prepared {
    items: Vec<Value>,
    _files: Option<Arc<Files>>,
}
impl std::fmt::Debug for Prepared {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str("Prepared([private native input])")
    }
}
impl PartialEq for Prepared {
    fn eq(&self, other: &Self) -> bool {
        self.items == other.items
    }
}
impl Eq for Prepared {}
impl Prepared {
    pub fn append(&self, input: &mut Value) {
        if let Some(items) = input.as_array_mut() {
            items.extend(self.items.iter().cloned());
        }
    }
}
pub fn qualify_skills(
    selected: &[CodexSkillSelection],
    catalog: &Value,
    cwd: &Path,
) -> Result<(), String> {
    if selected.is_empty() {
        return Ok(());
    }
    let catalog = catalog.get("providerResult").unwrap_or(catalog);
    let rows = catalog
        .pointer("/skills/data")
        .and_then(Value::as_array)
        .ok_or("native skills catalog unavailable")?;
    // skills/list groups exact canonical paths by cwd; only explicit enabled records qualify.
    if rows
        .iter()
        .any(|r| r.get("cwd").and_then(Value::as_str) != cwd.to_str())
    {
        return Err("native skill catalog workspace changed".into());
    }
    let entries: Vec<&Value> = rows
        .iter()
        .flat_map(|row| {
            row.get("skills")
                .and_then(Value::as_array)
                .into_iter()
                .flatten()
        })
        .collect();
    for skill in selected {
        let matches: Vec<_> = entries
            .iter()
            .filter(|entry| {
                entry.get("name").and_then(Value::as_str) == Some(skill.name.as_str())
                    && entry.get("path").and_then(Value::as_str) == Some(skill.path.as_str())
            })
            .collect();
        if matches.len() != 1 || matches[0].get("enabled").and_then(Value::as_bool) != Some(true) {
            return Err(
                "selected native skill is not uniquely enabled in current workspace".into(),
            );
        }
    }
    Ok(())
}
impl Submission {
    pub fn prepare(self, runtime: &Path) -> Result<Prepared, String> {
        if self.payloads.len() + self.skills.len() == 0
            || self.payloads.len() + self.skills.len() > MAX_SELECTION_REFS
        {
            return Err("invalid native input count".into());
        }
        let mut items = Vec::new();
        let mut guard = None;
        if !self.payloads.is_empty() {
            std::fs::DirBuilder::new()
                .recursive(true)
                .mode(0o700)
                .create(runtime)
                .map_err(|_| "native input runtime unavailable")?;
            let meta = std::fs::symlink_metadata(runtime)
                .map_err(|_| "native input runtime unavailable")?;
            if !meta.is_dir()
                || meta.file_type().is_symlink()
                || meta.uid() != unsafe { libc::geteuid() }
                || meta.permissions().mode() & 0o077 != 0
            {
                return Err("native input runtime is not private owned directory".into());
            }
            let root = runtime
                .canonicalize()
                .map_err(|_| "native input runtime unavailable")?;
            let dir = root.join(format!("native-input-{}", uuid::Uuid::new_v4()));
            std::fs::DirBuilder::new()
                .mode(0o700)
                .create(&dir)
                .map_err(|_| "native input directory unavailable")?;
            guard = Some(Arc::new(Files(dir)));
        }
        for (index, p) in self.payloads.into_iter().enumerate() {
            let sha = p.sha256.clone();
            if p.body.is_empty() || p.body.len() > MAX_MEDIA_BYTES || sha.len() != 64 {
                return Err("native input bytes changed".into());
            }
            match p.kind {
                CodexInputKind::Text => {
                    if p.body.len() > MAX_TEXT_FILE_BYTES {
                        return Err("native text file exceeds bound".into());
                    }
                    let text =
                        String::from_utf8(p.body).map_err(|_| "native input text must be UTF8")?;
                    items.push(json!({"type":"text","text":format!("Attached text file: {} (sha256 {})\n{}",p.filename,sha,text)}));
                }
                kind => {
                    let extension = match p.mime_type.as_str() {
                        "image/png" => "png",
                        "image/jpeg" => "jpg",
                        "image/gif" => "gif",
                        "image/webp" => "webp",
                        "audio/wav" => "wav",
                        "audio/mpeg" => "mp3",
                        "audio/mp4" => "m4a",
                        _ => return Err("unsupported native input media".into()),
                    };
                    let path = guard
                        .as_ref()
                        .unwrap()
                        .0
                        .join(format!("{index}.{extension}"));
                    let mut file = std::fs::OpenOptions::new()
                        .write(true)
                        .create_new(true)
                        .mode(0o600)
                        .open(&path)
                        .map_err(|_| "native input file unavailable")?;
                    file.write_all(&p.body)
                        .and_then(|_| file.sync_all())
                        .map_err(|_| "native input file unavailable")?;
                    items.push(json!({"type":if kind==CodexInputKind::Image{"localImage"}else{"localAudio"},"path":path}));
                }
            }
        }
        for skill in self.skills {
            items.push(json!({"type":"skill","name":skill.name,"path":skill.path}));
        }
        Ok(Prepared {
            items,
            _files: guard,
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn completion_private_media_files_are_owned_immutable_and_removed_with_last_runtime_guard() {
        let root = tempfile::tempdir().unwrap();
        std::fs::set_permissions(root.path(), std::fs::Permissions::from_mode(0o700)).unwrap();
        let submission = Submission {
            payloads: vec![Payload {
                kind: CodexInputKind::Image,
                filename: "display.png".into(),
                mime_type: "image/png".into(),
                sha256: "b".repeat(64),
                body: b"\x89PNG\r\n\x1a\nsynthetic private media".to_vec(),
            }],
            skills: vec![],
        };
        let prepared = submission.prepare(root.path()).unwrap();
        let path = PathBuf::from(prepared.items[0]["path"].as_str().unwrap());
        assert_eq!(
            std::fs::read(&path).unwrap(),
            b"\x89PNG\r\n\x1a\nsynthetic private media"
        );
        assert_eq!(
            std::fs::metadata(&path).unwrap().permissions().mode() & 0o777,
            0o600
        );
        let duplicate = prepared.clone();
        drop(prepared);
        assert!(path.exists());
        drop(duplicate);
        assert!(!path.exists());
        let link = root.path().join("symlink-root");
        std::os::unix::fs::symlink(root.path(), &link).unwrap();
        assert!(
            Submission {
                payloads: vec![Payload {
                    kind: CodexInputKind::Image,
                    filename: "a.png".into(),
                    mime_type: "image/png".into(),
                    sha256: "b".repeat(64),
                    body: vec![1]
                }],
                skills: vec![]
            }
            .prepare(&link)
            .is_err()
        );
    }
    #[test]
    fn completion_image_and_audio_use_native_paths_and_keep_original_bytes() {
        let root = tempfile::tempdir().unwrap();
        std::fs::set_permissions(root.path(), std::fs::Permissions::from_mode(0o700)).unwrap();
        let media = [
            (
                CodexInputKind::Image,
                "a.png",
                "image/png",
                b"\x89PNG\r\n\x1a\nsynthetic".as_slice(),
            ),
            (
                CodexInputKind::Audio,
                "a.wav",
                "audio/wav",
                b"RIFF0000WAVEsynthetic".as_slice(),
            ),
        ];
        let prepared = Submission {
            payloads: media
                .iter()
                .map(|(kind, name, mime, body)| Payload {
                    kind: *kind,
                    filename: (*name).into(),
                    mime_type: (*mime).into(),
                    sha256: "b".repeat(64),
                    body: body.to_vec(),
                })
                .collect(),
            skills: vec![],
        }
        .prepare(root.path())
        .unwrap();
        let mut paths = vec![];
        for (i, (_, _, _, body)) in media.iter().enumerate() {
            assert_eq!(
                prepared.items[i]["type"],
                if i == 0 { "localImage" } else { "localAudio" }
            );
            let path = PathBuf::from(prepared.items[i]["path"].as_str().unwrap());
            assert_eq!(std::fs::read(&path).unwrap(), *body);
            assert!(path.starts_with(root.path().canonicalize().unwrap()));
            paths.push(path);
        }
        drop(prepared);
        assert!(paths.iter().all(|p| !p.exists()));
    }
    #[test]
    fn completion_skills_require_exact_current_workspace_enabled_catalog_record() {
        let skill = CodexSkillSelection {
            name: "synthetic".into(),
            path: "/repo/skill/SKILL.md".into(),
        };
        for (cwd, enabled, expect) in [
            ("/repo", true, true),
            ("/repo", false, false),
            ("/other", true, false),
        ] {
            let catalog = json!({"skills":{"data":[{"cwd":cwd,"errors":[],"skills":[{"name":skill.name,"path":skill.path,"enabled":enabled}]}]}});
            assert_eq!(
                qualify_skills(std::slice::from_ref(&skill), &catalog, Path::new("/repo")).is_ok(),
                expect
            );
        }
        assert!(
            qualify_skills(&[skill], &json!({"skills":{"data":[]}}), Path::new("/repo")).is_err()
        );
    }
}

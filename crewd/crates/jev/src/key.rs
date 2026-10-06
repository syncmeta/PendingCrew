//! Jev key 放哪（spec §7.4、§13 Q7）：macOS 钥匙串 service `pendingcrew-jev` account `default`；
//! 其他平台放 crewd 配置目录下的 `jev-key`（0600，权限更宽就拒读）。key 只由 crewd 读，客户端只写。
//! 文件后端只信「本用户自己的、普通文件、不是符号链接、0600」；缺的父目录按 0700 建。
//! 只支持 unix（用 `std::os::unix` 的权限与属主；crate 在非 unix 上直接编译失败，见 lib.rs）。
use crate::client::ApiKey;
use std::fmt;
use std::io::{self, Read, Write};
use std::os::unix::fs::{DirBuilderExt, MetadataExt, OpenOptionsExt, PermissionsExt};
use std::path::{Path, PathBuf};

pub const KEYCHAIN_SERVICE: &str = "pendingcrew-jev";
pub const KEYCHAIN_ACCOUNT: &str = "default";
pub const KEY_FILE_NAME: &str = "jev-key";

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum KeyStore {
    Keychain { service: String, account: String },
    File(PathBuf),
}

#[derive(Debug)]
pub enum KeyError {
    /// 文件对组 / 其他人可读写：可能已经泄漏，拒绝使用，让人处理。
    InsecurePermissions {
        path: PathBuf,
        mode: u32,
    },
    /// 不是本用户自己的普通文件（符号链接、属主是别人、目录……）：不读，让人处理。
    Untrusted {
        path: PathBuf,
        reason: &'static str,
    },
    /// 存的内容不是合法 key（空、带空白）。
    Invalid,
    Io(io::Error),
    Keychain {
        code: i32,
        message: String,
    },
    /// 这个平台没有钥匙串。
    Unsupported,
}

impl fmt::Display for KeyError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            KeyError::InsecurePermissions { path, mode } => {
                write!(f, "{} 的权限是 {mode:o}，必须是 600", path.display())
            }
            KeyError::Untrusted { path, reason } => {
                write!(f, "{} 不可信：{reason}", path.display())
            }
            KeyError::Invalid => write!(f, "存的 key 不合法（空或含空白）"),
            KeyError::Io(e) => write!(f, "读写 key 文件失败：{e}"),
            KeyError::Keychain { code, message } => write!(f, "钥匙串错误 {code}：{message}"),
            KeyError::Unsupported => write!(f, "这个平台没有钥匙串"),
        }
    }
}

impl From<io::Error> for KeyError {
    fn from(e: io::Error) -> Self {
        KeyError::Io(e)
    }
}

impl KeyStore {
    /// macOS → 钥匙串；其他 → `<config_dir>/jev-key`。
    pub fn platform_default(config_dir: &Path) -> KeyStore {
        if cfg!(target_os = "macos") {
            KeyStore::Keychain {
                service: KEYCHAIN_SERVICE.into(),
                account: KEYCHAIN_ACCOUNT.into(),
            }
        } else {
            KeyStore::File(config_dir.join(KEY_FILE_NAME))
        }
    }

    /// 没存过 → Ok(None)（= 未配置，判断全交机长）。
    pub fn load(&self) -> Result<Option<ApiKey>, KeyError> {
        let raw = match self {
            KeyStore::Keychain { service, account } => keychain::load(service, account)?,
            KeyStore::File(path) => file_load(path)?,
        };
        match raw {
            None => Ok(None),
            Some(bytes) => {
                let text = String::from_utf8(bytes).map_err(|_| KeyError::Invalid)?;
                ApiKey::new(&text).map(Some).ok_or(KeyError::Invalid)
            }
        }
    }

    pub fn store(&self, key: &ApiKey) -> Result<(), KeyError> {
        match self {
            KeyStore::Keychain { service, account } => {
                keychain::store(service, account, key.expose().as_bytes())
            }
            KeyStore::File(path) => file_store(path, key.expose()),
        }
    }

    /// 删掉；本来就没有也算成功。
    pub fn clear(&self) -> Result<(), KeyError> {
        match self {
            KeyStore::Keychain { service, account } => keychain::clear(service, account),
            KeyStore::File(path) => match std::fs::remove_file(path) {
                Ok(()) => Ok(()),
                Err(e) if e.kind() == io::ErrorKind::NotFound => Ok(()),
                Err(e) => Err(e.into()),
            },
        }
    }
}

fn file_load(path: &Path) -> Result<Option<Vec<u8>>, KeyError> {
    // 先看链接本身（不跟随）：符号链接一律不读——它可能指向别人能改的地方。
    match std::fs::symlink_metadata(path) {
        Ok(m) if m.file_type().is_symlink() => {
            return Err(KeyError::Untrusted {
                path: path.to_path_buf(),
                reason: "是符号链接",
            });
        }
        Ok(_) => {}
        Err(e) if e.kind() == io::ErrorKind::NotFound => return Ok(None),
        Err(e) => return Err(e.into()),
    }
    // O_NOFOLLOW 打开，再对**打开的那个文件**做检查：检查与读取之间被换成链接也读不到别处。
    let mut file = std::fs::OpenOptions::new()
        .read(true)
        .custom_flags(libc::O_NOFOLLOW)
        .open(path)?;
    let meta = file.metadata()?;
    vet(
        path,
        meta.permissions().mode(),
        meta.uid(),
        meta.file_type().is_file(),
        current_uid(),
    )?;
    let mut bytes = Vec::new();
    file.read_to_end(&mut bytes)?;
    Ok(Some(bytes))
}

/// 本进程的有效 uid。
fn current_uid() -> u32 {
    // SAFETY: geteuid 没有前置条件、不会失败、不碰内存。
    unsafe { libc::geteuid() }
}

/// key 文件的信任检查（纯函数，便于测试）：普通文件、属主是自己、权限不宽于 0600。
fn vet(path: &Path, mode: u32, owner: u32, is_file: bool, me: u32) -> Result<(), KeyError> {
    if !is_file {
        return Err(KeyError::Untrusted {
            path: path.to_path_buf(),
            reason: "不是普通文件",
        });
    }
    if owner != me {
        return Err(KeyError::Untrusted {
            path: path.to_path_buf(),
            reason: "属主不是当前用户",
        });
    }
    let mode = mode & 0o777;
    if mode & 0o077 != 0 {
        return Err(KeyError::InsecurePermissions {
            path: path.to_path_buf(),
            mode,
        });
    }
    Ok(())
}

/// 写到同目录的临时文件（0600、create_new）再改名：任何时刻都没有一个半截或权限宽的 key 文件。
fn file_store(path: &Path, key: &str) -> Result<(), KeyError> {
    let dir = path.parent().ok_or(KeyError::Invalid)?;
    // 缺的父目录按 0700 建（已存在的不动）。
    std::fs::DirBuilder::new()
        .recursive(true)
        .mode(0o700)
        .create(dir)?;
    let tmp = path.with_extension(format!("tmp-{}", std::process::id()));
    let _ = std::fs::remove_file(&tmp);
    let result = write_private(&tmp, key).and_then(|()| std::fs::rename(&tmp, path));
    if result.is_err() {
        let _ = std::fs::remove_file(&tmp);
    }
    result.map_err(KeyError::from)
}

fn write_private(path: &Path, key: &str) -> io::Result<()> {
    let mut f = std::fs::OpenOptions::new()
        .write(true)
        .create_new(true)
        .mode(0o600)
        .open(path)?;
    f.write_all(key.as_bytes())?;
    f.sync_all()
}

#[cfg(target_os = "macos")]
mod keychain {
    use super::KeyError;
    use security_framework::passwords::{
        PasswordOptions, delete_generic_password, generic_password, set_generic_password,
    };

    /// errSecItemNotFound
    const ITEM_NOT_FOUND: i32 = -25300;

    fn err(e: security_framework::base::Error) -> KeyError {
        KeyError::Keychain {
            code: e.code(),
            message: e.message().unwrap_or_default(),
        }
    }

    pub fn load(service: &str, account: &str) -> Result<Option<Vec<u8>>, KeyError> {
        match generic_password(PasswordOptions::new_generic_password(service, account)) {
            Ok(bytes) => Ok(Some(bytes)),
            Err(e) if e.code() == ITEM_NOT_FOUND => Ok(None),
            Err(e) => Err(err(e)),
        }
    }

    pub fn store(service: &str, account: &str, secret: &[u8]) -> Result<(), KeyError> {
        set_generic_password(service, account, secret).map_err(err)
    }

    pub fn clear(service: &str, account: &str) -> Result<(), KeyError> {
        match delete_generic_password(service, account) {
            Ok(()) => Ok(()),
            Err(e) if e.code() == ITEM_NOT_FOUND => Ok(()),
            Err(e) => Err(err(e)),
        }
    }
}

#[cfg(not(target_os = "macos"))]
mod keychain {
    use super::KeyError;

    pub fn load(_: &str, _: &str) -> Result<Option<Vec<u8>>, KeyError> {
        Err(KeyError::Unsupported)
    }

    pub fn store(_: &str, _: &str, _: &[u8]) -> Result<(), KeyError> {
        Err(KeyError::Unsupported)
    }

    pub fn clear(_: &str, _: &str) -> Result<(), KeyError> {
        Err(KeyError::Unsupported)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn file_store_roundtrips_with_0600_and_clear_is_idempotent() {
        let dir = tempfile::tempdir().unwrap();
        let store = KeyStore::File(dir.path().join("cfg").join(KEY_FILE_NAME));
        assert!(store.load().unwrap().is_none(), "没存过 = 未配置");
        store.store(&ApiKey::new("test-key-1").unwrap()).unwrap();
        let KeyStore::File(path) = &store else {
            unreachable!()
        };
        let mode = std::fs::metadata(path).unwrap().permissions().mode() & 0o777;
        assert_eq!(mode, 0o600);
        assert_eq!(store.load().unwrap().unwrap().expose(), "test-key-1");
        store.store(&ApiKey::new("test-key-2").unwrap()).unwrap();
        assert_eq!(store.load().unwrap().unwrap().expose(), "test-key-2");
        let leftovers: Vec<_> = std::fs::read_dir(dir.path().join("cfg")).unwrap().collect();
        assert_eq!(leftovers.len(), 1, "没有残留的临时文件");
        store.clear().unwrap();
        store.clear().unwrap();
        assert!(store.load().unwrap().is_none());
    }

    #[test]
    fn loose_permissions_are_refused_not_used() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join(KEY_FILE_NAME);
        std::fs::write(&path, "test-key").unwrap();
        std::fs::set_permissions(&path, std::fs::Permissions::from_mode(0o644)).unwrap();
        match KeyStore::File(path).load() {
            Err(KeyError::InsecurePermissions { mode, .. }) => assert_eq!(mode, 0o644),
            other => panic!("{other:?}"),
        }
    }

    #[test]
    fn a_symlinked_key_file_is_refused() {
        let dir = tempfile::tempdir().unwrap();
        let real = dir.path().join("elsewhere");
        std::fs::write(&real, "test-key").unwrap();
        std::fs::set_permissions(&real, std::fs::Permissions::from_mode(0o600)).unwrap();
        let link = dir.path().join(KEY_FILE_NAME);
        std::os::unix::fs::symlink(&real, &link).unwrap();
        match KeyStore::File(link).load() {
            Err(KeyError::Untrusted { .. }) => {}
            other => panic!("符号链接不该被当成 key 文件读：{other:?}"),
        }
    }

    #[test]
    fn owner_type_and_mode_are_all_vetted() {
        let p = Path::new("/cfg/jev-key");
        assert!(vet(p, 0o600, 501, true, 501).is_ok());
        assert!(matches!(
            vet(p, 0o600, 0, true, 501),
            Err(KeyError::Untrusted { .. })
        ));
        assert!(matches!(
            vet(p, 0o600, 501, false, 501),
            Err(KeyError::Untrusted { .. })
        ));
        assert!(matches!(
            vet(p, 0o640, 501, true, 501),
            Err(KeyError::InsecurePermissions { mode: 0o640, .. })
        ));
    }

    #[test]
    fn missing_parent_directories_are_created_private() {
        let dir = tempfile::tempdir().unwrap();
        let cfg = dir.path().join("cfg");
        let store = KeyStore::File(cfg.join("sub").join(KEY_FILE_NAME));
        store.store(&ApiKey::new("test-key-3").unwrap()).unwrap();
        for d in [&cfg, &cfg.join("sub")] {
            let mode = std::fs::metadata(d).unwrap().permissions().mode() & 0o777;
            assert_eq!(mode, 0o700, "{}", d.display());
        }
        assert_eq!(store.load().unwrap().unwrap().expose(), "test-key-3");
    }

    #[test]
    fn garbage_in_the_file_is_invalid_not_a_key() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join(KEY_FILE_NAME);
        std::fs::write(&path, "two words").unwrap();
        std::fs::set_permissions(&path, std::fs::Permissions::from_mode(0o600)).unwrap();
        assert!(matches!(
            KeyStore::File(path).load(),
            Err(KeyError::Invalid)
        ));
    }

    #[test]
    fn platform_default_matches_the_spec() {
        let store = KeyStore::platform_default(Path::new("/cfg"));
        if cfg!(target_os = "macos") {
            assert_eq!(
                store,
                KeyStore::Keychain {
                    service: "pendingcrew-jev".into(),
                    account: "default".into()
                }
            );
        } else {
            assert_eq!(store, KeyStore::File(PathBuf::from("/cfg/jev-key")));
        }
    }

    /// 真钥匙串：写 / 读 / 删一个测试专用条目。默认不跑（check.sh 不带 --ignored）。
    #[test]
    #[ignore = "碰真钥匙串：CREWD_KEYCHAIN_TEST=1 cargo test -p crewd-jev keychain -- --ignored"]
    fn keychain_roundtrip() {
        if std::env::var("CREWD_KEYCHAIN_TEST").as_deref() != Ok("1") || !cfg!(target_os = "macos")
        {
            return;
        }
        let store = KeyStore::Keychain {
            service: "pendingcrew-jev-test".into(),
            account: "crewd-test".into(),
        };
        store.clear().unwrap();
        assert!(store.load().unwrap().is_none());
        store
            .store(&ApiKey::new("test-key-not-real").unwrap())
            .unwrap();
        assert_eq!(store.load().unwrap().unwrap().expose(), "test-key-not-real");
        store.clear().unwrap();
        assert!(store.load().unwrap().is_none());
    }
}

//! Bundled managed-task Mod source. Installing source does not qualify a provider.
use std::{
    io::{self, Write},
    os::unix::fs::{DirBuilderExt, OpenOptionsExt},
    path::{Path, PathBuf},
};
pub const PLUGIN_NAME: &str = "pendingcrew-managed";
pub const API_SOURCE: &str = include_str!("managed_plugin.js");
/// Only a freshly owned run directory is accepted; no installed plugin/config is changed.
pub fn install(parent: &Path) -> io::Result<PathBuf> {
    let root = parent.join("managed-plugin");
    std::fs::DirBuilder::new().mode(0o700).create(&root)?;
    for directory in [".claude-plugin", "hooks"] {
        std::fs::DirBuilder::new()
            .mode(0o700)
            .create(root.join(directory))?;
    }
    for (name, body) in [
        (
            ".claude-plugin/plugin.json",
            r#"{"name":"pendingcrew-managed","version":"1.0.0","description":"Core-owned managed task dispatch and native observations"}"#,
        ),
        ("hooks/hooks.json", r#"{"modules":["./register.js"]}"#),
        ("hooks/register.js", API_SOURCE),
    ] {
        let mut file = std::fs::OpenOptions::new()
            .write(true)
            .create_new(true)
            .mode(0o600)
            .open(root.join(name))?;
        file.write_all(body.as_bytes())?;
        file.sync_all()?;
    }
    Ok(root)
}

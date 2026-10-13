//! Public MCP surface snapshot and stable, non-security fingerprint.
use crate::agent::text::Locale;
use crate::agent::{Identity, Role, catalog, text};
use serde_json::{Value, json};

pub fn snapshot(loc: Locale) -> Value {
    let identity = |role| Identity {
        session_id: String::new(),
        crew_id: String::new(),
        label: String::new(),
        role,
    };
    json!({
        "instructions": text::get(loc, "instructions"),
        "worker": catalog::tools_for(&identity(Role::Worker), loc),
        "captain": catalog::tools_for(&identity(Role::Captain), loc),
    })
}

/// FNV-1a/64 on deterministic JSON (BTreeMap-backed objects); for change identification only,
/// never authentication. Covers roles, names, schemas, annotations and locale's md descriptions.
pub fn hash(loc: Locale) -> String {
    fingerprint(&snapshot(loc))
}

fn fingerprint(value: &Value) -> String {
    let bytes = serde_json::to_vec(value).expect("surface JSON serialises");
    let n = bytes.into_iter().fold(0xcbf29ce484222325_u64, |n, b| {
        (n ^ u64::from(b)).wrapping_mul(0x100000001b3)
    });
    format!("{n:016x}")
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn surface_golden_snapshots() {
        for (loc, file) in [
            (Locale::Zh, "surface.zh.json"),
            (Locale::En, "surface.en.json"),
        ] {
            let path = std::path::Path::new(env!("CARGO_MANIFEST_DIR"))
                .join("src/agent")
                .join(file);
            let value = snapshot(loc);
            if std::env::var("CREWD_BLESS").as_deref() == Ok("1") {
                std::fs::write(
                    &path,
                    format!("{}\n", serde_json::to_string_pretty(&value).unwrap()),
                )
                .unwrap();
            }
            let golden: Value = serde_json::from_slice(
                &std::fs::read(&path)
                    .expect("missing surface golden; CREWD_BLESS=1 to create intentionally"),
            )
            .unwrap();
            assert_eq!(
                value, golden,
                "{file}: inspect the change before CREWD_BLESS=1"
            );
            assert_eq!(
                hash(loc),
                fingerprint(&golden),
                "hash must cover the golden, not a constant"
            );
            assert_eq!(value["worker"].as_array().unwrap().len(), 28);
            assert_eq!(value["captain"].as_array().unwrap().len(), 39);
            let instructions = value["instructions"].as_str().unwrap();
            assert!(!instructions.is_empty() && instructions.lines().count() <= 3);
        }
    }
    #[test]
    fn fingerprint_covers_every_public_surface_component() {
        let original = json!({"instructions": "i", "worker": [{"name": "n", "inputSchema": {"type": "object"}, "annotations": {"readOnlyHint": false}, "description": "d"}]});
        let hash = fingerprint(&original);
        assert_eq!(hash, fingerprint(&original));
        for pointer in [
            "/instructions",
            "/worker/0/name",
            "/worker/0/inputSchema/type",
            "/worker/0/annotations/readOnlyHint",
            "/worker/0/description",
        ] {
            let mut changed = original.clone();
            *changed.pointer_mut(pointer).unwrap() = json!("changed");
            assert_ne!(hash, fingerprint(&changed), "{pointer}");
        }
        assert_eq!(hash.len(), 16);
    }
}

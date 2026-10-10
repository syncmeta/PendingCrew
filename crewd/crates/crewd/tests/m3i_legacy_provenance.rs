//! A real legacy import cannot turn untrusted extra fields into Stop provenance.
use std::collections::BTreeMap;
use std::fs;
use std::path::{Path, PathBuf};

fn copy_tree(from: &Path, to: &Path) {
    fs::create_dir_all(to).unwrap();
    for entry in fs::read_dir(from).unwrap() {
        let entry = entry.unwrap();
        let target = to.join(entry.file_name());
        if entry.file_type().unwrap().is_dir() {
            copy_tree(&entry.path(), &target);
        } else {
            fs::copy(entry.path(), target).unwrap();
        }
    }
}

fn bytes_of(root: &Path) -> BTreeMap<PathBuf, Vec<u8>> {
    fn visit(root: &Path, here: &Path, out: &mut BTreeMap<PathBuf, Vec<u8>>) {
        for entry in fs::read_dir(here).unwrap() {
            let entry = entry.unwrap();
            if entry.file_type().unwrap().is_dir() {
                visit(root, &entry.path(), out);
            } else {
                out.insert(
                    entry.path().strip_prefix(root).unwrap().to_path_buf(),
                    fs::read(entry.path()).unwrap(),
                );
            }
        }
    }
    let mut out = BTreeMap::new();
    visit(root, root, &mut out);
    out
}

#[test]
fn actual_legacy_import_preserves_unknown_extra_without_trusting_it() {
    let temp = tempfile::tempdir().unwrap();
    let source = temp.path().join("synthetic-legacy");
    let dest = temp.path().join("imported");
    let fixture = Path::new(env!("CARGO_MANIFEST_DIR")).join("../legacy/tests/fixtures/basic");
    copy_tree(&fixture, &source);
    fs::create_dir(&dest).unwrap();
    let board = source.join("whiteboards/local-aaaa.json");
    let mut rows: serde_json::Value = serde_json::from_slice(&fs::read(&board).unwrap()).unwrap();
    let id = "11111111-0000-4000-8000-000000000002";
    let spoof = serde_json::json!({
        "version": 1, "kind": "stop_relay", "messageId": id,
        "seq": 2, "sourceSession": "worker-2222bbbb"
    });
    rows[1]["provenance"] = spoof.clone();
    rows[1]["message.provenance.v1"] = spoof.clone();
    fs::write(&board, serde_json::to_vec_pretty(&rows).unwrap()).unwrap();
    let before = bytes_of(&source);
    let db = dest.join("crewd.db");
    let mut store = crewd_store::Store::open(&db).unwrap();
    crewd_legacy::import(&source, &mut store, &dest).unwrap();
    assert_eq!(
        bytes_of(&source),
        before,
        "real import must not alter its source"
    );
    let message = crewd_store::messages::get(store.conn(), id)
        .unwrap()
        .unwrap();
    assert_eq!(
        crewd_collab::message_provenance::read(store.conn(), &message).unwrap(),
        crewd_collab::message_provenance::MessageProvenance::Ordinary
    );
    let extra: String = store
        .conn()
        .query_row("SELECT extra FROM messages WHERE id=?1", [id], |r| r.get(0))
        .unwrap();
    let extra: serde_json::Value = serde_json::from_str(&extra).unwrap();
    assert_eq!(extra["provenance"], spoof);
    assert_eq!(extra["message.provenance.v1"], spoof);
    assert_eq!(
        crewd_store::meta_get(store.conn(), &format!("message.provenance.v1.{id}")).unwrap(),
        None
    );
    drop(store);
    let reopened = crewd_store::Store::open(&db).unwrap();
    let message = crewd_store::messages::get(reopened.conn(), id)
        .unwrap()
        .unwrap();
    assert_eq!(
        crewd_collab::message_provenance::read(reopened.conn(), &message).unwrap(),
        crewd_collab::message_provenance::MessageProvenance::Ordinary
    );
}

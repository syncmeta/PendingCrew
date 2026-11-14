//! Deterministic successor controls. Root alone executes these tests.
use super::tests::{archive_welcome_root, fresh, knowledge, no_automation, projection, state};
use super::*;
use std::os::unix::fs::{PermissionsExt, symlink};

struct Fault;
impl Fault {
    fn at(point: &'static str) -> Self {
        FAULT.with(|f| {
            assert!(f.borrow().is_none());
            *f.borrow_mut() = Some(point);
        });
        Self
    }
}
impl Drop for Fault {
    fn drop(&mut self) {
        FAULT.with(|f| *f.borrow_mut() = None);
    }
}
fn edit(s: &Store, data: &Path, text: &str) {
    let page = state(s).ready[0].pages[0].clone();
    let mut k = knowledge(s, data);
    let context = k.context().unwrap();
    let mut object = k
        .get(
            &DataRef {
                namespace: context.namespace.clone(),
                kind: "block".into(),
                id: page.block,
            },
            None,
        )
        .unwrap()
        .object;
    let before = match &mut object {
        DataObject::Block {
            revision, content, ..
        } => {
            let old = revision.clone();
            *revision = (old.parse::<u64>().unwrap() + 1).to_string();
            *content = Value::String(text.into());
            old
        }
        _ => unreachable!(),
    };
    k.apply(
        &uuid::Uuid::new_v4().to_string(),
        &context,
        "human:successor-qa",
        "full-native-edit",
        BTreeMap::new(),
        vec![Mutation::PutObject {
            object: Box::new(object),
            expected_revision: Some(before),
            captures: vec![],
        }],
        false,
        None,
    )
    .unwrap();
}
fn manifest(s: &Store, data: &Path) -> Manifest {
    let p = projection(s);
    let pointer: Value = serde_json::from_slice(
        &fs::read(data.join("welcome-crew").join(p.current.unwrap().path)).unwrap(),
    )
    .unwrap();
    serde_json::from_slice(
        &fs::read(
            data.join("welcome-crew")
                .join(pointer["manifest"].as_str().unwrap()),
        )
        .unwrap(),
    )
    .unwrap()
}
fn reopen(data: &Path, s: Store) -> Store {
    drop(s);
    Store::open_data_dir(data).unwrap()
}

#[test]
fn n1_same_generation_body_and_manifest_mutations_are_requalified_at_both_entries() {
    for startup in [false, true] {
        for target in ["body", "manifest"] {
            for delete in [false, true] {
                let (_t, data, _lock, mut s) = fresh();
                bootstrap(&mut s, &data).unwrap();
                let original = state(&s);
                let old = projection(&s);
                let file = if target == "body" {
                    manifest(&s, &data).pages[0].export_path.clone()
                } else {
                    old.manifests[0].path.clone()
                };
                let path = data.join("welcome-crew").join(&file);
                if delete {
                    fs::remove_file(&path).unwrap();
                } else {
                    fs::set_permissions(&path, fs::Permissions::from_mode(0o600)).unwrap();
                    fs::write(&path, b"user edit retained").unwrap();
                }
                if startup {
                    bootstrap(&mut s, &data).unwrap();
                } else {
                    project_committed(s.conn(), &data).unwrap();
                }
                assert!(qualified(s.conn(), &data, &projection(&s)).unwrap());
                assert_eq!(
                    serde_json::to_value(state(&s)).unwrap(),
                    serde_json::to_value(original).unwrap()
                );
                assert_eq!(
                    knowledge(&s, &data).context().unwrap().generation,
                    old.generation.unwrap()
                );
                if !delete {
                    assert_eq!(fs::read(&path).unwrap(), b"user edit retained");
                }
                let after = projection(&s);
                project_committed(s.conn(), &data).unwrap();
                assert_eq!(
                    projection(&s).current.unwrap().inode,
                    after.current.unwrap().inode,
                    "complete unchanged closure should reuse original pointer"
                );
                no_automation(&s);
            }
        }
    }
}

#[test]
fn n2_bootstrap_and_project_errors_revoke_before_external_work_and_survive_reopen() {
    for startup in [false, true] {
        for failure in [
            "directory",
            "knowledge.context",
            "knowledge-missing",
            "remove",
            "malformed-context",
            "foreign-context",
            "root-link",
            "root-file",
        ] {
            let (_t, data, _lock, mut s) = fresh();
            bootstrap(&mut s, &data).unwrap();
            let original = projection(&s);
            let root = data.join("welcome-crew");
            let mut guard = None;
            match failure {
                "directory" | "knowledge.context" | "remove" => {
                    if failure == "remove" {
                        edit(
                            &s,
                            &data,
                            "new source forces retirement of the prior current",
                        );
                    }
                    guard = Some(Fault::at(failure));
                }
                "knowledge-missing" => {
                    let db = state(&s).db_id;
                    fs::remove_file(data.join("knowledge").join(format!(
                        "{}.sqlite",
                        crewd_store::team_data::namespace(&db, facts::ROOT)
                    )))
                    .unwrap();
                }
                "malformed-context" | "foreign-context" => {
                    let k = knowledge(&s, &data);
                    let raw = if failure == "malformed-context" {
                        "not-json".into()
                    } else {
                        let mut context = k.context().unwrap();
                        context.authority_id = "foreign".into();
                        serde_json::to_string(&context).unwrap()
                    };
                    k.conn
                        .execute("UPDATE _pc_meta SET value=?1 WHERE key='context'", [raw])
                        .unwrap();
                }
                "root-link" | "root-file" => {
                    fs::rename(&root, data.join("saved-original-export")).unwrap();
                    if failure == "root-link" {
                        symlink(data.join("missing-user-target"), &root).unwrap();
                    } else {
                        fs::write(&root, b"user file").unwrap();
                    }
                }
                _ => unreachable!(),
            }
            if startup {
                let _ = bootstrap(&mut s, &data);
            } else {
                assert!(project_committed(s.conn(), &data).is_err());
            }
            drop(guard);
            let s = reopen(&data, s);
            let p = projection(&s);
            assert_ne!(p.state, "ready");
            assert!(p.current.is_none());
            assert_eq!(
                p.retired_current.as_ref().unwrap().inode,
                original.current.unwrap().inode
            );
            assert!(p.owned.len() >= original.owned.len());
            if failure == "root-file" {
                assert_eq!(fs::read(&root).unwrap(), b"user file");
            }
            no_automation(&s);
        }
    }
}

#[test]
fn n3_current_reservation_faults_never_publish_and_reopen_retains_original_receipt() {
    for point in [
        "current.unclaimed",
        "current.created",
        "current.partial",
        "current.write",
        "current.sync",
        "current.written",
        "current.commit",
    ] {
        let (_t, data, _lock, mut s) = fresh();
        bootstrap(&mut s, &data).unwrap();
        let original = state(&s).ready[0].request.clone();
        edit(&s, &data, "complete changed source, not an export edit");
        {
            let _fault = Fault::at(point);
            assert!(project_committed(s.conn(), &data).is_err(), "{point}");
        }
        let s = reopen(&data, s);
        let p = projection(&s);
        assert_eq!(p.state, "pending");
        assert!(p.current.is_none());
        let pending = p
            .reserved_current
            .as_ref()
            .expect("durable original path/inode reservation");
        if point == "current.unclaimed" {
            assert!(pending.inode.is_none());
        } else {
            assert!(pending.inode.is_some());
        }
        let bytes = fs::read(data.join("welcome-crew").join(&pending.path)).unwrap();
        if point == "current.partial" {
            assert_eq!(bytes, b"{");
        }
        if let Some(status) = &p.status {
            let status: Value = serde_json::from_slice(
                &fs::read(data.join("welcome-crew").join(&status.path)).unwrap(),
            )
            .unwrap();
            assert!(status["currentManifestPointer"].is_null());
        }
        assert_eq!(state(&s).ready[0].request, original);
        project_committed(s.conn(), &data).unwrap();
        assert!(qualified(s.conn(), &data, &projection(&s)).unwrap());
        if point == "current.partial" {
            assert_eq!(
                fs::read(data.join("welcome-crew").join(&pending.path)).unwrap(),
                b"{"
            );
        }
        no_automation(&s);
    }
}

#[test]
fn n3_actual_final_core_write_refusal_leaves_only_reservation_until_retry() {
    let (_t, data, _lock, mut s) = fresh();
    bootstrap(&mut s, &data).unwrap();
    edit(&s, &data, "original revised document");
    s.conn().execute_batch("CREATE TRIGGER hold_export_ready BEFORE UPDATE ON meta WHEN NEW.key='onboarding.export.v1' AND instr(NEW.value,'\"state\":\"ready\"')>0 BEGIN SELECT RAISE(ABORT,'controlled final publication refusal'); END;").unwrap();
    assert!(project_committed(s.conn(), &data).is_err());
    let s = reopen(&data, s);
    assert!(projection(&s).current.is_none());
    assert!(projection(&s).reserved_current.is_some());
    s.conn()
        .execute_batch("DROP TRIGGER hold_export_ready")
        .unwrap();
    project_committed(s.conn(), &data).unwrap();
    assert!(qualified(s.conn(), &data, &projection(&s)).unwrap());
}

#[test]
fn n4_active_aliases_and_hard_purge_never_follow_or_claim_cleared() {
    for kind in [
        "symlink",
        "broken-link",
        "hardlink",
        "broken-root",
        "manifest-parent-link",
    ] {
        let (_t, data, _lock, mut s) = fresh();
        bootstrap(&mut s, &data).unwrap();
        let original = projection(&s);
        let root = data.join("welcome-crew");
        let target = data.join("original-user-target");
        fs::write(&target, b"keep user bytes").unwrap();
        if kind == "broken-root" {
            fs::rename(&root, data.join("original-retained-root")).unwrap();
            symlink(data.join("absent-user-root"), &root).unwrap();
        } else if kind == "manifest-parent-link" {
            let file = root.join(&original.manifests[0].path);
            let parent = file.parent().unwrap();
            let retained = data.join("retained-original-manifest-directory");
            fs::rename(parent, &retained).unwrap();
            symlink(&retained, parent).unwrap();
            assert!(project_committed(s.conn(), &data).is_err());
            assert!(projection(&s).current.is_none());
        } else {
            let file = root.join(&manifest(&s, &data).pages[0].export_path);
            fs::remove_file(&file).unwrap();
            match kind {
                "symlink" => symlink(&target, &file).unwrap(),
                "broken-link" => symlink(data.join("missing-user-file"), &file).unwrap(),
                "hardlink" => fs::hard_link(&target, &file).unwrap(),
                _ => unreachable!(),
            }
            project_committed(s.conn(), &data).unwrap();
            assert!(qualified(s.conn(), &data, &projection(&s)).unwrap());
        }
        archive_welcome_root(&mut s, 200);
        collab::run(&mut s, 200, |tx| collab::crews::purge(tx, facts::ROOT)).unwrap();
        assert!(project_committed(s.conn(), &data).is_err());
        let s = reopen(&data, s);
        let p = projection(&s);
        assert_eq!(p.state, "cleanupUnverified");
        assert!(p.current.is_none());
        assert!(!p.owned.is_empty() || !p.manifests.is_empty());
        assert_eq!(fs::read(&target).unwrap(), b"keep user bytes");
        if kind == "manifest-parent-link" {
            assert_eq!(
                sha(
                    &fs::read(data.join("retained-original-manifest-directory/manifest.json"))
                        .unwrap()
                ),
                original.manifests[0].sha256
            );
        }
        if kind == "broken-root" {
            assert_eq!(p.owned.len(), original.owned.len());
            assert!(
                fs::symlink_metadata(&root)
                    .unwrap()
                    .file_type()
                    .is_symlink()
            );
        }
    }
}

#[test]
fn n4_unlink_io_failure_retains_all_obligations_then_true_absence_settles() {
    let (_t, data, _lock, mut s) = fresh();
    bootstrap(&mut s, &data).unwrap();
    let original = projection(&s);
    archive_welcome_root(&mut s, 200);
    collab::run(&mut s, 200, |tx| collab::crews::purge(tx, facts::ROOT)).unwrap();
    {
        let _fault = Fault::at("remove");
        assert!(project_committed(s.conn(), &data).is_err());
    }
    let s = reopen(&data, s);
    let p = projection(&s);
    assert_eq!(p.state, "cleanupUnverified");
    assert!(p.current.is_none());
    assert_eq!(p.owned.len(), original.owned.len());
    assert_eq!(p.manifests.len(), original.manifests.len());
    assert!(p.retired_current.is_some());
    project_committed(s.conn(), &data).unwrap();
    assert_eq!(projection(&s).state, "cleared");
    fs::remove_dir_all(data.join("welcome-crew")).unwrap();
    project_committed(s.conn(), &data).unwrap();
    assert_eq!(projection(&s).state, "cleared");
}

#[test]
fn n5_complete_batch_capacity_cannot_write_4097_or_discard_unknown_receipts() {
    for count in [FILE_LIMIT - 1, FILE_LIMIT] {
        let (_t, data, _lock, mut s) = fresh();
        bootstrap(&mut s, &data).unwrap();
        let mut p = projection(&s);
        while p.owned.len() < count {
            p.owned.push(record(
                &format!("unverified-{}.md", p.owned.len()),
                &sha(b"unknown"),
                7,
            ));
        }
        let user = data
            .join("welcome-crew")
            .join(&p.owned.last().unwrap().path);
        fs::write(&user, b"unverified capacity user file").unwrap();
        save(s.conn(), &p).unwrap();
        let before = crewd_store::meta_get(s.conn(), EXPORT_KEY).unwrap();
        let mut invalid = p.clone();
        invalid.owned.push(record("over-bound.md", &sha(b""), 0));
        if count == FILE_LIMIT {
            assert!(save(s.conn(), &invalid).is_err());
            assert_eq!(crewd_store::meta_get(s.conn(), EXPORT_KEY).unwrap(), before);
        }
        edit(&s, &data, "capacity cannot justify a partial export claim");
        assert!(project_committed(s.conn(), &data).is_err());
        let mut s = reopen(&data, s);
        let after = projection(&s);
        assert!(after.current.is_none());
        assert_eq!(after.owned.len(), count);
        assert!(after.retired_current.is_some());
        archive_welcome_root(&mut s, 300);
        collab::run(&mut s, 300, |tx| collab::crews::purge(tx, facts::ROOT)).unwrap();
        assert!(project_committed(s.conn(), &data).is_err());
        assert_eq!(projection(&s).state, "cleanupUnverified");
        assert_eq!(projection(&s).owned.len(), 1);
        assert_eq!(fs::read(&user).unwrap(), b"unverified capacity user file");
    }
}

#[test]
fn n5_facts_write_256k_bound_rolls_back_original_plan_instead_of_self_refusing() {
    let (_t, _data, _lock, mut s) = fresh();
    let (initial, _) = collab::run(&mut s, 100, facts::begin).unwrap();
    let before = crewd_store::meta_get(s.conn(), facts::KEY).unwrap();
    let mut context = base(&initial.db_id);
    context.restore_epoch = "r".repeat(256 * 1024);
    assert!(
        collab::run(&mut s, 100, |tx| facts::prepare(
            tx,
            EDITION,
            &assets(),
            context
        ))
        .is_err()
    );
    assert_eq!(crewd_store::meta_get(s.conn(), facts::KEY).unwrap(), before);
    assert!(facts::read(s.conn()).unwrap().unwrap().pending.is_none());
}

#[test]
fn n5_full_edition_history_refuses_a_before_any_unpublishable_b_request() {
    let (_t, data, _lock, mut s) = fresh();
    bootstrap(&mut s, &data).unwrap();
    let mut history = state(&s);
    for edition in 2..=32 {
        let pages = vec![facts::Page {
            asset: facts::Asset {
                file: format!("capacity-edition-{edition}.md"),
                sha256: sha(b""),
                bytes: 0,
            },
            block: uuid::Uuid::new_v4().to_string(),
            view: uuid::Uuid::new_v4().to_string(),
            placement: uuid::Uuid::new_v4().to_string(),
        }];
        let payload_sha256 = crewd_store::team_data::digest(&(edition, &pages)).unwrap();
        history.ready.push(facts::Publication {
            edition,
            request: uuid::Uuid::new_v4().to_string(),
            origin: base(&history.db_id),
            payload_sha256,
            created_at: "capacity-fixture".into(),
            pages,
        });
    }
    // Explicit boundary metadata fixture, not a claim these synthetic editions
    // were published to Knowledge. Exercise prepare's pre-B C-size forecast.
    let before = serde_json::to_string(&history).unwrap();
    crewd_store::meta_set(s.conn(), facts::KEY, &before).unwrap();
    facts::read(s.conn()).unwrap();
    let mut next = assets();
    next.push(facts::Asset {
        file: "new-edition-33.md".into(),
        sha256: sha(b""),
        bytes: 0,
    });
    assert!(
        collab::run(&mut s, 100, |tx| facts::prepare(
            tx,
            33,
            &next,
            base(&history.db_id)
        ))
        .is_err()
    );
    assert_eq!(
        crewd_store::meta_get(s.conn(), facts::KEY)
            .unwrap()
            .unwrap(),
        before
    );
    assert!(state(&s).pending.is_none());
}

#[test]
fn n5_manifest_4096_boundary_remains_readable_and_cleanup_can_settle_missing_history() {
    let (_t, data, _lock, mut s) = fresh();
    bootstrap(&mut s, &data).unwrap();
    let mut p = projection(&s);
    while p.manifests.len() < FILE_LIMIT - 1 {
        p.manifests.push(record(
            &format!("missing-version-{}/manifest.json", p.manifests.len()),
            &sha(b"missing"),
            7,
        ));
    }
    save(s.conn(), &p).unwrap();
    edit(&s, &data, "last permitted complete manifest");
    project_committed(s.conn(), &data).unwrap();
    assert_eq!(projection(&s).manifests.len(), FILE_LIMIT);
    assert!(qualified(s.conn(), &data, &projection(&s)).unwrap());
    edit(&s, &data, "next batch must stop before creating a manifest");
    assert!(project_committed(s.conn(), &data).is_err());
    let mut s = reopen(&data, s);
    assert!(projection(&s).current.is_none());
    assert_eq!(projection(&s).manifests.len(), FILE_LIMIT);
    archive_welcome_root(&mut s, 300);
    collab::run(&mut s, 300, |tx| collab::crews::purge(tx, facts::ROOT)).unwrap();
    project_committed(s.conn(), &data).unwrap();
    assert_eq!(projection(&s).state, "cleared");
}

#[test]
fn n6_absent_database_recovery_clues_never_receive_a_fresh_marker() {
    for clue in [
        "identity",
        "backup",
        "allocator",
        "option-floor",
        "purge-pending",
    ] {
        let t = tempfile::tempdir().unwrap();
        let data = fs::canonicalize(t.path()).unwrap();
        crate::ensure_data_dir(&data).unwrap();
        let _lock = crate::lock_data_dir(&data).unwrap();
        match clue {
            "identity" => {
                let s = Store::open_data_dir(&data).unwrap();
                drop(s);
                fs::remove_file(data.join(crewd_store::DB_FILE)).unwrap();
            }
            "backup" => {
                let dir = data.join(crewd_store::backup::BACKUPS_DIR);
                fs::create_dir(&dir).unwrap();
                fs::write(dir.join("original.db"), b"retained original evidence").unwrap();
            }
            "allocator" => {
                let file = data.join(".object-refs-restore-floor.v1");
                fs::write(&file, b"7\n").unwrap();
                fs::set_permissions(file, fs::Permissions::from_mode(0o600)).unwrap();
            }
            "option-floor" => {
                crewd_store::managed_tables_v3::persist_option_witness(&data, 7).unwrap()
            }
            "purge-pending" => {
                let dir = data.join(".crew-purge-pending-v1");
                fs::create_dir(&dir).unwrap();
                fs::set_permissions(&dir, fs::Permissions::from_mode(0o700)).unwrap();
                fs::write(dir.join("original-unresolved-operation"), b"retained").unwrap();
            }
            _ => unreachable!(),
        }
        assert!(Store::open_data_dir(&data).is_err(), "{clue}");
        assert!(!data.join(crewd_store::DB_FILE).exists(), "{clue}");
        assert!(!data.join("welcome-crew").exists());
    }
}

#[test]
fn n6_resume_rejects_alien_original_revision_capture_and_receipt_next_to_b() {
    for kind in ["capture", "revision", "extra-object", "missing-receipt"] {
        let (_t, data, _lock, mut s) = fresh();
        s.conn().execute_batch("CREATE TRIGGER hold_original_C BEFORE INSERT ON crews BEGIN SELECT RAISE(ABORT,'hold C'); END;").unwrap();
        assert!(bootstrap(&mut s, &data).is_err());
        let plan = state(&s).pending.unwrap();
        let k = knowledge(&s, &data);
        match kind {
            "capture" => {
                k.conn.execute("UPDATE _pc_revisions SET body=json_set(body,'$.captures',json('[]')) WHERE kind='view' AND id=?1", [&plan.pages[0].view]).unwrap();
            }
            "revision" => {
                k.conn
                    .execute(
                        "UPDATE _pc_revisions SET body='{}' WHERE kind='block' AND id=?1",
                        [&plan.pages[0].block],
                    )
                    .unwrap();
            }
            "extra-object" => {
                k.conn.execute("INSERT INTO _pc_objects SELECT kind,'alien-original-id',revision,archived,body FROM _pc_objects WHERE kind='block' LIMIT 1", []).unwrap();
            }
            "missing-receipt" => {
                k.conn
                    .execute("DELETE FROM _pc_receipts WHERE id=?1", [&plan.request])
                    .unwrap();
            }
            _ => unreachable!(),
        }
        drop(k);
        s.conn()
            .execute_batch("DROP TRIGGER hold_original_C")
            .unwrap();
        let mut s = reopen(&data, s);
        assert!(bootstrap(&mut s, &data).is_err());
        assert_eq!(state(&s).pending.unwrap().request, plan.request);
        assert!(crewd_store::crews::list(s.conn()).unwrap().is_empty());
        no_automation(&s);
    }
}

#[test]
fn n8_preexisting_identical_root_file_is_never_adopted_or_deleted() {
    let (_t, data, _lock, mut s) = fresh();
    let root = data.join("welcome-crew");
    secure_dir(&root).unwrap();
    let target = root.join(BUNDLED[0].0);
    fs::write(&target, BUNDLED[0].1).unwrap();
    bootstrap(&mut s, &data).unwrap();
    assert!(!projection(&s).owned.iter().any(|f| f.path == BUNDLED[0].0));
    archive_welcome_root(&mut s, 300);
    collab::run(&mut s, 300, |tx| collab::crews::purge(tx, facts::ROOT)).unwrap();
    project_committed(s.conn(), &data).unwrap();
    assert_eq!(fs::read_to_string(&target).unwrap(), BUNDLED[0].1);
}

#[test]
fn n8_maximum_normal_stored_block_and_multiblock_document_export_every_byte() {
    let (_t, data, _lock, mut s) = fresh();
    bootstrap(&mut s, &data).unwrap();
    let pages = state(&s).ready[0].pages.clone();
    let mut k = knowledge(&s, &data);
    let context = k.context().unwrap();
    let first_ref = DataRef {
        namespace: context.namespace.clone(),
        kind: "block".into(),
        id: pages[0].block.clone(),
    };
    let second_ref = DataRef {
        namespace: context.namespace.clone(),
        kind: "block".into(),
        id: pages[1].block.clone(),
    };
    let mut first = k.get(&first_ref, None).unwrap().object;
    if let DataObject::Block {
        revision, content, ..
    } = &mut first
    {
        *revision = "2".into();
        *content = json!("");
    }
    // Actual normal Knowledge::apply bounds serialized Stored at exactly 1MiB,
    // separately from SQLite's 2MiB value limit. Derive the largest ASCII body
    // from the real public Stored encoder, not an invented export-size limit.
    let overhead = serde_json::to_vec(&crewd_store::team_data::Stored {
        object: first.clone(),
        captures: vec![],
    })
    .unwrap()
    .len();
    let maximum = 1024 * 1024 - overhead;
    let complete = "x".repeat(maximum);
    let mut illegal = first.clone();
    if let DataObject::Block { content, .. } = &mut illegal {
        *content = json!("x".repeat(maximum + 1));
    }
    let refused = k
        .apply(
            &uuid::Uuid::new_v4().to_string(),
            &context,
            "human:bound-qa",
            "one-byte-over-real-limit",
            BTreeMap::new(),
            vec![Mutation::PutObject {
                object: Box::new(illegal),
                expected_revision: Some("1".into()),
                captures: vec![],
            }],
            false,
            None,
        )
        .unwrap_err();
    assert!(refused.to_string().contains("object byte bound exceeded"));
    assert_eq!(k.context().unwrap(), context);
    if let DataObject::Block { content, .. } = &mut first {
        *content = json!(complete);
    }
    let mut second = k.get(&second_ref, None).unwrap().object;
    let suffix = "complete second block\n".repeat(4096);
    if let DataObject::Block {
        revision, content, ..
    } = &mut second
    {
        *revision = "2".into();
        *content = json!(suffix);
    }
    let view_ref = DataRef {
        namespace: context.namespace.clone(),
        kind: "view".into(),
        id: pages[0].view.clone(),
    };
    let mut view = k.get(&view_ref, None).unwrap().object;
    let placement = uuid::Uuid::new_v4().to_string();
    if let DataObject::View {
        revision,
        definition: DataViewDefinition::Document { placements, .. },
        ..
    } = &mut view
    {
        *revision = "2".into();
        placements.push(DataPlacement {
            id: placement.clone(),
            block: DataRefState::Visible {
                reference: second_ref.clone(),
            },
            at_revision: None,
            layout: json!({}),
        });
    }
    let captures = vec![(&pages[0].placement, &first_ref), (&placement, &second_ref)]
        .into_iter()
        .map(|(id, r)| Capture {
            anchor: format!("placement:{id}"),
            namespace: r.namespace.clone(),
            kind: "block".into(),
            id: r.id.clone(),
            commit: "self".into(),
            revision: "2".into(),
            live: true,
        })
        .collect();
    k.apply(
        &uuid::Uuid::new_v4().to_string(),
        &context,
        "human:bound-qa",
        "full-multiblock",
        BTreeMap::new(),
        vec![
            Mutation::PutObject {
                object: Box::new(first),
                expected_revision: Some("1".into()),
                captures: vec![],
            },
            Mutation::PutObject {
                object: Box::new(second),
                expected_revision: Some("1".into()),
                captures: vec![],
            },
            Mutation::PutObject {
                object: Box::new(view),
                expected_revision: Some("1".into()),
                captures,
            },
        ],
        false,
        None,
    )
    .unwrap();
    drop(k);
    project_committed(s.conn(), &data).unwrap();
    let m = manifest(&s, &data);
    let page = &m.pages[0];
    let all = format!("{complete}{suffix}");
    assert!(all.len() > 1024 * 1024);
    assert_eq!(page.bytes, all.len() as u64);
    assert_eq!(page.sha256, sha(all.as_bytes()));
    assert_eq!(
        page.blocks,
        vec![(first_ref.id, "2".into()), (second_ref.id, "2".into())]
    );
    assert_eq!(
        fs::read(data.join("welcome-crew").join(&page.export_path)).unwrap(),
        all.as_bytes()
    );
    assert!(qualified(s.conn(), &data, &projection(&s)).unwrap());
    no_automation(&s);
}

#[test]
fn n9_original_titles_decode_to_exact_original_utf8_without_scanner_exceptions() {
    assert_eq!(
        facts::CREWS.iter().map(|(_, t, _)| *t).collect::<Vec<_>>(),
        ["PendingCrew怎么用", "演示", "问答", "子机组"]
    );
}

#[tokio::test]
async fn n7_normal_serve_knowledge_commit_confirm_restore_and_protected_purge_use_real_hub() {
    tokio::time::timeout(std::time::Duration::from_secs(45), async {
    use crate::agent::testkit::RpcClient;
    use std::time::Duration;
    let t = tempfile::tempdir().unwrap();
    let data = t.path().join("data");
    let (ready_tx, ready_rx) = tokio::sync::oneshot::channel();
    let (stop_tx, stop_rx) = tokio::sync::oneshot::channel();
    let task = tokio::spawn(crate::serve_with(
        data.clone(),
        crate::ServeOptions {
            test_ready: Some(Box::new(move |state, _| {
                let _ = ready_tx.send(state);
            })),
            ..Default::default()
        },
        async {
            let _ = stop_rx.await;
        },
    ));
    let owner = tokio::time::timeout(Duration::from_secs(10), ready_rx)
        .await
        .unwrap()
        .unwrap();
    let canonical = fs::canonicalize(&data).unwrap();
    let scope = canonical.clone();
    let page = owner
        .db
        .call_result(move |s| {
            assert!(qualified(s.conn(), &scope, &projection(s)).unwrap());
            no_automation(s);
            Ok::<_, crewd_store::StoreError>(state(s).ready[0].pages[0].clone())
        })
        .await
        .unwrap()
        .unwrap();
    let mut client = RpcClient::connect(&data.join("crewd.sock")).await;
    let workspace = client
        .call("data.workspace.get", json!({"crewId":facts::ROOT}))
        .await
        .unwrap();
    let edited = client.call("data.submit", json!({"requestId":uuid::Uuid::new_v4().to_string(), "context":workspace["context"],
        "change":{"kind":"apply","message":"Original tutorial edit","edits":[{"kind":"updateBlock","id":page.block,"expectedRevision":"1","contentType":"text/plain","content":"Real native authority update","sources":null,"attributes":{}}]}})).await.unwrap();
    assert_eq!(edited["phase"], "committed");
    let sql = client.call("data.submit", json!({"requestId":uuid::Uuid::new_v4().to_string(),"context":edited["context"],
        "change":{"kind":"apply","message":"Preview fixture","edits":[{"kind":"sql","statements":[{"sql":"CREATE TABLE onboarding_sample(id INTEGER PRIMARY KEY,value TEXT)","params":[]},{"sql":"INSERT INTO onboarding_sample VALUES(1,'original')","params":[]}]}]}})).await.unwrap();
    let preview = client.call("data.preview", json!({"context":sql["context"],"statements":[{"sql":"UPDATE onboarding_sample SET value='confirmed' WHERE id=1","params":[]}]})).await.unwrap();
    let confirmed = client.call("data.confirm", json!({"requestId":uuid::Uuid::new_v4().to_string(),"context":sql["context"],"token":preview["token"]})).await.unwrap();
    assert_eq!(confirmed["phase"], "committed");
    let scope = canonical.clone();
    let hub = owner.hub.clone();
    owner
        .db
        .call_result(move |s| {
            assert!(qualified(s.conn(), &scope, &projection(s)).unwrap());
            let before = crewd_store::meta_get(s.conn(), EXPORT_KEY)?;
            hub.emit(
                s.conn(),
                &[collab::Change::DataChanged {
                    crew_id: "unrelated-owner".into(),
                }],
            );
            assert_eq!(crewd_store::meta_get(s.conn(), EXPORT_KEY)?, before);
            no_automation(s);
            Ok::<_, crewd_store::StoreError>(())
        })
        .await
        .unwrap()
        .unwrap();
    let snapshots = client
        .call("data.snapshots", json!({"context":confirmed["context"]}))
        .await
        .unwrap();
    let snapshot = snapshots["items"].as_array().unwrap().last().unwrap()["id"].clone();
    let restored = client.call("data.restore", json!({"requestId":uuid::Uuid::new_v4().to_string(),"context":confirmed["context"],"snapshotId":snapshot})).await.unwrap();
    assert_eq!(restored["phase"], "committed");
    owner
        .db
        .call_result(|s| {
            assert_eq!(projection(s).state, "inactive");
            assert!(projection(s).current.is_none());
            Ok::<_, crewd_store::StoreError>(())
        })
        .await
        .unwrap()
        .unwrap();
    // The ordinary protected lifecycle is non-cascading. Move only the Root's
    // two direct edges through the real API; keep the shared child's two other
    // parent edges and original identity, then use remove and protected purge.
    let shared_before = owner
        .db
        .call_result(|s| {
            Ok::<_, crewd_store::StoreError>(
                crewd_store::crews::get(s.conn(), "pendingcrew-welcome-child")?.unwrap(),
            )
        })
        .await
        .unwrap()
        .unwrap();
    for &(id, _, parents) in facts::CREWS {
        if !parents.contains(&facts::ROOT) {
            continue;
        }
        let released = client
            .call(
                "crew.release",
                json!({"crewId":id,"fromParentId":facts::ROOT,"toParentId":null}),
            )
            .await
            .unwrap();
        assert_eq!(released["id"], id);
        assert!(
            released["parentIds"]
                .as_array()
                .unwrap()
                .iter()
                .all(|parent| parent.as_str().is_some_and(|id| id != facts::ROOT))
        );
    }
    owner
        .db
        .call_result(move |s| {
            let root = crewd_store::crews::get(s.conn(), facts::ROOT)?.unwrap();
            assert!(root.child_ids.is_empty());
            let shared =
                crewd_store::crews::get(s.conn(), "pendingcrew-welcome-child")?.unwrap();
            assert_eq!(shared.id, shared_before.id);
            assert_eq!(shared.number, shared_before.number);
            assert_eq!(shared.parent_ids, shared_before.parent_ids);
            assert!(shared.removed_at.is_none());
            Ok::<_, crewd_store::StoreError>(())
        })
        .await
        .unwrap()
        .unwrap();
    client
        .call("crew.remove", json!({"crewId":facts::ROOT}))
        .await
        .unwrap();
    client
        .call("crew.purge", json!({"crewId":facts::ROOT}))
        .await
        .unwrap();
    owner
        .db
        .call_result(|s| {
            assert_eq!(projection(s).state, "cleared");
            assert!(crewd_store::crews::get(s.conn(), facts::ROOT)?.is_none());
            no_automation(s);
            Ok::<_, crewd_store::StoreError>(())
        })
        .await
        .unwrap()
        .unwrap();
    drop(client);
    drop(owner);
    stop_tx.send(()).unwrap();
    tokio::time::timeout(Duration::from_secs(10), task)
        .await
        .unwrap()
        .unwrap()
        .unwrap();
    let mut s = Store::open_data_dir(&canonical).unwrap();
    bootstrap(&mut s, &canonical).unwrap();
    assert!(
        crewd_store::crews::get(s.conn(), facts::ROOT)
            .unwrap()
            .is_none()
    );
    no_automation(&s);
    }).await.expect("bounded normal serve/Knowledge/lifecycle reconciliation");
}

#[tokio::test]
async fn n7_normal_serve_existing_empty_organization_remains_skipped() {
    tokio::time::timeout(std::time::Duration::from_secs(20), async {
        let t = tempfile::tempdir().unwrap();
        let data = t.path().join("data");
        crate::ensure_data_dir(&data).unwrap();
        drop(Store::open(&data.join(crewd_store::DB_FILE)).unwrap());
        let (ready_tx, ready_rx) = tokio::sync::oneshot::channel();
        let (stop_tx, stop_rx) = tokio::sync::oneshot::channel();
        let task = tokio::spawn(crate::serve_with(
            data.clone(),
            crate::ServeOptions {
                test_ready: Some(Box::new(move |state, _| {
                    let _ = ready_tx.send(state);
                })),
                ..Default::default()
            },
            async {
                let _ = stop_rx.await;
            },
        ));
        let owner = ready_rx.await.unwrap();
        owner
            .db
            .call_result(|s| {
                assert!(state(s).skipped);
                assert!(crewd_store::crews::list(s.conn())?.is_empty());
                no_automation(s);
                Ok::<_, crewd_store::StoreError>(())
            })
            .await
            .unwrap()
            .unwrap();
        assert!(!data.join("welcome-crew").exists());
        drop(owner);
        stop_tx.send(()).unwrap();
        task.await.unwrap().unwrap();
    })
    .await
    .expect("bounded old organization normal startup");
}

struct StatusHook;
impl StatusHook {
    fn once(action: impl FnOnce(&rusqlite::Connection, &Path) + 'static) -> Self {
        STATUS_ACTION.with(|slot| {
            assert!(slot.borrow().is_none());
            *slot.borrow_mut() = Some(Box::new(action));
        });
        Self
    }
}
impl Drop for StatusHook {
    fn drop(&mut self) {
        STATUS_ACTION.with(|slot| *slot.borrow_mut() = None);
    }
}
struct SelectedUnlinkFault;
impl SelectedUnlinkFault {
    fn at(path: &str) -> Self {
        UNLINK_FAULT_PATH.with(|slot| {
            assert!(slot.borrow().is_none());
            *slot.borrow_mut() = Some(path.into());
        });
        Self
    }
}
impl Drop for SelectedUnlinkFault {
    fn drop(&mut self) {
        UNLINK_FAULT_PATH.with(|slot| *slot.borrow_mut() = None);
    }
}

#[test]
fn p1_status_only_qualification_errors_revoke_after_actual_ready_commit_at_both_entries() {
    for startup in [false, true] {
        for kind in [
            "stage-io",
            "root-link",
            "broken-root",
            "root-file",
            "root-mode",
            "body-edit",
        ] {
            let (_t, data, _lock, mut s) = fresh();
            bootstrap(&mut s, &data).unwrap();
            let original = projection(&s);
            let original_facts = serde_json::to_value(state(&s)).unwrap();
            let user = data.join("status-user-directory");
            secure_dir(&user).unwrap();
            fs::write(user.join("user-text"), b"must not touch status target").unwrap();
            let observed = std::rc::Rc::new(std::cell::Cell::new(false));
            let witness = observed.clone();
            let _hook = StatusHook::once(move |c, data| {
                let p = load(c).unwrap().unwrap();
                assert_eq!(p.state, "ready");
                assert!(p.current.is_some());
                assert!(qualified(c, data, &p).unwrap()); // inner really completed, not an early failure
                witness.set(true);
                let root = data.join("welcome-crew");
                match kind {
                    "stage-io" => (),
                    "root-link" | "broken-root" | "root-file" => {
                        fs::rename(&root, data.join("retained-before-status")).unwrap();
                        if kind == "root-file" {
                            fs::write(&root, b"user root replacement").unwrap();
                        } else {
                            let target = if kind == "root-link" {
                                data.join("status-user-directory")
                            } else {
                                data.join("absent-status-target")
                            };
                            symlink(target, &root).unwrap();
                        }
                    }
                    "root-mode" => {
                        fs::set_permissions(&root, fs::Permissions::from_mode(0o755)).unwrap()
                    }
                    "body-edit" => {
                        let body = root.join(
                            &p.owned
                                .iter()
                                .find(|f| f.path.starts_with("body-"))
                                .unwrap()
                                .path,
                        );
                        fs::set_permissions(&body, fs::Permissions::from_mode(0o600)).unwrap();
                        fs::write(body, b"status-stage user edit").unwrap();
                    }
                    _ => unreachable!(),
                }
            });
            let fault = if kind == "stage-io" {
                Some(Fault::at("status.qualify"))
            } else {
                None
            };
            if startup {
                bootstrap(&mut s, &data).unwrap();
            } else {
                assert!(
                    project_committed(s.conn(), &data)
                        .unwrap_err()
                        .is::<StatusQualificationFailure>()
                );
            }
            drop(fault);
            assert!(observed.get());
            let s = reopen(&data, s);
            let p = projection(&s);
            assert_eq!(p.state, "pending");
            assert!(p.current.is_none());
            assert_eq!(
                p.retired_current.as_ref().unwrap().inode,
                original.current.unwrap().inode
            );
            assert_eq!(p.owned.len(), original.owned.len());
            assert_eq!(p.manifests.len(), original.manifests.len());
            assert_eq!(serde_json::to_value(state(&s)).unwrap(), original_facts);
            assert_eq!(
                fs::read(user.join("user-text")).unwrap(),
                b"must not touch status target"
            );
            if kind == "body-edit" {
                let body = p
                    .owned
                    .iter()
                    .find(|f| f.path.starts_with("body-"))
                    .unwrap();
                assert_eq!(
                    fs::read(data.join("welcome-crew").join(&body.path)).unwrap(),
                    b"status-stage user edit"
                );
            }
            if kind == "root-file" {
                assert_eq!(
                    fs::read(data.join("welcome-crew")).unwrap(),
                    b"user root replacement"
                );
            }
            no_automation(&s);
        }
    }
}

#[test]
fn p1_status_diagnostic_write_error_is_best_effort_only_while_closure_stays_qualified() {
    let (_t, data, _lock, mut s) = fresh();
    bootstrap(&mut s, &data).unwrap();
    let before = projection(&s).current.unwrap();
    {
        let _fault = Fault::at("status.write");
        project_committed(s.conn(), &data).unwrap();
    }
    let s = reopen(&data, s);
    let p = projection(&s);
    assert_eq!(p.state, "ready");
    assert_eq!(p.current.as_ref().unwrap().inode, before.inode);
    assert!(qualified(s.conn(), &data, &p).unwrap());
}

#[test]
fn p1_status_revocation_commit_failure_reports_unknown_instead_of_ready_success() {
    for startup in [false, true] {
        let (_t, data, _lock, mut s) = fresh();
        bootstrap(&mut s, &data).unwrap();
        let _hook = StatusHook::once(|c, data| {
            let p = load(c).unwrap().unwrap();
            assert_eq!(p.state, "ready");
            assert!(p.current.is_some());
            c.execute_batch("CREATE TRIGGER deny_status_revoke BEFORE UPDATE ON meta WHEN NEW.key='onboarding.export.v1' AND instr(NEW.value,'\"current\":null')>0 BEGIN SELECT RAISE(ABORT,'controlled trusted revocation refusal'); END;").unwrap();
            fs::set_permissions(data.join("welcome-crew"), fs::Permissions::from_mode(0o755))
                .unwrap();
        });
        let error = if startup {
            bootstrap(&mut s, &data).unwrap_err()
        } else {
            project_committed(s.conn(), &data).unwrap_err()
        };
        assert!(error.is::<StatusQualificationFailure>());
        assert!(error.to_string().contains("readiness unknown"));
        let s = reopen(&data, s);
        // The failing trusted transaction did NOT revoke anything. Never assert or
        // report otherwise: the typed error must propagate even while Core says ready.
        assert_eq!(projection(&s).state, "ready");
        assert!(projection(&s).current.is_some());
        s.conn()
            .execute_batch("DROP TRIGGER deny_status_revoke")
            .unwrap();
        assert!(project_committed(s.conn(), &data).is_err());
        assert!(projection(&s).current.is_none());
    }
}

#[test]
fn q1_partial_real_unlink_then_selected_io_failure_preserves_tail_at_capacity_and_reopen() {
    for bound in [0, FILE_LIMIT - 1, FILE_LIMIT] {
        let (_t, data, _lock, mut s) = fresh();
        bootstrap(&mut s, &data).unwrap();
        let root = data.join("welcome-crew");
        let mut p = projection(&s);
        // Capacity fixture ownership is witnessed by actual create_new files,
        // original inode/device, complete bytes and digest; no invented inodes.
        while p.owned.len() < bound {
            let path = format!("real-owned-history-{}.md", p.owned.len());
            let bytes = b"original owned history";
            let mut file = OpenOptions::new()
                .write(true)
                .create_new(true)
                .mode(0o400)
                .open(root.join(&path))
                .unwrap();
            file.write_all(bytes).unwrap();
            let m = file.metadata().unwrap();
            p.owned.push(ExportFile {
                path,
                sha256: sha(bytes),
                bytes: bytes.len() as u64,
                device: Some(m.dev()),
                inode: Some(m.ino()),
            });
        }
        save(s.conn(), &p).unwrap();
        let fail = p.owned[2].clone();
        let unvisited = p.owned[3].clone();
        let absent_later = p.owned[4].clone();
        let user = p.owned.last().unwrap().clone();
        fs::set_permissions(root.join(&user.path), fs::Permissions::from_mode(0o600)).unwrap();
        fs::write(root.join(&user.path), b"original user changed history").unwrap();
        fs::remove_file(root.join(&absent_later.path)).unwrap();
        archive_welcome_root(&mut s, 200);
        collab::run(&mut s, 200, |tx| collab::crews::purge(tx, facts::ROOT)).unwrap();
        {
            let _fault = SelectedUnlinkFault::at(&fail.path);
            assert!(project_committed(s.conn(), &data).is_err());
        }
        // At least two real successful unlinks precede the injected failure;
        // remaining paths have never been visited and must survive the cut.
        for f in &p.owned[..2] {
            assert!(fs::symlink_metadata(root.join(&f.path)).is_err());
        }
        assert!(matches(&root, &fail).unwrap());
        assert!(matches(&root, &unvisited).unwrap());
        let s = reopen(&data, s);
        let remaining = projection(&s);
        assert_eq!(remaining.state, "cleanupUnverified");
        assert!(remaining.current.is_none());
        assert_eq!(remaining.owned.len(), p.owned.len() - 2);
        assert_eq!(remaining.manifests.len(), p.manifests.len());
        for f in [&fail, &unvisited, &absent_later, &user] {
            let retained = remaining.owned.iter().find(|x| x.path == f.path).unwrap();
            assert_eq!(
                serde_json::to_value(retained).unwrap(),
                serde_json::to_value(f).unwrap()
            );
        }
        assert!(remaining.retired_current.is_some());
        assert!(remaining.owned.len() <= FILE_LIMIT);
        assert!(project_committed(s.conn(), &data).is_err()); // only user-altered history remains
        let after = projection(&s);
        assert_eq!(after.state, "cleanupUnverified");
        assert!(after.current.is_none());
        assert_eq!(after.owned.len(), 1);
        assert_eq!(after.owned[0].path, user.path);
        assert_eq!(
            fs::read(root.join(&user.path)).unwrap(),
            b"original user changed history"
        );
        assert!(!root.join(&fail.path).exists());
        assert!(!root.join(&unvisited.path).exists());
        assert!(!after.owned.iter().any(|f| f.path == absent_later.path));
    }
}

fn original_b_facts(c: &rusqlite::Connection) -> Value {
    let mut object = c
        .prepare("SELECT kind,id,revision,archived,body FROM _pc_objects ORDER BY kind,id")
        .unwrap();
    let objects = object
        .query_map([], |r| {
            Ok((
                r.get::<_, String>(0)?,
                r.get::<_, String>(1)?,
                r.get::<_, i64>(2)?,
                r.get::<_, i64>(3)?,
                r.get::<_, String>(4)?,
            ))
        })
        .unwrap()
        .collect::<Result<Vec<_>, _>>()
        .unwrap();
    let mut revision = c
        .prepare("SELECT kind,id,revision,body FROM _pc_revisions ORDER BY kind,id,revision")
        .unwrap();
    let revisions = revision
        .query_map([], |r| {
            Ok((
                r.get::<_, String>(0)?,
                r.get::<_, String>(1)?,
                r.get::<_, i64>(2)?,
                r.get::<_, String>(3)?,
            ))
        })
        .unwrap()
        .collect::<Result<Vec<_>, _>>()
        .unwrap();
    let mut receipt = c
        .prepare("SELECT id,body FROM _pc_receipts ORDER BY id")
        .unwrap();
    let receipts = receipt
        .query_map([], |r| Ok((r.get::<_, String>(0)?, r.get::<_, String>(1)?)))
        .unwrap()
        .collect::<Result<Vec<_>, _>>()
        .unwrap();
    json!({"objects":objects,"revisions":revisions,"receipts":receipts})
}

#[test]
fn q2_actual_committed_b_with_independent_current_scope_conflicts_never_replays_or_publishes_c() {
    for field in [
        "restore",
        "generation",
        "authority",
        "namespace",
        "database",
        "crew",
    ] {
        let (_t, data, _lock, mut s) = fresh();
        s.conn().execute_batch("CREATE TRIGGER hold_scope_C BEFORE INSERT ON crews BEGIN SELECT RAISE(ABORT,'hold original C after B'); END;").unwrap();
        assert!(bootstrap(&mut s, &data).is_err());
        let plan = state(&s).pending.unwrap();
        let raw_plan = crewd_store::meta_get(s.conn(), facts::KEY)
            .unwrap()
            .unwrap();
        let k = knowledge(&s, &data);
        let before = original_b_facts(&k.conn);
        assert_eq!(before["receipts"].as_array().unwrap().len(), 1);
        let receipt = k
            .retry(
                &plan.request,
                &plan.origin,
                facts::PRINCIPAL,
                &plan.payload_sha256,
            )
            .unwrap()
            .unwrap();
        assert_eq!(receipt.request_id, plan.request);
        let mut context = k.context().unwrap();
        match field {
            "restore" => context.restore_epoch = "changed-original-restore-epoch".into(),
            "generation" => {
                context.generation = (context.generation.parse::<u64>().unwrap() + 1).to_string()
            }
            "authority" => context.authority_id = uuid::Uuid::new_v4().to_string(),
            "namespace" => context.namespace = uuid::Uuid::new_v4().to_string(),
            "database" => context.db_id = uuid::Uuid::new_v4().to_string(),
            "crew" => context.crew_id = "foreign-original-owner".into(),
            _ => unreachable!(),
        }
        let raw_context = serde_json::to_string(&context).unwrap();
        k.conn
            .execute(
                "UPDATE _pc_meta SET value=?1 WHERE key='context'",
                [&raw_context],
            )
            .unwrap();
        drop(k);
        s.conn().execute_batch("DROP TRIGGER hold_scope_C").unwrap();
        let mut s = reopen(&data, s);
        assert!(bootstrap(&mut s, &data).is_err());
        assert_eq!(state(&s).pending.unwrap().request, plan.request);
        assert_eq!(
            crewd_store::meta_get(s.conn(), facts::KEY)
                .unwrap()
                .unwrap(),
            raw_plan
        );
        for table in [
            "crews",
            "crew_parents",
            "channels",
            "channel_knowledge_links",
        ] {
            let count: i64 = s
                .conn()
                .query_row(&format!("SELECT count(*) FROM {table}"), [], |r| r.get(0))
                .unwrap();
            assert_eq!(count, 0, "{field}/{table}");
        }
        // Normal Knowledge::open must refuse some alien contexts. This reader
        // inspects only our exact original private file; it cannot repair/adopt it.
        let path = data
            .join("knowledge")
            .join(format!("{}.sqlite", plan.origin.namespace));
        let raw =
            rusqlite::Connection::open_with_flags(path, rusqlite::OpenFlags::SQLITE_OPEN_READ_ONLY)
                .unwrap();
        assert_eq!(original_b_facts(&raw), before);
        let after: String = raw
            .query_row("SELECT value FROM _pc_meta WHERE key='context'", [], |r| {
                r.get(0)
            })
            .unwrap();
        assert_eq!(after, raw_context);
        no_automation(&s);
    }
}

struct OwnedService {
    owner: std::sync::Arc<crate::AppState>,
    client: crate::agent::testkit::RpcClient,
    stop: tokio::sync::oneshot::Sender<()>,
    task: tokio::task::JoinHandle<anyhow::Result<()>>,
    data: std::path::PathBuf,
}
impl OwnedService {
    async fn start(data: std::path::PathBuf) -> Self {
        let (ready_tx, ready_rx) = tokio::sync::oneshot::channel();
        let (stop, stopping) = tokio::sync::oneshot::channel();
        let task = tokio::spawn(crate::serve_with(
            data.clone(),
            crate::ServeOptions {
                test_ready: Some(Box::new(move |state, _| {
                    let _ = ready_tx.send(state);
                })),
                ..Default::default()
            },
            async {
                let _ = stopping.await;
            },
        ));
        let owner = tokio::time::timeout(std::time::Duration::from_secs(10), ready_rx)
            .await
            .unwrap()
            .unwrap();
        let client = crate::agent::testkit::RpcClient::connect(&data.join("crewd.sock")).await;
        Self {
            owner,
            client,
            stop,
            task,
            data,
        }
    }
    async fn stop(self) {
        let Self {
            owner,
            client,
            stop,
            task,
            ..
        } = self;
        drop(client);
        drop(owner);
        stop.send(()).unwrap();
        tokio::time::timeout(std::time::Duration::from_secs(10), task)
            .await
            .unwrap()
            .unwrap()
            .unwrap();
    }
    async fn workspace(&mut self) -> Value {
        self.client
            .call("data.workspace.get", json!({"crewId":facts::ROOT}))
            .await
            .unwrap()
    }
    async fn object(&mut self, context: &Value, namespace: &str, kind: &str, id: &str) -> Value {
        self.client
            .call(
                "data.object.get",
                json!({"context":context,"reference":{"namespace":namespace,"kind":kind,"id":id}}),
            )
            .await
            .unwrap()
    }
}
fn quiet_business(s: &Store) -> Value {
    no_automation(s);
    // Current H/Work authority is the organization book, not the legacy
    // work_books view. Validate its typed state and original public identities
    // as well as preserving every raw state/event/metadata row below.
    let authority = crewd_store::work_organization::load(s.conn())
        .unwrap()
        .expect("current organization work authority must exist");
    crewd_store::work_organization::verify_references(s.conn(), &authority).unwrap();
    let mut result = serde_json::Map::new();
    for (table, predicate) in [
        ("messages", ""),
        ("organization_work_book", ""),
        ("organization_work_events", ""),
        ("ask_meta", ""),
        ("todos", ""),
        ("todo_attributes", ""),
        ("todo_attribute_events", ""),
        ("todo_responses", ""),
        ("work_creation_requests", ""),
        ("object_refs", ""),
        // H/requirement mutation receipts share this table with normal
        // channel.knowledgeUnlink. The latter is the expected reference
        // mutation tested separately by its original UUID/source/CAS receipt;
        // it must not masquerade as a change to H/Work authority or receipts.
        (
            "resource_mutation_requests",
            " WHERE object_kind <> 'knowledgeView'",
        ),
        // Preserve the former valid legacy snapshots as additional evidence;
        // they are never substituted for organization_work_book/events.
        ("work_books", ""),
        ("work_events", ""),
    ] {
        let mut query = s
            .conn()
            .prepare(&format!("SELECT * FROM {table}{predicate}"))
            .unwrap();
        let columns = query.column_count();
        let mut rows = query
            .query_map([], |row| {
                (0..columns)
                    .map(|index| row.get_ref(index).map(|value| format!("{value:?}")))
                    .collect::<Result<Vec<_>, _>>()
            })
            .unwrap()
            .collect::<Result<Vec<_>, _>>()
            .unwrap();
        rows.sort();
        result.insert(table.into(), json!(rows));
    }
    Value::Object(result)
}

#[tokio::test]
async fn q3_independent_active_ready_unlink_archive_and_derived_failure_keep_receipts_through_second_serve()
 {
    tokio::time::timeout(std::time::Duration::from_secs(90),async {
        for action in ["unlink","archiveView","archiveBlock","derived-failure"] {
            let t=tempfile::tempdir().unwrap();let data=t.path().join("data");
            let mut first=OwnedService::start(data.clone()).await;
            let canonical=fs::canonicalize(&first.data).unwrap();let scope=canonical.clone();
            let (publication,original_facts,quiet)=first.owner.db.call_result(move |s| {
                assert_eq!(projection(s).state,"ready");assert!(qualified(s.conn(),&scope,&projection(s)).unwrap());
                Ok::<_,crewd_store::StoreError>((state(s).ready[0].clone(),serde_json::to_value(state(s)).unwrap(),quiet_business(s)))
            }).await.unwrap().unwrap();
            let page=publication.pages[0].clone();let workspace=first.workspace().await;
            // The immutable receipt capsule belongs to this exact pre-submit
            // context. Current/after contexts remain correct for object reads,
            // but never replace this origin when reconciling the original UUID.
            let submitted_context=workspace["context"].clone();
            let namespace=workspace["context"]["namespace"].as_str().unwrap().to_string();
            let original_block=first.object(&workspace["context"],&namespace,"block",&page.block).await;
            let original_view=first.object(&workspace["context"],&namespace,"view",&page.view).await;
            assert_eq!(original_block["object"]["archived"],false);assert_eq!(original_view["object"]["archived"],false);
            let channel=crewd_store::channels::lobby_id(facts::ROOT);
            let request=uuid::Uuid::new_v4().to_string();let receipt;
            if action=="unlink" {
                let list=first.client.call("channel.knowledgeList",json!({"channelId":channel,"limit":100})).await.unwrap();
                let source=list["items"].as_array().unwrap().iter().find(|item|item["source"]["reference"]["id"]==page.view).unwrap()["source"].clone();
                receipt=first.client.call("channel.knowledgeUnlink",json!({"requestId":request,"channelId":channel,"expectedLinkRevision":list["linkRevision"],"source":source})).await.unwrap();
                assert_eq!(receipt["linked"],false);assert_eq!(receipt["source"],source);
                let after=first.client.call("channel.knowledgeList",json!({"channelId":channel,"limit":100})).await.unwrap();
                assert_eq!(after["linkRevision"],receipt["linkRevision"]);
                assert!(!after["items"].as_array().unwrap().iter().any(|item|item["source"]["reference"]["id"]==page.view));
                let current=first.workspace().await;
                assert_eq!(first.object(&current["context"],&namespace,"block",&page.block).await,original_block);
                assert_eq!(first.object(&current["context"],&namespace,"view",&page.view).await,original_view);
            } else {
                if action=="derived-failure" {
                    // Arm the existing test-only TLS on the real dedicated Db
                    // writer, not the test thread. The next normal RPC must
                    // commit before Hub observes a STATUS-only derived error.
                    first.owner.db.call_result(|s| {
                        assert_eq!(projection(s).state,"ready");
                        FAULT.with(|slot| {assert!(slot.borrow().is_none());*slot.borrow_mut()=Some("status.qualify");});
                        Ok::<_,crewd_store::StoreError>(())
                    }).await.unwrap().unwrap();
                }
                let edit=match action {
                    "archiveView"=>json!({"kind":"archiveView","id":page.view,"expectedRevision":original_view["object"]["revision"],"archived":true}),
                    "archiveBlock"=>json!({"kind":"archiveBlock","id":page.block,"expectedRevision":original_block["object"]["revision"],"archived":true}),
                    "derived-failure"=>json!({"kind":"updateBlock","id":page.block,"expectedRevision":original_block["object"]["revision"],"contentType":"text/plain","content":"Committed normal document despite derived failure","sources":null,"attributes":{}}),
                    _=>unreachable!(),
                };
                receipt=first.client.call("data.submit",json!({"requestId":request,"context":submitted_context,"change":{"kind":"apply","message":"Original active tutorial mutation","edits":[edit]}})).await.unwrap();
                assert_eq!(receipt["phase"],"committed");assert_eq!(receipt["requestId"],request);
                let lookup=first.client.call("data.request.get",json!({"requestId":request,"context":submitted_context})).await.unwrap();
                assert_eq!(lookup["lookup"]["status"],"known");assert_eq!(lookup["lookup"]["result"],receipt);
                assert_ne!(submitted_context,receipt["context"]);
                let refused=first.client.call("data.request.get",json!({"requestId":request,"context":receipt["context"]})).await.unwrap_err();
                assert_eq!(refused.0,crewd_proto::RpcError::CONFLICT);
                assert!(refused.1.contains("original request owner or immutable context differs"));
                if action=="derived-failure" {
                    first.owner.db.call_result(|s| {
                        assert_eq!(projection(s).state,"pending");assert!(projection(s).current.is_none());
                        assert!(projection(s).retired_current.is_some());
                        FAULT.with(|slot| {assert_eq!(*slot.borrow(),Some("status.qualify"));*slot.borrow_mut()=None;});
                        Ok::<_,crewd_store::StoreError>(())
                    }).await.unwrap().unwrap();
                    let object=first.object(&receipt["context"],&namespace,"block",&page.block).await;
                    assert_eq!(object["object"]["revision"],"2");assert_eq!(object["object"]["content"],"Committed normal document despite derived failure");
                } else {
                    let (kind,id)=if action=="archiveView" {("view",&page.view)} else {("block",&page.block)};
                    let object=first.object(&receipt["context"],&namespace,kind,id).await;
                    assert_eq!(object["object"]["id"],*id);assert_eq!(object["object"]["revision"],"2");assert_eq!(object["object"]["archived"],true);
                }
            }
            let facts_before=original_facts.clone();let quiet_before=quiet.clone();
            first.owner.db.call_result(move |s| {
                assert_ne!(projection(s).state,"ready");assert!(projection(s).current.is_none());
                assert_eq!(serde_json::to_value(state(s)).unwrap(),facts_before);assert_eq!(quiet_business(s),quiet_before);
                Ok::<_,crewd_store::StoreError>(())
            }).await.unwrap().unwrap();
            first.stop().await;assert!(!data.join("crewd.sock").exists());
            // Actual cold read between two fully joined serve generations.
            // This is not substituted for the second serve below.
            let cold=Store::open_data_dir(&canonical).unwrap();assert!(projection(&cold).current.is_none());
            assert_eq!(serde_json::to_value(state(&cold)).unwrap(),original_facts);drop(cold);
            let mut second=OwnedService::start(data.clone()).await;
            let current=second.workspace().await;
            if action=="unlink" {
                let list=second.client.call("channel.knowledgeList",json!({"channelId":channel,"limit":100})).await.unwrap();
                assert!(!list["items"].as_array().unwrap().iter().any(|item|item["source"]["reference"]["id"]==page.view));
                assert_eq!(second.object(&current["context"],&namespace,"block",&page.block).await,original_block);
                assert_eq!(second.object(&current["context"],&namespace,"view",&page.view).await,original_view);
            } else {
                let lookup=second.client.call("data.request.get",json!({"requestId":request,"context":submitted_context})).await.unwrap();
                assert_eq!(lookup["lookup"]["status"],"known");assert_eq!(lookup["lookup"]["result"],receipt);
                assert_ne!(submitted_context,current["context"]);
                let refused=second.client.call("data.request.get",json!({"requestId":request,"context":current["context"]})).await.unwrap_err();
                assert_eq!(refused.0,crewd_proto::RpcError::CONFLICT);
                assert!(refused.1.contains("original request owner or immutable context differs"));
                let (kind,id)=if action=="archiveView" {("view",&page.view)} else {("block",&page.block)};
                let object=second.object(&current["context"],&namespace,kind,id).await;
                assert_eq!(object["object"]["id"],*id);assert_eq!(object["object"]["revision"],"2");
                if action=="derived-failure" {assert_eq!(object["object"]["content"],"Committed normal document despite derived failure");}
                else {assert_eq!(object["object"]["archived"],true);}
            }
            let scope=canonical.clone();
            second.owner.db.call_result(move |s| {
                assert_eq!(serde_json::to_value(state(s)).unwrap(),original_facts);assert_eq!(quiet_business(s),quiet);
                if action=="derived-failure" {assert!(qualified(s.conn(),&scope,&projection(s)).unwrap());}
                else {assert_ne!(projection(s).state,"ready");assert!(projection(s).current.is_none());}
                Ok::<_,crewd_store::StoreError>(())
            }).await.unwrap().unwrap();
            second.stop().await;assert!(!data.join("crewd.sock").exists());
        }
    }).await.expect("bounded independent active-ready mutations and two actual serve generations");
}

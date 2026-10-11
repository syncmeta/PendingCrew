// Seeded typed stamps below exercise real storage inspection/transactions, not actual PTY ancestry.
// The generated MCP three-party fixture provides the separate genuine producer/caller proof.
type AdmissionState = (Vec<(String, String)>, Vec<(String, String, i64, String)>, Vec<crate::wakes::WakeRow>);
fn admission_state(s: &Store) -> AdmissionState {
    let mut q=s.conn().prepare("SELECT reader,channel_id,seq,updated_at FROM read_cursors ORDER BY reader,channel_id").unwrap();
    let cursors=q.query_map([],|r|Ok((r.get(0)?,r.get(1)?,r.get(2)?,r.get(3)?))).unwrap().collect::<rusqlite::Result<_>>().unwrap();
    let mut q=s.conn().prepare("SELECT id FROM wakes ORDER BY id").unwrap();
    let ids=q.query_map([],|r|r.get(0)).unwrap().collect::<rusqlite::Result<Vec<i64>>>().unwrap();
    (durable(s),cursors,crate::wakes::by_ids(s.conn(),&ids).unwrap())
}
fn seed_message_hop(s:&Store,m:&Message,hop:u32)->Frontier {
    let key=format!("{MESSAGE}{}",m.id);
    let mut stamp=read::<Stamp>(s.conn(),&key,"message").unwrap().unwrap();
    let mut f=match stamp.resolution {Resolution::Resolved{frontier}=>frontier,_=>panic!("fixture requires existing resolved actual message")};
    for v in &mut f.0{v.hop=hop;}
    stamp.resolution=Resolution::Resolved{frontier:f.clone()};
    put(s.conn(),&key,"message",&stamp).unwrap();f
}
fn selected_message(s:&Store,target:&str,m:&Message)->(cr::Snapshot,CausalSnapshot,Vec<cr::Window>) {
    let snap=cr::snapshot(s.conn(),target,std::slice::from_ref(&m.channel_id),NOW).unwrap();
    let ends=[(m.channel_id.clone(),m.seq)];let causes=capture(s.conn(),&snap,&ends).unwrap();
    let windows=snap.windows_for(&ends).unwrap();(snap,causes,windows)
}
fn session_input_at(s:&mut Store,session:&str,channel:&str,text:&str)->Message {
    ok(s,|tx|messages::post(tx,Sender::Session(session.into()),messages::NewPost{channel_id:channel.into(),text:text.into(),..Default::default()})).message
}

#[test]
fn admission_exact16_positive_over16_candidate_and_existing_episode_are_distinct() {
    let (mut s,_)=fixture();let m=post(&mut s,Sender::Session(READER.into()),"actual stored session row");
    seed_message_hop(&s,&m,16);
    let(_,causes,_)=selected_message(&s,"w-a2",&m);
    assert!(!inspect_admission(s.conn(),&causes,CAUSAL_HOP_MAX,NOW).unwrap().blocked(),"exact16 must remain eligible");
    seed_message_hop(&s,&m,17);let(_,causes,_)=selected_message(&s,"w-a2",&m);
    let a=inspect_admission(s.conn(),&causes,CAUSAL_HOP_MAX,NOW).unwrap();assert!(a.blocked());
    assert!(a.channels[0].holds.is_empty());assert_eq!(a.channels[0].proposed_holds.len(),1);
    let h=a.channels[0].proposed_holds[0].clone();assert_eq!(h.from,READER);assert_eq!(h.hop,17);
    assert!(ok(&mut s,|tx|hold(tx,&h)));
    let a=inspect_admission(s.conn(),&causes,CAUSAL_HOP_MAX,NOW+1).unwrap();
    assert_eq!(a.channels[0].holds,vec![h]);assert!(a.channels[0].proposed_holds.is_empty(),"readonly candidate is never reported as durable before INSERT");
}

#[test]
fn admission_human_row_is_immune_but_actual_session_descendant_is_counted() {
    let (mut s,input)=fixture();seed_message_hop(&s,&input,99);
    let(_,causes,_)=selected_message(&s,"w-a2",&input);
    assert!(!inspect_admission(s.conn(),&causes,16,NOW).unwrap().blocked());
    let c=claim(&mut s,READER,1);ack(&mut s,&c,&witness(&c));
    let child=post(&mut s,Sender::Session(READER.into()),"actual session child of Human message");
    let(_,causes,_)=selected_message(&s,"w-a2",&child);
    assert!(inspect_admission(s.conn(),&causes,16,NOW).unwrap().blocked(),"a Human root does not exempt its Session descendant");
}

#[test]
fn admission_captain_system_relay_and_source_less_system_use_actual_routes() {
    for (sender,relay,expected) in [(Sender::Session("cap-a".into()),None,"cap-a"),(Sender::System,Some(READER),READER),(Sender::System,None,"system")] {
        let (mut s,_)=fixture();
        let m=ok(&mut s,|tx|{let p=messages::NewPost{channel_id:CH.into(),text:"actual routed fixture message".into(),..Default::default()};match relay{Some(source)=>messages::post_stop_relay(tx,source,sender,p),None=>messages::post(tx,sender,p)}}).message;
        seed_message_hop(&s,&m,17);let(_,causes,_)=selected_message(&s,"w-a2",&m);
        let a=inspect_admission(s.conn(),&causes,16,NOW).unwrap();assert!(a.blocked());
        assert_eq!(a.channels[0].proposed_holds[0].from,expected,"route comes from actual kind/sender/private relay source, never wake IDs");
    }
}

#[test]
fn admission_mixed_indivisible_body_blocks_only_its_channel() {
    let (mut s,_)=fixture();let reader=crate::testkit::org_worker(&mut s);
    ok(&mut s,|tx|{crate::listens::set(tx,&reader,CH,None,vec![])?;crate::listens::set(tx,&reader,"crew-x/lobby",None,vec![])});
    let bad=session_input_at(&mut s,READER,CH,"bad component channel");seed_message_hop(&s,&bad,17);
    let good=session_input_at(&mut s,"cap-x","crew-x/lobby","independent eligible channel");
    let snap=cr::snapshot(s.conn(),&reader,&[CH.into(),"crew-x/lobby".into()],NOW).unwrap();
    let causes=capture(s.conn(),&snap,&[(CH.into(),bad.seq),("crew-x/lobby".into(),good.seq)]).unwrap();
    let a=inspect_admission(s.conn(),&causes,16,NOW).unwrap();
    assert!(a.channels.iter().find(|c|c.channel==CH).unwrap().blocked);
    assert!(!a.channels.iter().find(|c|c.channel=="crew-x/lobby").unwrap().blocked);
    let good_only=causes.select(&[("crew-x/lobby".into(),good.seq)]).unwrap();
    assert!(!inspect_admission(s.conn(),&good_only,16,NOW).unwrap().blocked(),"pure original snapshot selection can preserve unrelated channel");
}

#[test]
fn prepared_zero_id_claim_and_hook_recheck_new_exact_hold_without_mutating() {
    for hook in [false,true] {
        let (mut s,_)=fixture();let m=post(&mut s,Sender::Session(READER.into()),"prepared actual zero ID body");
        let(snap,causes,windows)=selected_message(&s,"w-a2",&m);
        let f=frontier(&s,&m);let h=ChainHold{chain_id:f.components()[0].chain_id(),from:READER.into(),target:"w-a2".into(),hop:0,reason:"existing exact episode inserted after prepare".into(),at_ms:NOW};
        ok(&mut s,|tx|hold(tx,&h));let before=admission_state(&s);
        if hook {assert_eq!(ok(&mut s,|tx|cr::commit_hook_output(tx,&snap,&causes,&[(CH.into(),m.seq)])),cr::Decision::Busy);}
        else {assert_eq!(ok(&mut s,|tx|cr::claim_output(tx,&snap,&causes,&windows,&[],&[],cr::Owner::new(1)?)),cr::Decision::Busy);}
        assert_eq!(admission_state(&s),before,"final hold CAS changes no attempts/cursors/receipt/lease");
    }
}

#[test]
fn diagnostics_is_readonly_reports_missing_cursor_and_no_fake_hold() {
    let (mut s,_)=fixture();let m=post(&mut s,Sender::Session(READER.into()),"diagnostic high hop actual row");seed_message_hop(&s,&m,17);
    let before=admission_state(&s);let a=diagnostics(s.conn(),"w-a2",NOW).unwrap();assert!(a.blocked());
    assert_eq!(admission_state(&s),before);assert!(a.channels[0].holds.is_empty());assert_eq!(a.channels[0].proposed_holds.len(),1);
    s.conn().execute("DELETE FROM read_cursors WHERE reader='w-a2' AND channel_id=?1",[CH]).unwrap();
    let before=admission_state(&s);let a=diagnostics(s.conn(),"w-a2",NOW).unwrap();
    assert_eq!(a.unknown_channels,vec![CH.to_string()]);assert!(a.channels.is_empty());assert_eq!(admission_state(&s),before,"pure diagnosis cannot lazy pin missing cursor");
}

#[test]
fn unresolved_diagnostic_is_not_frontier_and_exact_human_recovery_alone_grants_budget() {
    let (mut s,_)=fixture();let c=claim(&mut s,READER,1);let child=observed_post(&mut s,&c,None,"before any accepted successful output");
    let stamp=crewd_store::meta_get(s.conn(),&format!("{MESSAGE}{}",child.id)).unwrap().unwrap();
    let(_,causes,_)=selected_message(&s,"w-a2",&child);let a=inspect_admission(s.conn(),&causes,16,NOW).unwrap();
    let unresolved=&a.channels[0].unresolved[0];assert_eq!(unresolved.message_id,child.id);assert_eq!(unresolved.seq,child.seq);assert_eq!(unresolved.parent_receipt,c.pending.stable_key());
    assert!(causes.0.parts[causes.0.tuples.iter().position(|t|t.id==child.id).unwrap()].components().is_empty(),"diagnostic key is not ancestry");
    ack(&mut s,&c,&witness(&c));
    assert!(matches!(read_message(s.conn(),&child).unwrap(),MessageCause::Unresolved{..}),"later successful candidate cannot reparent this pre-success child");
    let h=a.channels[0].proposed_holds.iter().find(|h|h.chain_id==unresolved.diagnostic_chain_id).unwrap().clone();ok(&mut s,|tx|hold(tx,&h));
    let action=HumanRecoveryAction::new();assert!(ok(&mut s,|tx|recover(tx,&h.chain_id,READER,"w-a2",&action)));
    let f=effective_tuple_frontier(&s,"w-a2",&child);assert_eq!(f.components()[0].hop(),0);
    assert_eq!(f.components()[0].key.root,Root::Message{id:child.id.clone(),seq:child.seq});assert_eq!(f.components()[0].key.reset,ResetIdentity::Recovery{action:action.stable_key().into()});
    assert_eq!(crewd_store::meta_get(s.conn(),&format!("{MESSAGE}{}",child.id)).unwrap().unwrap(),stamp,"human overlay never rewrites source stamp");
    let(_,causes,_)=selected_message(&s,"w-a2",&child);assert!(!causes.unresolved());assert!(!inspect_admission(s.conn(),&causes,16,NOW).unwrap().blocked());
    assert!(!ok(&mut s,|tx|recover(tx,&h.chain_id,READER,"w-a2",&action)));
}

#[test]
fn admission_corrupt_stamp_and_sender_mismatch_are_fail_loud_not_legacy() {
    let (mut s,_)=fixture();let m=post(&mut s,Sender::Session(READER.into()),"strict actual sender fixture");
    let key=format!("{MESSAGE}{}",m.id);let original=crewd_store::meta_get(s.conn(),&key).unwrap().unwrap();
    let mut stamp=read::<Stamp>(s.conn(),&key,"message").unwrap().unwrap();stamp.source=Some("w-a2".into());put(s.conn(),&key,"message",&stamp).unwrap();
    assert!(diagnostics(s.conn(),"w-a2",NOW).is_err());
    crewd_store::meta_set(s.conn(),&key,"{broken").unwrap();assert!(diagnostics(s.conn(),"w-a2",NOW).is_err());
    crewd_store::meta_set(s.conn(),&key,&original).unwrap();assert!(diagnostics(s.conn(),"w-a2",NOW).is_ok());
}

#[test]
fn hold_insert_trigger_abort_rolls_back_whole_actual_episode() {
    let (mut s,_)=fixture();let m=post(&mut s,Sender::Session(READER.into()),"actual episode transaction");seed_message_hop(&s,&m,17);
    let(_,causes,_)=selected_message(&s,"w-a2",&m);let proposed=inspect_admission(s.conn(),&causes,16,NOW).unwrap().channels.remove(0).proposed_holds;
    let before=admission_state(&s);
    s.conn().execute_batch("CREATE TEMP TRIGGER causal_hold_abort BEFORE INSERT ON meta WHEN substr(NEW.key,1,length('causal.hold.v1.'))='causal.hold.v1.' BEGIN SELECT RAISE(ABORT,'synthetic causal hold abort'); END;").unwrap();
    let error=crate::run(&mut s,NOW,|tx|hold(tx,&proposed[0])).unwrap_err();
    assert!(matches!(error,CollabError::Store(StoreError::Sql(rusqlite::Error::SqliteFailure(code,Some(ref text)))) if code.extended_code==rusqlite::ffi::SQLITE_CONSTRAINT_TRIGGER&&text=="synthetic causal hold abort"));
    assert_eq!(admission_state(&s),before);
}

fn completed_recovery_fixture()->(Store,Message,HumanRecoveryAction) {
    let (mut s,_)=fixture();let c=claim(&mut s,READER,1);ack(&mut s,&c,&witness(&c));let m=post(&mut s,Sender::Session(READER.into()),"actual recovery ordering fixture");
    let chain=frontier(&s,&m).components()[0].chain_id();let h=ChainHold{chain_id:chain.clone(),from:READER.into(),target:"w-a2".into(),hop:1,reason:"real exact recovery fixture".into(),at_ms:NOW};ok(&mut s,|tx|hold(tx,&h));
    let action=HumanRecoveryAction::new();assert!(ok(&mut s,|tx|recover(tx,&chain,READER,"w-a2",&action)));(s,m,action)
}
fn second_recovery_hold(s:&mut Store,m:&Message)->ChainHold {
    let f=effective_tuple_frontier(s,"w-a2",m);let h=ChainHold{chain_id:f.components()[0].chain_id(),from:READER.into(),target:"w-a2".into(),hop:17,reason:"next actual recovery episode".into(),at_ms:NOW};ok(s,|tx|hold(tx,&h));h
}

#[test]
fn recovery_missing_corrupt_or_zero_order_cannot_hide_behind_duplicate_action() {
    let (mut s,m,action)=completed_recovery_fixture();let key=format!("{RECOVERY}{}",action.stable_key());let r=read::<Recovery>(s.conn(),&key,"recovery").unwrap().unwrap();
    let original=crewd_store::meta_get(s.conn(),RECOVERY_ORDER).unwrap().unwrap();
    for raw in ["{broken".to_string(),original.replace("\"version\":1","\"version\":2"),encoded("recovery_order",&RecoveryOrder{value:0}).unwrap()] {
        crewd_store::meta_set(s.conn(),RECOVERY_ORDER,&raw).unwrap();let before=admission_state(&s);
        assert!(crate::run(&mut s,NOW,|tx|recover(tx,&r.selected.chain_id,READER,"w-a2",&action)).is_err());assert_eq!(admission_state(&s),before);assert!(diagnostics(s.conn(),"w-a2",NOW).is_err());
    }
    s.conn().execute("DELETE FROM meta WHERE key=?1",[RECOVERY_ORDER]).unwrap();assert!(effective_message(s.conn(),"w-a2",&m).is_err());
}

#[test]
fn recovery_duplicate_stored_order_is_rejected_not_sorted_arbitrarily() {
    let (mut s,m,first)=completed_recovery_fixture();let h=second_recovery_hold(&mut s,&m);let second=HumanRecoveryAction::new();ok(&mut s,|tx|recover(tx,&h.chain_id,READER,"w-a2",&second));
    let key=format!("{RECOVERY}{}",second.stable_key());let mut r=read::<Recovery>(s.conn(),&key,"recovery").unwrap().unwrap();r.order=read::<Recovery>(s.conn(),&format!("{RECOVERY}{}",first.stable_key()),"recovery").unwrap().unwrap().order;put(s.conn(),&key,"recovery",&r).unwrap();
    assert!(recovery_records(s.conn()).is_err());assert!(diagnostics(s.conn(),"w-a2",NOW).is_err());
}

#[test]
fn recovery_checked_order_overflow_preserves_exact_hold_and_action_rows() {
    let (mut s,m,_)=completed_recovery_fixture();let h=second_recovery_hold(&mut s,&m);put(s.conn(),RECOVERY_ORDER,"recovery_order",&RecoveryOrder{value:u64::MAX}).unwrap();let before=admission_state(&s);let action=HumanRecoveryAction::new();
    let error=crate::run(&mut s,NOW,|tx|recover(tx,&h.chain_id,READER,"w-a2",&action)).unwrap_err();assert!(matches!(error,CollabError::Store(StoreError::Invalid(ref message)) if message.contains("overflow")));
    assert_eq!(admission_state(&s),before);assert!(holds(s.conn(),Some("w-a2")).unwrap().contains(&h));
}

#[test]
fn recovery_insert_and_exact_hold_delete_abort_roll_back_counter_overlay_and_hold() {
    for delete in [false,true] {
        let (mut s,m,_)=completed_recovery_fixture();let h=second_recovery_hold(&mut s,&m);let before=admission_state(&s);
        let trigger=if delete{"CREATE TEMP TRIGGER recovery_abort BEFORE DELETE ON meta WHEN substr(OLD.key,1,length('causal.hold.v1.'))='causal.hold.v1.' BEGIN SELECT RAISE(ABORT,'synthetic exact hold delete abort'); END;"}else{"CREATE TEMP TRIGGER recovery_abort BEFORE INSERT ON meta WHEN substr(NEW.key,1,length('causal.recovery.v1.'))='causal.recovery.v1.' BEGIN SELECT RAISE(ABORT,'synthetic recovery insert abort'); END;"};s.conn().execute_batch(trigger).unwrap();
        let action=HumanRecoveryAction::new();let error=crate::run(&mut s,NOW,|tx|recover(tx,&h.chain_id,READER,"w-a2",&action)).unwrap_err();
        assert!(matches!(error,CollabError::Store(StoreError::Sql(rusqlite::Error::SqliteFailure(code,Some(ref message)))) if code.extended_code==rusqlite::ffi::SQLITE_CONSTRAINT_TRIGGER&&message.starts_with("synthetic")));
        assert_eq!(admission_state(&s),before);assert!(holds(s.conn(),Some("w-a2")).unwrap().contains(&h));
    }
}

#[test]
fn strict_stored_capture_windows_and_unresolved_tuple_membership_are_validated() {
    let (mut s,_)=fixture();let c=claim(&mut s,READER,1);let key=format!("{OUTPUT}{}",c.pending.stable_key());let o=output(s.conn(),c.pending.stable_key()).unwrap();
    let mut changes=Vec::new();let mut x=o.clone();x.receipt.capture.windows[0].from_exclusive = -1;changes.push(x);
    let mut x=o.clone();x.receipt.capture.windows.push(x.receipt.capture.windows[0].clone());changes.push(x);
    let mut x=o.clone();x.receipt.capture.tuples[0].channel="uncaptured-channel".into();changes.push(x);
    let mut x=o.clone();x.receipt.capture.unresolved=true;x.receipt.capture.unresolved_ids=vec!["not-a-captured-tuple".into()];changes.push(x);
    for x in changes {put(s.conn(),&key,"output",&x).unwrap();assert!(output(s.conn(),c.pending.stable_key()).is_err());}
    put(s.conn(),&key,"output",&o).unwrap();assert!(output(s.conn(),c.pending.stable_key()).is_ok());
}


#[test]
fn diagnostic_recovery_cannot_release_known_chain_hold_on_same_unresolved_tuple() {
    let (mut s,input)=fixture();
    let original_parent=claim(&mut s,READER,1);
    let success=witness(&original_parent);
    // Real Session post after synthetic accepted-success emission, before its actual DB ack.
    // The actual immutable stamp has Eligible parent, with known input-chain X and unresolved D.
    let child=observed_post(&mut s,&original_parent,Some(&success),"known X plus unresolved exact parent on same tuple");
    let original_stamp=crewd_store::meta_get(s.conn(),&format!("{MESSAGE}{}",child.id)).unwrap().unwrap();
    let (_,before,_)=selected_message(&s,"w-a2",&child);
    assert!(before.unresolved(),"fixture must carry genuine stored unresolved parent");
    let x=frontier(&s,&input).components()[0].chain_id();
    let child_index=before.0.tuples.iter().position(|t|t.id==child.id).unwrap();
    assert_eq!(before.0.parts[child_index].components()[0].chain_id(),x,"same actual tuple carries known X identity");
    let initial=inspect_admission(s.conn(),&before,16,NOW).unwrap();
    let d=initial.channels[0].unresolved.iter().find(|v|v.message_id==child.id).unwrap().diagnostic_chain_id.clone();
    assert_ne!(x,d);
    let known_hold=ChainHold{chain_id:x.clone(),from:READER.into(),target:"w-a2".into(),hop:1,reason:"known component X exact episode".into(),at_ms:NOW};
    let diagnostic_hold=initial.channels[0].proposed_holds.iter().find(|h|h.chain_id==d).unwrap().clone();
    assert!(ok(&mut s,|tx|hold(tx,&known_hold)));
    assert!(ok(&mut s,|tx|hold(tx,&diagnostic_hold)));
    let diagnostic_action=HumanRecoveryAction::new();
    assert!(ok(&mut s,|tx|recover(tx,&d,READER,"w-a2",&diagnostic_action)));
    assert_eq!(holds(s.conn(),Some("w-a2")).unwrap(),vec![known_hold.clone()],"recover D must remove only D's exact hold");
    let (_,after_d,_)=selected_message(&s,"w-a2",&child);
    let inspected=inspect_admission(s.conn(),&after_d,16,NOW).unwrap();
    assert!(inspected.channels[0].blocked,"S42 diagnostic-only recovery must retain known X blocked on the same actual tuple");
    assert!(inspected.channels[0].holds.contains(&known_hold));
    let effective=effective_tuple_frontier(&s,"w-a2",&child);
    assert!(effective.components().iter().any(|v|v.chain_id()==x),"granting unknown D budget must not rename known X");
    let known_action=HumanRecoveryAction::new();
    assert!(ok(&mut s,|tx|recover(tx,&x,READER,"w-a2",&known_action)));
    let (_,after_x,_)=selected_message(&s,"w-a2",&child);
    assert!(!inspect_admission(s.conn(),&after_x,16,NOW).unwrap().blocked(),"only separate exact human recover X makes that body eligible");
    assert!(holds(s.conn(),Some("w-a2")).unwrap().is_empty());
    assert!(!ok(&mut s,|tx|recover(tx,&d,READER,"w-a2",&diagnostic_action)));
    assert!(!ok(&mut s,|tx|recover(tx,&x,READER,"w-a2",&known_action)));
    assert_eq!(crewd_store::meta_get(s.conn(),&format!("{MESSAGE}{}",child.id)).unwrap().unwrap(),original_stamp,"both receiver overlays leave actual source stamp immutable");
    ack(&mut s,&original_parent,&success);
}

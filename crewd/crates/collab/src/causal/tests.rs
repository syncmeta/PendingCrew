use super::*;
use crate::{Sender, TxContext, cursor_reservations as cr};
use crate::testkit::{NOW, ok, store};
use crewd_store::Store;
use std::sync::Arc;

const CH: &str = "crew-a/lobby";
const READER: &str = "w-a1";

fn applied<T: std::fmt::Debug>(d: cr::Decision<T>) -> T {
    match d { cr::Decision::Applied(v) => v, x => panic!("expected Applied, got {x:?}") }
}
fn post(s: &mut Store, who: Sender, text: &str) -> Message {
    ok(s, |tx| messages::post(tx, who, messages::NewPost { channel_id: CH.into(), text: text.into(), ..Default::default() })).message
}
fn fixture() -> (Store, Message) {
    let mut s = store();
    ok(&mut s, |tx| initialize(tx, READER));
    let m = post(&mut s, Sender::Human, "synthetic causal input");
    (s,m)
}
fn claim(s: &mut Store, reader: &str, generation: u64) -> ClaimedOutput {
    ok(s, |tx| {
        let prepared = cr::prepare_causal_unread(tx, reader)?;
        let ends = prepared.blocks.iter().filter_map(|(ch,ms)| ms.last().map(|m|(ch.clone(),m.seq))).collect::<Vec<_>>();
        let causes = prepared.causes.select(&ends)?;
        let windows = prepared.snapshot.windows_for(&ends)?;
        Ok(applied(cr::claim_output(tx, &prepared.snapshot, &causes, &windows, &[], &[], cr::Owner::new(generation)?)?))
    })
}
fn witness(c: &ClaimedOutput) -> ObservedSuccess {
    // Synthetic trusted adapter capability. Actual physical producer is tested by server/exec.
    c.pending.observe_success(7,c.pending.generation(),&uuid::Uuid::new_v4().to_string(),1).unwrap()
}
struct FixedObservation { session: String, value: PostObservation }
impl PostObservationSource for FixedObservation {
    fn observe(&self, session: &str) -> Result<PostObservation> {
        assert_eq!(session, self.session, "fixture observation scoped to its real sender");
        Ok(self.value.clone())
    }
}
fn observed_post(s: &mut Store, c: &ClaimedOutput, success: Option<&ObservedSuccess>, text: &str) -> Message {
    let producer = success.map(|v|match &v.0.kind {SuccessKind::Executor{producer,..}=>producer.clone(),_=>panic!("not executor")}).unwrap_or_else(||uuid::Uuid::new_v4().to_string());
    let observation = PostObservation::new(&producer,2,Some(c.pending.clone()),success.cloned()).unwrap();
    let context = TxContext::new(Arc::new(FixedObservation { session:c.pending.session().into(),value:observation }));
    crate::run_with_context(s,NOW,context,|tx| messages::post(tx,Sender::Session(c.pending.session().into()),messages::NewPost{channel_id:CH.into(),text:text.into(),..Default::default()})).unwrap().0.message
}
fn frontier(s: &Store, m: &Message) -> Frontier {
    match read_message(s.conn(),m).unwrap() {MessageCause::Resolved(f)=>f, other=>panic!("expected resolved {other:?}")}
}
fn ack(s: &mut Store,c: &ClaimedOutput, success: &ObservedSuccess) -> OutputCommit {
    applied(ok(s,|tx| cr::ack_output_delivered(tx,c,success,crate::wakes::Riders::Release)))
}
fn durable(s: &Store) -> Vec<(String,String)> {
    let mut q=s.conn().prepare("SELECT key,value FROM meta ORDER BY key").unwrap();
    q.query_map([],|r|Ok((r.get(0)?,r.get(1)?))).unwrap().collect::<rusqlite::Result<_>>().unwrap()
}

#[test]
fn candidate_before_success_never_gains_later_parent() {
    let (mut s,_) = fixture();
    let c=claim(&mut s,READER,1);
    let before=observed_post(&mut s,&c,None,"queued before successful outcome");
    assert!(matches!(read_message(s.conn(),&before).unwrap(),MessageCause::Unresolved{..}));
    ack(&mut s,&c,&witness(&c));
    assert!(matches!(read_message(s.conn(),&before).unwrap(),MessageCause::Unresolved{..}),"later success cannot retroactively activate candidate");
}

#[test]
fn observed_success_before_db_ack_resolves_once_and_repeat_post_keeps_basis() {
    let (mut s,input)=fixture();let c=claim(&mut s,READER,1);let success=witness(&c);
    let child=observed_post(&mut s,&c,Some(&success),"after synthetic accepted-success fence before DB ack");
    assert!(matches!(read_message(s.conn(),&child).unwrap(),MessageCause::Unresolved{..}));
    assert!(ack(&mut s,&c,&success).newly_committed);
    let f=frontier(&s,&child);assert_eq!(f.components().len(),1);assert_eq!(f.components()[0].hop(),1);
    assert_eq!(f.components()[0].chain_id(),frontier(&s,&input).components()[0].chain_id());
    assert!(!ack(&mut s,&c,&success).newly_committed,"duplicate exact success is not another hop");
    let repeated=observed_post(&mut s,&c,Some(&success),"same output second post");
    assert_eq!(frontier(&s,&repeated),f,"same output reuses canonical basis");
}

#[test]
fn failed_candidate_stays_unresolved_and_does_not_create_free_root() {
    let (mut s,_) = fixture();let c=claim(&mut s,READER,1);
    let child=observed_post(&mut s,&c,None,"before failed output");
    applied(ok(&mut s,|tx| cr::ack_output_failed(tx,&c,"synthetic failure", |_|None)));
    assert!(matches!(read_message(s.conn(),&child).unwrap(),MessageCause::Unresolved{..}));
    assert!(crate::run(&mut s,NOW,|tx| finalize(tx,&c.pending,&witness(&c))).is_err());
}

#[test]
fn first_actual_work_output_hop_zero_then_downstream_one_same_key() {
    let (mut s,input)=fixture();let c=claim(&mut s,READER,1);
    ok(&mut s,|tx| crate::sessions::note_progress(tx,READER,ProgressKind::ToolCall));
    let success=witness(&c);let first=observed_post(&mut s,&c,Some(&success),"first real work output");
    ack(&mut s,&c,&success);let f=frontier(&s,&first);
    assert_eq!(f.components()[0].hop(),0);
    assert_ne!(f.components()[0].chain_id(),frontier(&s,&input).components()[0].chain_id());
    let repeat=observed_post(&mut s,&c,Some(&success),"same work output repeat");
    assert_eq!(frontier(&s,&repeat),f);
    // Consume only this exact message through a genuine hook transaction for another reader.
    let prepared=cr::snapshot(s.conn(),"w-a2",&[CH.into()],NOW).unwrap();
    let causes=capture(s.conn(),&prepared,&[(CH.into(),first.seq)]).unwrap();
    applied(ok(&mut s,|tx| cr::commit_hook_output(tx,&prepared,&causes,&[(CH.into(),first.seq)])));
    let downstream=post(&mut s,Sender::Session("w-a2".into()),"downstream output");
    let downstream_f=frontier(&s,&downstream);
    let propagated=downstream_f.components().iter().find(|v|v.chain_id()==f.components()[0].chain_id()).unwrap();
    assert_eq!(propagated.hop(),1);
}

#[test]
fn new_work_applied_then_old_success_cannot_replace_new_basis() {
    let (mut s,_) = fixture();let c=claim(&mut s,READER,1);let success=witness(&c);
    let old_child=observed_post(&mut s,&c,Some(&success),"old eligible child");
    ok(&mut s,|tx| crate::sessions::note_progress(tx,READER,ProgressKind::WorkItem));
    let new_child=post(&mut s,Sender::Session(READER.into()),"new work applied before old DB ack");
    let new_basis=frontier(&s,&new_child);
    ack(&mut s,&c,&success);
    assert!(matches!(read_message(s.conn(),&old_child).unwrap(),MessageCause::Resolved(_)),"old eligible child may settle");
    let late=observed_post(&mut s,&c,Some(&success),"late old receipt post");
    assert_eq!(frontier(&s,&late),new_basis,"late old success cannot overwrite newer applied work");
}

#[test]
fn same_millisecond_work_epochs_and_sessions_have_distinct_qualified_resets() {
    let (mut s,_) = fixture();
    ok(&mut s,|tx| {crate::sessions::note_progress(tx,READER,ProgressKind::ToolCall)?;crate::sessions::note_progress(tx,READER,ProgressKind::ToolCall)});
    let a=post(&mut s,Sender::Session(READER.into()),"two work epochs same ms");
    assert_eq!(state(s.conn(),READER).unwrap().epoch,2);
    ok(&mut s,|tx| crate::sessions::note_progress(tx,"w-a2",ProgressKind::WorkItem));
    let b=post(&mut s,Sender::Session("w-a2".into()),"other worker epoch");
    assert_ne!(frontier(&s,&a).components()[0].chain_id(),frontier(&s,&b).components()[0].chain_id());
    assert_eq!(frontier(&s,&a).components()[0].hop(),0);
    assert_eq!(frontier(&s,&b).components()[0].hop(),0);
}

#[test]
fn passive_post_does_not_reset_and_fan_in_uses_per_key_max() {
    let (mut s,_) = fixture();let c=claim(&mut s,READER,1);ack(&mut s,&c,&witness(&c));
    let a=post(&mut s,Sender::Session(READER.into()),"passive one");
    let b=post(&mut s,Sender::Session(READER.into()),"passive two");
    assert_eq!(frontier(&s,&a),frontier(&s,&b));assert_eq!(state(s.conn(),READER).unwrap().epoch,0);
    let fa=frontier(&s,&a);let mut higher=fa.clone();higher.0[0].hop=9;
    let merged=Frontier::merge([fa,higher]);assert_eq!(merged.components()[0].hop(),9);
}

#[test]
fn snapshot_select_is_captured_prefix_only_and_work_epoch_change_is_stale() {
    let (mut s,input)=fixture();let other=post(&mut s,Sender::Human,"second actual prefix item");
    let snap=cr::snapshot(s.conn(),READER,&[CH.into()],NOW).unwrap();
    let full=capture(s.conn(),&snap,&[(CH.into(),other.seq)]).unwrap();
    let selected=full.select(&[(CH.into(),input.seq)]).unwrap();
    assert_eq!(selected.0.tuples.len(),1);
    assert!(full.select(&[(CH.into(),other.seq+1)]).is_err());
    assert!(full.select(&[("not-captured".into(),input.seq)]).is_err());
    ok(&mut s,|tx| crate::sessions::note_progress(tx,READER,ProgressKind::ToolCall));
    let windows=snap.windows_for(&[(CH.into(),input.seq)]).unwrap();
    assert_eq!(ok(&mut s,|tx| cr::claim_output(tx,&snap,&selected,&windows,&[],&[],cr::Owner::new(1)?)),cr::Decision::Stale);
}

#[test]
fn actual_causal_insert_abort_rolls_back_post_cursor_progress_and_meta() {
    let (mut s,_) = fixture();let before=durable(&s);
    let count:i64=s.conn().query_row("SELECT COUNT(*) FROM messages",[],|r|r.get(0)).unwrap();
    let cursor=crate::cursors::cursor(s.conn(),READER,CH).unwrap();
    let progress=crate::sessions::get(s.conn(),READER).unwrap().last_progress_at;
    s.conn().execute_batch("CREATE TEMP TRIGGER causal_stamp_abort BEFORE INSERT ON meta WHEN substr(NEW.key,1,18)='causal.message.v1.' BEGIN SELECT RAISE(ABORT,'synthetic causal stamp abort'); END;").unwrap();
    let error=crate::run(&mut s,NOW,|tx| {crate::sessions::note_progress(tx,READER,ProgressKind::ToolCall)?;messages::post(tx,Sender::Session(READER.into()),messages::NewPost{channel_id:CH.into(),text:"must fully roll back".into(),..Default::default()})}).unwrap_err();
    assert!(matches!(error,CollabError::Store(StoreError::Sql(rusqlite::Error::SqliteFailure(code,Some(ref text)))) if code.extended_code==rusqlite::ffi::SQLITE_CONSTRAINT_TRIGGER && text=="synthetic causal stamp abort"),"real INSERT trigger must fire");
    assert_eq!(durable(&s),before);assert_eq!(s.conn().query_row("SELECT COUNT(*) FROM messages",[],|r|r.get::<_,i64>(0)).unwrap(),count);
    assert_eq!(crate::cursors::cursor(s.conn(),READER,CH).unwrap(),cursor);
    assert_eq!(crate::sessions::get(s.conn(),READER).unwrap().last_progress_at,progress);
}

#[test]
fn known_success_finalize_abort_is_atomic_and_same_fate_retry_is_once() {
    let (mut s,_) = fixture();let c=claim(&mut s,READER,1);let success=witness(&c);
    let before=durable(&s);let cursor=crate::cursors::cursor(s.conn(),READER,CH).unwrap();
    s.conn().execute_batch("CREATE TEMP TRIGGER causal_output_abort BEFORE UPDATE ON meta WHEN substr(NEW.key,1,17)='causal.output.v1.' BEGIN SELECT RAISE(ABORT,'synthetic causal output abort'); END;").unwrap();
    let error=crate::run(&mut s,NOW,|tx| cr::ack_output_delivered(tx,&c,&success,crate::wakes::Riders::Release)).unwrap_err();
    assert!(matches!(error,CollabError::Store(StoreError::Sql(rusqlite::Error::SqliteFailure(code,Some(ref text)))) if code.extended_code==rusqlite::ffi::SQLITE_CONSTRAINT_TRIGGER && text=="synthetic causal output abort"));
    assert_eq!(durable(&s),before);assert_eq!(crate::cursors::cursor(s.conn(),READER,CH).unwrap(),cursor);
    s.conn().execute_batch("DROP TRIGGER causal_output_abort").unwrap();
    assert!(ack(&mut s,&c,&success).newly_committed);assert!(!ack(&mut s,&c,&success).newly_committed);
}

#[test]
fn strict_present_corrupt_chain_and_message_are_errors_not_absence() {
    let (s,input)=fixture();
    let key=format!("{MESSAGE}{}",input.id);let original=crewd_store::meta_get(s.conn(),&key).unwrap().unwrap();
    for raw in ["{broken".into(),original.replace("\"version\":1","\"version\":2"),original.replace("\"kind\":\"message\"","\"kind\":\"unsupported\"")] {
        crewd_store::meta_set(s.conn(),&key,&raw).unwrap();assert!(read_message(s.conn(),&input).is_err());
    }
    crewd_store::meta_set(s.conn(),&key,&original).unwrap();
    let chain=frontier(&s,&input).components()[0].chain_id();assert!(validate_chain_id(&chain).is_ok());
    assert!(validate_chain_id("c1:00").is_err());assert!(validate_chain_id(&(chain.clone()+"00")).is_err());assert!(validate_chain_id(&chain.to_uppercase()).is_err());
}

#[test]
fn hold_recovery_persists_exact_episode_and_retained_message_overlay() {
    let (mut s,input)=fixture();let c=claim(&mut s,READER,1);ack(&mut s,&c,&witness(&c));
    let m=post(&mut s,Sender::Session(READER.into()),"held retained downstream body");
    let f=frontier(&s,&m);let chain=f.components()[0].chain_id();
    let h=ChainHold{chain_id:chain.clone(),from:READER.into(),target:"w-a2".into(),hop:1,reason:"synthetic budget".into(),at_ms:NOW};
    assert!(ok(&mut s,|tx|hold(tx,&h)));assert!(!ok(&mut s,|tx|hold(tx,&h)));
    ok(&mut s,|tx|crate::sessions::note_progress(tx,"w-a2",ProgressKind::ToolCall));
    assert_eq!(holds(s.conn(),Some("w-a2")).unwrap(),vec![h.clone()],"work never auto unlatches");
    let action=HumanRecoveryAction::new();assert!(ok(&mut s,|tx|recover(tx,&chain,READER,"w-a2",&action)));
    assert!(!ok(&mut s,|tx|recover(tx,&chain,READER,"w-a2",&action)));
    let snap=cr::snapshot(s.conn(),"w-a2",&[CH.into()],NOW).unwrap();let captured=capture(s.conn(),&snap,&[(CH.into(),m.seq)]).unwrap();
    let overlay=captured.frontier().components().iter().find(|v|v.key.reset==ResetIdentity::Recovery{action:action.stable_key().into()}).unwrap();
    assert_eq!(overlay.hop(),0);assert!(holds(s.conn(),Some("w-a2")).unwrap().is_empty());
    assert_eq!(frontier(&s,&input).components()[0].hop(),0,"recovery does not rewrite immutable input");
}

#[test]
fn durable_success_and_hold_survive_database_reopen_without_new_epoch() {
    let (mut s,_)=fixture();let c=claim(&mut s,READER,1);ack(&mut s,&c,&witness(&c));
    let m=post(&mut s,Sender::Session(READER.into()),"durable after-success child");let f=frontier(&s,&m);
    let h=ChainHold{chain_id:f.components()[0].chain_id(),from:READER.into(),target:"w-a2".into(),hop:1,reason:"synthetic persisted hold".into(),at_ms:NOW};ok(&mut s,|tx|hold(tx,&h));
    let dir=tempfile::tempdir().unwrap();let path=dir.path().join("synthetic-causal.sqlite");
    s.conn().execute("VACUUM INTO ?1",params![path.to_str().unwrap()]).unwrap();drop(s);
    let reopened=Store::open(&path).unwrap();assert_eq!(frontier(&reopened,&m),f);assert_eq!(holds(reopened.conn(),None).unwrap(),vec![h]);assert_eq!(state(reopened.conn(),READER).unwrap().epoch,0);
}

#[test]
fn empty_hook_has_no_output_and_nonempty_hook_has_exact_same_tx_cause() {
    let (mut s,input)=fixture();let snap=cr::snapshot(s.conn(),READER,&[CH.into()],NOW).unwrap();
    let causes=capture(s.conn(),&snap,&[(CH.into(),input.seq)]).unwrap();
    let empty=causes.select(&[]).unwrap();let result=applied(ok(&mut s,|tx|cr::commit_hook_output(tx,&snap,&empty,&[])));assert!(result.output.is_none());
    let snap=cr::snapshot(s.conn(),READER,&[CH.into()],NOW).unwrap();let causes=capture(s.conn(),&snap,&[(CH.into(),input.seq)]).unwrap();
    let result=applied(ok(&mut s,|tx|cr::commit_hook_output(tx,&snap,&causes,&[(CH.into(),input.seq)])));assert!(result.output.is_some());
    let m=post(&mut s,Sender::Session(READER.into()),"after real nonempty hook commit");assert_eq!(frontier(&s,&m).components()[0].hop(),1);
}


fn effective_tuple_frontier(s: &Store, target: &str, m: &Message) -> Frontier {
    let snapshot=cr::snapshot(s.conn(),target,std::slice::from_ref(&m.channel_id),NOW).unwrap();
    let captured=capture(s.conn(),&snapshot,&[(m.channel_id.clone(),m.seq)]).unwrap();
    let index=captured.0.tuples.iter().position(|t|t.id==m.id).expect("same actual retained message must remain in prefix");
    captured.0.parts[index].clone()
}

fn two_exact_recoveries(first_id: &str, second_id: &str, reopen: bool) {
    let (mut s,_)=fixture();let c=claim(&mut s,READER,1);ack(&mut s,&c,&witness(&c));
    let m=post(&mut s,Sender::Session(READER.into()),"same immutable retained body recovered twice");
    let original_frontier=frontier(&s,&m);
    let original_stamp=crewd_store::meta_get(s.conn(),&format!("{MESSAGE}{}",m.id)).unwrap().unwrap();
    let original_message=messages::get(s.conn(),&m.id).unwrap();
    let first=HumanRecoveryAction(first_id.into());let second=HumanRecoveryAction(second_id.into());
    assert_ne!(first,second);
    let initial_chain=original_frontier.components()[0].chain_id();
    let initial_hold=ChainHold{chain_id:initial_chain.clone(),from:READER.into(),target:"w-a2".into(),hop:1,reason:"first actual retained episode".into(),at_ms:NOW};
    assert!(ok(&mut s,|tx|hold(tx,&initial_hold)));
    assert!(ok(&mut s,|tx|recover(tx,&initial_chain,READER,"w-a2",&first)));
    let after_first=effective_tuple_frontier(&s,"w-a2",&m);
    assert_eq!(after_first.components()[0].key.reset,ResetIdentity::Recovery{action:first_id.into()},"first exact recovery must act on same tuple");
    assert_eq!(after_first.components()[0].hop(),0);
    let second_chain=after_first.components()[0].chain_id();
    let second_hold=ChainHold{chain_id:second_chain.clone(),from:READER.into(),target:"w-a2".into(),hop:17,reason:"second exact current-frontier episode".into(),at_ms:NOW};
    assert!(ok(&mut s,|tx|hold(tx,&second_hold)));
    assert!(ok(&mut s,|tx|recover(tx,&second_chain,READER,"w-a2",&second)));
    let after_second=effective_tuple_frontier(&s,"w-a2",&m);
    assert_eq!(after_second.components()[0].key.reset,ResetIdentity::Recovery{action:second_id.into()},"F3 second exact recovery must select effective retained tuple and supersede first action irrespective of UUID ordering");
    assert_eq!(after_second.components()[0].hop(),0);
    assert!(!ok(&mut s,|tx|recover(tx,&second_chain,READER,"w-a2",&second)),"same action retry is not a new reset");
    assert_eq!(effective_tuple_frontier(&s,"w-a2",&m),after_second,"rescan is stable");
    assert_eq!(frontier(&s,&m),original_frontier,"receiver overlay cannot mutate source history");
    assert_eq!(messages::get(s.conn(),&m.id).unwrap(),original_message);
    assert_eq!(crewd_store::meta_get(s.conn(),&format!("{MESSAGE}{}",m.id)).unwrap().unwrap(),original_stamp);
    assert!(holds(s.conn(),Some("w-a2")).unwrap().is_empty());
    if reopen {
        let dir=tempfile::tempdir().unwrap();let path=dir.path().join("synthetic-two-recoveries.sqlite");
        s.conn().execute("VACUUM INTO ?1",params![path.to_str().unwrap()]).unwrap();drop(s);
        let reopened=Store::open(&path).unwrap();
        assert_eq!(effective_tuple_frontier(&reopened,"w-a2",&m),after_second,"reopen must preserve actual recovery order, not UUID sort");
        assert_eq!(frontier(&reopened,&m),original_frontier);
        assert_eq!(messages::get(reopened.conn(),&m.id).unwrap(),original_message);
        assert_eq!(crewd_store::meta_get(reopened.conn(),&format!("{MESSAGE}{}",m.id)).unwrap().unwrap(),original_stamp);
        assert!(holds(reopened.conn(),Some("w-a2")).unwrap().is_empty());
    }
}

#[test]
fn f3_same_tuple_two_exact_recoveries_use_effective_frontier_and_preserve_history() {
    // Actual actions in chronological ascending UUID order; UUID values are synthetic fixture identities.
    two_exact_recoveries("11111111-1111-4111-8111-111111111111","eeeeeeee-eeee-4eee-8eee-eeeeeeeeeeee",false);
}

#[test]
fn f3_reverse_uuid_recovery_order_survives_reopen_and_rescan() {
    // Chronologically later human action sorts first as text. Sorting UUID keys is not transaction order.
    two_exact_recoveries("eeeeeeee-eeee-4eee-8eee-eeeeeeeeeeee","11111111-1111-4111-8111-111111111111",true);
}


#[test]
fn empty_timer_success_settles_exact_row_without_erasing_previous_message_basis() {
    let (mut s,_)=fixture();let c=claim(&mut s,READER,1);ack(&mut s,&c,&witness(&c));let m=post(&mut s,Sender::Session(READER.into()),"previous committed message output");let basis=frontier(&s,&m);
    let id=match ok(&mut s,|tx|crate::wakes::enqueue(tx,crate::wakes::WakeRequest{session:READER.into(),kind:crate::wakes::WakeKind::Timer,source:crate::wakes::source::timer("synthetic-empty-body"),priority:crate::wakes::WakePriority::System,not_before_ms:None,note:None,origin:None})){crate::wakes::Enqueued::New(id)=>id,x=>panic!("actual timer setup {x:?}")};
    let snap=cr::snapshot(s.conn(),READER,&[],NOW).unwrap();let causes=capture(s.conn(),&snap,&[]).unwrap();
    let timer=applied(ok(&mut s,|tx|cr::claim_output(tx,&snap,&causes,&[],&[id],&[],cr::Owner::new(2)?)));
    let success=witness(&timer);ack(&mut s,&timer,&success);assert_eq!(crate::wakes::by_ids(s.conn(),&[id]).unwrap()[0].state,crate::wakes::WakeState::Delivered);
    assert_eq!(state(s.conn(),READER).unwrap().basis,basis,"empty actual-message receipt must not erase committed message ancestry");
    let after=observed_post(&mut s,&timer,Some(&success),"post after empty timer success");assert_eq!(frontier(&s,&after),basis,"empty success is not a new message parent");
}

#[test]
fn first_empty_timer_true_success_has_stable_autonomous_event_basis() {
    let mut s = store();
    ok(&mut s, |tx| initialize(tx, READER));
    let id = match ok(&mut s, |tx| crate::wakes::enqueue(tx, crate::wakes::WakeRequest {
        session: READER.into(), kind: crate::wakes::WakeKind::Timer,
        source: crate::wakes::source::timer("synthetic-first-autonomous-timer"),
        priority: crate::wakes::WakePriority::System, not_before_ms: None,
        note: None, origin: None,
    })) {
        crate::wakes::Enqueued::New(id) => id,
        other => panic!("actual Timer setup failed {other:?}"),
    };
    let snap = cr::snapshot(s.conn(), READER, &[], NOW).unwrap();
    let causes = capture(s.conn(), &snap, &[]).unwrap();
    let claimed = applied(ok(&mut s, |tx| cr::claim_output(
        tx, &snap, &causes, &[], &[id], &[], cr::Owner::new(1)?,
    )));
    assert_eq!(crate::wakes::by_ids(s.conn(), &[id]).unwrap()[0].state,
        crate::wakes::WakeState::Inflight);
    let early = observed_post(&mut s, &claimed, None, "before true Timer success");
    assert!(matches!(read_message(s.conn(), &early).unwrap(), MessageCause::Unresolved { .. }));
    let success = witness(&claimed);
    ack(&mut s, &claimed, &success);
    assert_eq!(crate::wakes::by_ids(s.conn(), &[id]).unwrap()[0].state,
        crate::wakes::WakeState::Delivered);
    let first = observed_post(&mut s, &claimed, Some(&success), "first autonomous Timer output");
    let actual = read_message(s.conn(), &first).unwrap();
    assert!(matches!(&actual, MessageCause::Resolved(_)),
        "first autonomous empty Timer exact success must establish stable explicit event basis");
    let MessageCause::Resolved(event) = actual else { unreachable!("asserted resolved") };
    assert_eq!(event.components().len(), 1);
    assert_eq!(event.components()[0].hop(), 0);
    let repeated = observed_post(&mut s, &claimed, Some(&success), "same autonomous event again");
    assert_eq!(frontier(&s, &repeated), event);
    assert!(!ack(&mut s, &claimed, &success).newly_committed);
    assert!(matches!(read_message(s.conn(), &early).unwrap(), MessageCause::Unresolved { .. }),
        "later success may not retroactively activate the pre-success child");
}

#[test]
fn empty_timer_success_does_not_consume_pending_actual_work_before_real_post() {
    let mut s = store();
    ok(&mut s, |tx| initialize(tx, READER));
    ok(&mut s, |tx| crate::sessions::note_progress(tx, READER, ProgressKind::WorkItem));
    let before = state(s.conn(), READER).unwrap();
    assert_eq!(before.epoch, 1);
    assert_eq!(before.applied, 0);
    assert!(before.basis.components().is_empty());
    let id = match ok(&mut s, |tx| crate::wakes::enqueue(tx, crate::wakes::WakeRequest {
        session: READER.into(), kind: crate::wakes::WakeKind::Timer,
        source: crate::wakes::source::timer("synthetic-pending-work-empty-timer"),
        priority: crate::wakes::WakePriority::System, not_before_ms: None,
        note: None, origin: None,
    })) { crate::wakes::Enqueued::New(id) => id, x => panic!("actual Timer setup {x:?}") };
    let snap = cr::snapshot(s.conn(), READER, &[], NOW).unwrap();
    let causes = capture(s.conn(), &snap, &[]).unwrap();
    let timer = applied(ok(&mut s, |tx| cr::claim_output(tx, &snap, &causes, &[], &[id], &[], cr::Owner::new(1)?)));
    let success = witness(&timer);
    ack(&mut s, &timer, &success);
    assert_eq!(crate::wakes::by_ids(s.conn(), &[id]).unwrap()[0].state, crate::wakes::WakeState::Delivered);
    let settled = state(s.conn(), READER).unwrap();
    assert_eq!(settled.epoch, before.epoch);
    assert_eq!(settled.applied, before.applied, "empty Timer success must not consume pending genuine Work reset");
    assert_eq!(settled.basis, before.basis);
    assert_eq!(settled.last_output, before.last_output);
    let first = observed_post(&mut s, &timer, Some(&success), "first actual pending Work output");
    let basis = frontier(&s, &first);
    assert_eq!(basis.components().len(), 1);
    assert_eq!(basis.components()[0].key.root, Root::Work { session: READER.into(), epoch: 1 });
    assert_eq!(basis.components()[0].hop(), 0);
    assert_eq!(state(s.conn(), READER).unwrap().applied, 1);
    let repeated = observed_post(&mut s, &timer, Some(&success), "same actual Work output repeat");
    assert_eq!(frontier(&s, &repeated), basis);
    assert_eq!(state(s.conn(), READER).unwrap().epoch, 1);
    assert!(!ack(&mut s, &timer, &success).newly_committed);
}

// OWN proposal only. Appended inside existing causal::tests; no module declaration changes.
fn lifecycle_observed_post(s: &mut Store, c: &ClaimedOutput, success: &ObservedSuccess, channel: &str, text: &str) -> Message {
    let producer = match &success.0.kind { SuccessKind::Executor { producer, .. } => producer.clone(), _ => panic!("executor fixture") };
    let observation = PostObservation::new(&producer, 2, Some(c.pending.clone()), Some(success.clone())).unwrap();
    let context = TxContext::new(Arc::new(FixedObservation { session: c.pending.session().into(), value: observation }));
    crate::run_with_context(s, NOW, context, |tx| messages::post(tx, Sender::Session(c.pending.session().into()), messages::NewPost { channel_id: channel.into(), text: text.into(), ..Default::default() })).unwrap().0.message
}
fn lifecycle_event_claim(s: &mut Store, session: &str, kind: crate::wakes::WakeKind, source: &str, generation: u64) -> (ClaimedOutput, i64) {
    let id = match ok(s, |tx| crate::wakes::enqueue(tx, crate::wakes::WakeRequest { session: session.into(), kind, source: source.into(), priority: crate::wakes::WakePriority::System, not_before_ms: None, note: None, origin: None })) {
        crate::wakes::Enqueued::New(id) => id, x => panic!("actual event enqueue {x:?}"),
    };
    let snap = cr::snapshot(s.conn(), session, &[], NOW).unwrap();
    let cause = capture(s.conn(), &snap, &[]).unwrap();
    let c = applied(ok(s, |tx| cr::claim_output(tx, &snap, &cause, &[], &[id], &[], cr::Owner::new(generation)?)));
    assert_eq!(crate::wakes::by_ids(s.conn(), &[id]).unwrap()[0].state, crate::wakes::WakeState::Inflight);
    (c, id)
}
fn lifecycle_db(s: &Store) -> Vec<(String, Vec<Vec<rusqlite::types::Value>>)> {
    let mut q = s.conn().prepare("SELECT name FROM sqlite_master WHERE type='table' ORDER BY name").unwrap();
    let names = q.query_map([], |r| r.get::<_, String>(0)).unwrap().collect::<rusqlite::Result<Vec<_>>>().unwrap();
    names.into_iter().map(|name| {
        let quoted = name.replace('"', "\"\"");
        let mut rows = s.conn().prepare(&format!("SELECT * FROM \"{quoted}\" ORDER BY rowid")).unwrap();
        let columns = rows.column_count();
        let values = rows.query_map([], |r| (0..columns).map(|i| r.get::<_, rusqlite::types::Value>(i)).collect::<rusqlite::Result<Vec<_>>>()).unwrap().collect::<rusqlite::Result<Vec<_>>>().unwrap();
        (name, values)
    }).collect()
}
struct LifecycleHistory {
    doomed: Message, foreign: Message, descendant: Message,
    referenced: ClaimedOutput, orphan: ClaimedOutput,
}
fn lifecycle_history_with_foreign(direct_foreign: bool) -> (Store, LifecycleHistory) {
    let mut s = store();
    ok(&mut s, |tx| initialize(tx, "cap-x"));
    let (doomed_dm, _) = ok(&mut s, |tx| crate::channels::ensure_dm(tx, "crew-x", &crewd_proto::MemberRef::session("cap-x"), &crewd_proto::MemberRef::session("w-a1")));
    let (foreign_dm, _) = ok(&mut s, |tx| crate::channels::ensure_dm(tx, "crew-a", &crewd_proto::MemberRef::session("w-a2"), &crewd_proto::MemberRef::session("cap-x")));
    let (downstream_dm, _) = ok(&mut s, |tx| crate::channels::ensure_dm(tx, "crew-a", &crewd_proto::MemberRef::session("w-a1"), &crewd_proto::MemberRef::session("w-a2")));
    assert_eq!(doomed_dm.crew_id, "crew-x"); assert_eq!(foreign_dm.crew_id, "crew-a");
    assert_ne!(doomed_dm.id, foreign_dm.id); assert_ne!(doomed_dm.id, downstream_dm.id);
    let (referenced, _) = lifecycle_event_claim(&mut s, "cap-x", crate::wakes::WakeKind::Timer, "timer:original-purge-event", 1);
    let success = witness(&referenced); ack(&mut s, &referenced, &success);
    let doomed = lifecycle_observed_post(&mut s, &referenced, &success, &doomed_dm.id, "source-owned actual event body");
    let foreign = if direct_foreign { lifecycle_observed_post(&mut s, &referenced, &success, &foreign_dm.id, "surviving foreign history with gone sender") } else { doomed.clone() };
    let snap = cr::snapshot(s.conn(), "w-a1", std::slice::from_ref(&doomed_dm.id), NOW).unwrap();
    let cause = capture(s.conn(), &snap, &[(doomed_dm.id.clone(), doomed.seq)]).unwrap();
    applied(ok(&mut s, |tx| cr::commit_hook_output(tx, &snap, &cause, &[(doomed_dm.id.clone(), doomed.seq)])));
    let descendant = ok(&mut s, |tx| messages::post(tx, Sender::Session("w-a1".into()), messages::NewPost { channel_id: downstream_dm.id, text: "Root Event retained without original parent stamp candidate".into(), ..Default::default() })).message;
    let ds: Stamp = read(s.conn(), &format!("{MESSAGE}{}", descendant.id), "message").unwrap().unwrap();
    assert!(ds.candidate.is_none(), "descendant proves event-root reference independently of candidate field");
    assert!(frontier(&s, &descendant).components().iter().any(|v| matches!(&v.key.root, Root::Event { output, .. } if output == referenced.pending.stable_key())));
    let (orphan, _) = lifecycle_event_claim(&mut s, "cap-x", crate::wakes::WakeKind::System, "system:unreferenced-completed-event", 2);
    ack(&mut s, &orphan, &witness(&orphan));
    ok(&mut s, |tx| crate::crews::archive(tx, "crew-x"));
    (s, LifecycleHistory { doomed, foreign, descendant, referenced, orphan })
}
fn lifecycle_history() -> (Store, LifecycleHistory) { lifecycle_history_with_foreign(true) }
#[test]
fn purge_causal_foreign_history_and_event_reference_survive_source_crew() {
    let (mut s, h) = lifecycle_history();
    let foreign_stamp = crewd_store::meta_get(s.conn(), &format!("{MESSAGE}{}", h.foreign.id)).unwrap();
    let descendant_stamp = crewd_store::meta_get(s.conn(), &format!("{MESSAGE}{}", h.descendant.id)).unwrap();
    let foreign_basis = frontier(&s, &h.foreign); let descendant_basis = frontier(&s, &h.descendant);
    let event_record = crewd_store::meta_get(s.conn(), &format!("{OUTPUT}{}", h.referenced.pending.stable_key())).unwrap();
    ok(&mut s, |tx| crate::crews::purge(tx, "crew-x"));
    assert!(crewd_store::sessions::get(s.conn(), "cap-x").unwrap().is_none());
    assert!(crewd_store::messages::get(s.conn(), &h.doomed.id).unwrap().is_none());
    assert!(crewd_store::meta_get(s.conn(), &format!("{MESSAGE}{}", h.doomed.id)).unwrap().is_none());
    assert_eq!(messages::get(s.conn(), &h.foreign.id).unwrap(), h.foreign);
    assert_eq!(crewd_store::meta_get(s.conn(), &format!("{MESSAGE}{}", h.foreign.id)).unwrap(), foreign_stamp);
    assert_eq!(crewd_store::meta_get(s.conn(), &format!("{MESSAGE}{}", h.descendant.id)).unwrap(), descendant_stamp);
    assert_eq!(frontier(&s, &h.foreign), foreign_basis);
    assert_eq!(frontier(&s, &h.descendant), descendant_basis);
    assert_eq!(crewd_store::meta_get(s.conn(), &format!("{OUTPUT}{}", h.referenced.pending.stable_key())).unwrap(), event_record, "retained Root Event cannot lose its original actual output record");
    let dir = tempfile::tempdir().unwrap(); let path = dir.path().join("causal-foreign-event.sqlite");
    s.conn().execute("VACUUM INTO ?1", params![path.to_str().unwrap()]).unwrap(); drop(s);
    let reopened = Store::open(&path).unwrap();
    assert_eq!(frontier(&reopened, &h.foreign), foreign_basis); assert_eq!(frontier(&reopened, &h.descendant), descendant_basis);
}
#[test]
fn purge_prunes_proven_unreferenced_completed_output_only() {
    let (mut s, h) = lifecycle_history();
    let live = format!("{OUTPUT}{}", h.referenced.pending.stable_key());
    let dead = format!("{OUTPUT}{}", h.orphan.pending.stable_key());
    assert!(crewd_store::meta_get(s.conn(), &dead).unwrap().is_some());
    crewd_store::meta_set(s.conn(), "causalXoutput.v1.keep", "unrelated exact-prefix control").unwrap();
    ok(&mut s, |tx| crate::crews::purge(tx, "crew-x"));
    assert!(crewd_store::meta_get(s.conn(), &live).unwrap().is_some());
    assert!(crewd_store::meta_get(s.conn(), &dead).unwrap().is_none(), "S42 purge must prune completed original output proven unreachable after actual cascade");
    assert_eq!(crewd_store::meta_get(s.conn(), "causalXoutput.v1.keep").unwrap().as_deref(), Some("unrelated exact-prefix control"));
}
#[test]
fn purge_causal_actual_orphan_stamp_delete_abort_rolls_back_whole_cascade() {
    let (mut s, h) = lifecycle_history(); let key = format!("{MESSAGE}{}", h.doomed.id);
    s.conn().execute_batch("CREATE TEMP TABLE synthetic_causal_delete_key(key TEXT PRIMARY KEY); CREATE TEMP TRIGGER synthetic_causal_delete BEFORE DELETE ON meta WHEN OLD.key IN (SELECT key FROM synthetic_causal_delete_key) BEGIN SELECT RAISE(ABORT,'synthetic causal cascade delete abort'); END;").unwrap();
    s.conn().execute("INSERT INTO synthetic_causal_delete_key VALUES(?1)", [&key]).unwrap();
    let before = lifecycle_db(&s);
    let error = crate::run(&mut s, NOW, |tx| crate::crews::purge(tx, "crew-x")).unwrap_err();
    assert!(matches!(error, CollabError::Store(StoreError::Sql(rusqlite::Error::SqliteFailure(code, Some(ref msg)))) if code.extended_code == rusqlite::ffi::SQLITE_CONSTRAINT_TRIGGER && msg == "synthetic causal cascade delete abort"), "actual orphan marker DELETE must reach trigger");
    assert_eq!(lifecycle_db(&s), before, "failed cleanup rolls back rows, cursors, attempts, ownership, meta and cascade together");
}
#[test]
fn purge_causal_unreachable_output_delete_abort_is_reachable_and_atomic() {
    let (mut s, h) = lifecycle_history(); let key = format!("{OUTPUT}{}", h.orphan.pending.stable_key());
    s.conn().execute_batch("CREATE TEMP TABLE synthetic_causal_delete_key(key TEXT PRIMARY KEY); CREATE TEMP TRIGGER synthetic_causal_delete BEFORE DELETE ON meta WHEN OLD.key IN (SELECT key FROM synthetic_causal_delete_key) BEGIN SELECT RAISE(ABORT,'synthetic unreachable output delete abort'); END;").unwrap();
    s.conn().execute("INSERT INTO synthetic_causal_delete_key VALUES(?1)", [&key]).unwrap(); let before = lifecycle_db(&s);
    let result = crate::run(&mut s, NOW, |tx| crate::crews::purge(tx, "crew-x"));
    assert!(result.is_err(), "S42 unreferenced output DELETE must actually reach SQL abort rather than retain orphan forever");
    let error = result.unwrap_err();
    assert!(matches!(error, CollabError::Store(StoreError::Sql(rusqlite::Error::SqliteFailure(code, Some(ref msg)))) if code.extended_code == rusqlite::ffi::SQLITE_CONSTRAINT_TRIGGER && msg == "synthetic unreachable output delete abort"));
    assert_eq!(lifecycle_db(&s), before);
}
#[test]
fn purge_preserves_foreign_recovery_overlay_hold_and_legacy_history() {
    let (mut s, h) = lifecycle_history();
    let original = frontier(&s, &h.descendant); let x = original.components()[0].chain_id();
    let old_hold = ChainHold { chain_id: x.clone(), from: "w-a1".into(), target: "w-a2".into(), hop: 1, reason: "actual retained foreign descendant".into(), at_ms: NOW };
    ok(&mut s, |tx| hold(tx, &old_hold)); let action = HumanRecoveryAction::new();
    assert!(ok(&mut s, |tx| recover(tx, &x, "w-a1", "w-a2", &action)));
    let overlay = effective_tuple_frontier(&s, "w-a2", &h.descendant);
    let current_hold = ChainHold { chain_id: overlay.components()[0].chain_id(), from: "w-a1".into(), target: "w-a2".into(), hop: 0, reason: "new actual foreign episode remains human-owned".into(), at_ms: NOW };
    ok(&mut s, |tx| hold(tx, &current_hold));
    // Real low-level Store legacy-import boundary: no private causal stamp and no trusted userextra.
    let mut legacy = h.foreign.clone(); legacy.id = uuid::Uuid::new_v4().to_string(); legacy.seq = 0; legacy.text = "actual imported foreign history without marker".into();
    crewd_store::messages::insert(s.conn(), &legacy, Some(&serde_json::json!({"causal":"untrusted legacy extra"}))).unwrap();
    let legacy = messages::get(s.conn(), &legacy.id).unwrap(); let legacy_basis = frontier(&s, &legacy);
    assert!(crewd_store::meta_get(s.conn(), &format!("{MESSAGE}{}", legacy.id)).unwrap().is_none());
    let recovery = crewd_store::meta_get(s.conn(), &format!("{RECOVERY}{}", action.stable_key())).unwrap();
    ok(&mut s, |tx| crate::crews::purge(tx, "crew-x"));
    assert_eq!(effective_tuple_frontier(&s, "w-a2", &h.descendant), overlay);
    assert_eq!(frontier(&s, &h.descendant), original);
    assert!(holds(s.conn(), Some("w-a2")).unwrap().contains(&current_hold));
    assert_eq!(crewd_store::meta_get(s.conn(), &format!("{RECOVERY}{}", action.stable_key())).unwrap(), recovery);
    assert_eq!(frontier(&s, &legacy), legacy_basis, "legacy absence remains immutable message root, never inferred causal ancestry");
    assert_eq!(messages::get(s.conn(), &legacy.id).unwrap(), legacy);
}
#[test]
fn genuine_pending_owner_and_foreign_zero_id_window_block_cleanup_without_mutation() {
    let mut s = store(); ok(&mut s, |tx| initialize(tx, "cap-x"));
    let (pending, id) = lifecycle_event_claim(&mut s, "cap-x", crate::wakes::WakeKind::Timer, "timer:pending-cleanup-guard", 1);
    let before = lifecycle_db(&s);
    assert!(guard_crew(s.conn(), "crew-x").is_err(), "real Pending output owner must block its source cleanup");
    assert!(crate::run(&mut s, NOW, |tx| crate::crews::archive(tx, "crew-x")).is_err());
    assert_eq!(lifecycle_db(&s), before);
    assert_eq!(crate::wakes::by_ids(s.conn(), &[id]).unwrap()[0].state, crate::wakes::WakeState::Inflight);
    ack(&mut s, &pending, &witness(&pending));
    let (dm, _) = ok(&mut s, |tx| crate::channels::ensure_dm(tx, "crew-x", &crewd_proto::MemberRef::session("cap-x"), &crewd_proto::MemberRef::session("w-a1")));
    let m = ok(&mut s, |tx| messages::post(tx, Sender::Human, messages::NewPost { channel_id: dm.id.clone(), text: "actual foreign reader zero-ID cleanup hole".into(), ..Default::default() })).message;
    let snap = cr::snapshot(s.conn(), "w-a1", std::slice::from_ref(&dm.id), NOW).unwrap();
    let causes = capture(s.conn(), &snap, &[(dm.id.clone(), m.seq)]).unwrap(); let windows = snap.windows_for(&[(dm.id.clone(), m.seq)]).unwrap();
    let foreign = applied(ok(&mut s, |tx| cr::claim_output(tx, &snap, &causes, &windows, &[], &[], cr::Owner::new(1)?)));
    let before = lifecycle_db(&s);
    assert!(crate::run(&mut s, NOW, |tx| crate::crews::archive(tx, "crew-x")).is_err());
    assert_eq!(lifecycle_db(&s), before, "foreign active zero-ID window cannot be erased by source crew archive");
    ack(&mut s, &foreign, &witness(&foreign));
    ok(&mut s, |tx| crate::crews::archive(tx, "crew-x")); ok(&mut s, |tx| crate::crews::purge(tx, "crew-x"));
}

#[test]
fn purge_retains_original_output_reachable_only_through_foreign_event_root() {
    let (mut s, h) = lifecycle_history_with_foreign(false);
    let stamp: Stamp = read(s.conn(), &format!("{MESSAGE}{}", h.descendant.id), "message").unwrap().unwrap();
    assert!(stamp.candidate.is_none());
    let before = frontier(&s, &h.descendant);
    let key = format!("{OUTPUT}{}", h.referenced.pending.stable_key());
    let actual_record = crewd_store::meta_get(s.conn(), &key).unwrap();
    ok(&mut s, |tx| crate::crews::purge(tx, "crew-x"));
    assert!(crewd_store::messages::get(s.conn(), &h.doomed.id).unwrap().is_none());
    assert_eq!(crewd_store::meta_get(s.conn(), &key).unwrap(), actual_record, "S42 foreign Root::Event alone must retain original known-success record after original parent stamp cascade");
    assert_eq!(frontier(&s, &h.descendant), before);
}
#[test]
fn retained_event_root_missing_original_record_is_fail_loud() {
    let (s, h) = lifecycle_history_with_foreign(false);
    let key = format!("{OUTPUT}{}", h.referenced.pending.stable_key());
    // Exact missing-reference fault injection, starting from a genuine persisted output/root.
    s.conn().execute("DELETE FROM meta WHERE key=?1", [&key]).unwrap();
    assert!(read_message(s.conn(), &h.descendant).is_err(), "S42 present Event root must not silently accept missing original known-success metadata");
}

// OWN proposal only. Reuses lifecycle helpers in purge group, appended in existing causal::tests.
#[test]
fn original_ask_and_system_event_roots_are_stable_distinct_and_generation_bound() {
    for (kind, source) in [(crate::wakes::WakeKind::AskAnswer, "ask:actual-answer-event"), (crate::wakes::WakeKind::System, "system:actual-system-event")] {
        let mut s = store(); ok(&mut s, |tx| initialize(tx, READER));
        let (c, id) = lifecycle_event_claim(&mut s, READER, kind, source, 23);
        assert!(c.pending.observe_success(7, 22, &uuid::Uuid::new_v4().to_string(), 1).is_err());
        let early = observed_post(&mut s, &c, None, "queued actual event before success");
        assert!(matches!(read_message(s.conn(), &early).unwrap(), MessageCause::Unresolved { .. }));
        let success = witness(&c); ack(&mut s, &c, &success);
        assert_eq!(crate::wakes::by_ids(s.conn(), &[id]).unwrap()[0].state, crate::wakes::WakeState::Delivered);
        let first = observed_post(&mut s, &c, Some(&success), "actual explicit autonomous cause");
        let basis = frontier(&s, &first); assert_eq!(basis.components().len(), 1); assert_eq!(basis.components()[0].hop(), 0);
        assert!(matches!(&basis.components()[0].key.root, Root::Event { output, session, .. } if output == c.pending.stable_key() && session == READER));
        let again = observed_post(&mut s, &c, Some(&success), "same registered actual event repeat"); assert_eq!(frontier(&s, &again), basis);
        assert!(matches!(read_message(s.conn(), &early).unwrap(), MessageCause::Unresolved { .. }));
    }
}
#[test]
fn strict_original_event_envelope_order_kind_and_session_are_not_absence() {
    let mut s = store(); ok(&mut s, |tx| initialize(tx, READER));
    let (c, _) = lifecycle_event_claim(&mut s, READER, crate::wakes::WakeKind::Timer, "timer:strict-event-parser", 1);
    let key = format!("{OUTPUT}{}", c.pending.stable_key()); let original = crewd_store::meta_get(s.conn(), &key).unwrap().unwrap();
    for raw in ["{broken".into(), original.replace("\"version\":1", "\"version\":2"), original.replace("\"kind\":\"output\"", "\"kind\":\"unknown\"")] {
        crewd_store::meta_set(s.conn(), &key, &raw).unwrap(); assert!(output(s.conn(), c.pending.stable_key()).is_err());
    }
    crewd_store::meta_set(s.conn(), &key, &original).unwrap(); let good = output(s.conn(), c.pending.stable_key()).unwrap();
    let mut invalids = Vec::new();
    let mut x = good.clone(); x.receipt.events[0].id = 0; invalids.push(x);
    let mut x = good.clone(); x.receipt.events.push(x.receipt.events[0].clone()); invalids.push(x);
    let mut x = good.clone(); x.receipt.events[0].kind = crate::wakes::WakeKind::Message; invalids.push(x);
    let mut x = good.clone(); x.receipt.events[0].source.clear(); invalids.push(x);
    let mut x = good.clone(); x.receipt.events[0].created_at.clear(); invalids.push(x);
    let mut x = good.clone(); x.receipt.session = "w-a2".into(); invalids.push(x);
    for x in invalids { put(s.conn(), &key, "output", &x).unwrap(); assert!(output(s.conn(), c.pending.stable_key()).is_err()); }
    crewd_store::meta_set(s.conn(), &key, &original).unwrap();
    let mut value: serde_json::Value = serde_json::from_str(&original).unwrap();
    // Removal must be strict parser failure, not default into a free autonomous receipt.
    value["value"]["receipt"].as_object_mut().expect("actual typed private envelope has value.receipt").remove("events");
    crewd_store::meta_set(s.conn(), &key, &value.to_string()).unwrap(); assert!(output(s.conn(), c.pending.stable_key()).is_err());
    crewd_store::meta_set(s.conn(), &key, &original).unwrap(); ack(&mut s, &c, &witness(&c));
}
#[test]
fn original_event_present_live_row_identity_mismatch_is_fail_loud() {
    let mut s = store(); ok(&mut s, |tx| initialize(tx, READER));
    let (c, id) = lifecycle_event_claim(&mut s, READER, crate::wakes::WakeKind::Timer, "timer:actual-original-row-evidence", 1);
    let key = format!("{OUTPUT}{}", c.pending.stable_key()); let original = crewd_store::meta_get(s.conn(), &key).unwrap().unwrap();
    let good = output(s.conn(), c.pending.stable_key()).unwrap();
    let foreign_id = match ok(&mut s, |tx| crate::wakes::enqueue(tx, crate::wakes::WakeRequest { session: "w-a2".into(), kind: crate::wakes::WakeKind::Timer, source: "timer:foreign-original-row".into(), priority: crate::wakes::WakePriority::System, not_before_ms: None, note: None, origin: None })) { crate::wakes::Enqueued::New(id) => id, x => panic!("foreign event {x:?}") };
    let mut variants = Vec::new();
    let mut wrong = good.clone(); wrong.receipt.events[0].source = "timer:other-valid-but-not-original-source".into(); variants.push(wrong);
    let mut wrong = good.clone(); wrong.receipt.events[0].kind = crate::wakes::WakeKind::System; variants.push(wrong);
    let mut wrong = good.clone(); wrong.receipt.events[0].created_at = crewd_store::ts_from_ms(NOW + 1); variants.push(wrong);
    let mut wrong = good.clone(); wrong.receipt.events[0].id = foreign_id; variants.push(wrong);
    for wrong in variants {
        put(s.conn(), &key, "output", &wrong).unwrap();
        assert!(output(s.conn(), c.pending.stable_key()).is_err(), "S42 present original event evidence must match the actual still-retained row");
        crewd_store::meta_set(s.conn(), &key, &original).unwrap();
    }
    assert_eq!(crate::wakes::by_ids(s.conn(), &[id]).unwrap()[0].source, "timer:actual-original-row-evidence");
    ack(&mut s, &c, &witness(&c));
}
#[test]
fn empty_no_event_brief_receipt_success_never_creates_autonomous_root() {
    let mut s = store(); ok(&mut s, |tx| initialize(tx, READER));
    let st = state(s.conn(), READER).unwrap(); let snap = cr::snapshot(s.conn(), READER, &[], NOW).unwrap(); let cause = capture(s.conn(), &snap, &[]).unwrap();
    // Existing private producer registration boundary, no actual Task/PTY claim is asserted.
    let pending = ok(&mut s, |tx| register_pending(tx, READER, &st.incarnation, &uuid::Uuid::new_v4().to_string(), 1, &cause, &[]));
    let success = pending.observe_success(0, 1, &uuid::Uuid::new_v4().to_string(), 1).unwrap();
    ok(&mut s, |tx| finalize(tx, &pending, &success));
    assert!(state(s.conn(), READER).unwrap().basis.components().is_empty());
    let producer = match &success.0.kind { SuccessKind::Executor { producer, .. } => producer.clone(), _ => unreachable!() };
    let observation = PostObservation::new(&producer, 2, Some(pending.clone()), Some(success)).unwrap();
    let ctx = TxContext::new(Arc::new(FixedObservation { session: READER.into(), value: observation }));
    let m = crate::run_with_context(&mut s, NOW, ctx, |tx| messages::post(tx, Sender::Session(READER.into()), messages::NewPost { channel_id: CH.into(), text: "no event brief output is not a free causal root".into(), ..Default::default() })).unwrap().0.message;
    assert!(matches!(read_message(s.conn(), &m).unwrap(), MessageCause::Unresolved { .. }));
}
#[test]
fn event_output_insert_abort_rolls_back_claim_attempt_and_receipt_together() {
    let mut s = store(); ok(&mut s, |tx| initialize(tx, READER));
    let id = match ok(&mut s, |tx| crate::wakes::enqueue(tx, crate::wakes::WakeRequest { session: READER.into(), kind: crate::wakes::WakeKind::Timer, source: "timer:claim-insert-abort".into(), priority: crate::wakes::WakePriority::System, not_before_ms: None, note: None, origin: None })) { crate::wakes::Enqueued::New(id) => id, x => panic!("enqueue {x:?}") };
    let snap = cr::snapshot(s.conn(), READER, &[], NOW).unwrap(); let cause = capture(s.conn(), &snap, &[]).unwrap();
    s.conn().execute_batch("CREATE TEMP TRIGGER original_event_claim_abort BEFORE INSERT ON meta WHEN substr(NEW.key,1,length('causal.output.v1.'))='causal.output.v1.' BEGIN SELECT RAISE(ABORT,'synthetic original event receipt insert abort'); END;").unwrap();
    let before = lifecycle_db(&s);
    let error = crate::run(&mut s, NOW, |tx| cr::claim_output(tx, &snap, &cause, &[], &[id], &[], cr::Owner::new(1)?)).unwrap_err();
    assert!(matches!(error, CollabError::Store(StoreError::Sql(rusqlite::Error::SqliteFailure(code, Some(ref msg)))) if code.extended_code == rusqlite::ffi::SQLITE_CONSTRAINT_TRIGGER && msg == "synthetic original event receipt insert abort"));
    assert_eq!(lifecycle_db(&s), before); assert_eq!(crate::wakes::by_ids(s.conn(), &[id]).unwrap()[0].attempts, 0);
}
#[test]
fn event_post_ordinal_overflow_is_atomic_without_extra_root_or_message() {
    let mut s = store(); ok(&mut s, |tx| initialize(tx, READER));
    let (c, _) = lifecycle_event_claim(&mut s, READER, crate::wakes::WakeKind::Timer, "timer:ordinal-overflow", 1); let success = witness(&c); ack(&mut s, &c, &success);
    // Exact numeric fault injection; constructing 2^64 real posts is not a meaningful fixture.
    let mut st = state(s.conn(), READER).unwrap(); st.ordinal = u64::MAX; ok(&mut s, |tx| save_state(tx, &st));
    let before = lifecycle_db(&s);
    let producer = match &success.0.kind { SuccessKind::Executor { producer, .. } => producer.clone(), _ => unreachable!() };
    let observation = PostObservation::new(&producer, 2, Some(c.pending.clone()), Some(success)).unwrap(); let ctx = TxContext::new(Arc::new(FixedObservation { session: READER.into(), value: observation }));
    let error = crate::run_with_context(&mut s, NOW, ctx, |tx| messages::post(tx, Sender::Session(READER.into()), messages::NewPost { channel_id: CH.into(), text: "must not survive overflow".into(), ..Default::default() })).unwrap_err();
    assert!(matches!(error, CollabError::Store(StoreError::Invalid(ref msg)) if msg.contains("overflow"))); assert_eq!(lifecycle_db(&s), before);
}
#[test]
fn original_multiple_events_have_strict_sorted_identity_and_survive_real_row_prune() {
    let mut s = store(); ok(&mut s, |tx| initialize(tx, READER)); let mut ids = Vec::new();
    for (kind, source) in [(crate::wakes::WakeKind::Timer, "timer:first-original-multi"), (crate::wakes::WakeKind::System, "system:second-original-multi")] {
        ids.push(match ok(&mut s, |tx| crate::wakes::enqueue(tx, crate::wakes::WakeRequest { session: READER.into(), kind, source: source.into(), priority: crate::wakes::WakePriority::System, not_before_ms: None, note: None, origin: None })) { crate::wakes::Enqueued::New(id) => id, x => panic!("actual event {x:?}") });
    }
    let snap = cr::snapshot(s.conn(), READER, &[], NOW).unwrap(); let cause = capture(s.conn(), &snap, &[]).unwrap();
    let c = applied(ok(&mut s, |tx| cr::claim_output(tx, &snap, &cause, &[], &ids, &[], cr::Owner::new(9)?)));
    let key = format!("{OUTPUT}{}", c.pending.stable_key()); let raw = crewd_store::meta_get(s.conn(), &key).unwrap().unwrap();
    let mut record = output(s.conn(), c.pending.stable_key()).unwrap(); assert_eq!(record.receipt.events.len(), 2);
    assert!(record.receipt.events[0].id < record.receipt.events[1].id);
    record.receipt.events.reverse(); put(s.conn(), &key, "output", &record).unwrap();
    assert!(output(s.conn(), c.pending.stable_key()).is_err(), "stored event order is strict, not silently sorted after corruption");
    crewd_store::meta_set(s.conn(), &key, &raw).unwrap(); let success = witness(&c); ack(&mut s, &c, &success);
    let before = observed_post(&mut s, &c, Some(&success), "original multi-event output"); let basis = frontier(&s, &before);
    assert_eq!(ok(&mut s, |tx| crate::wakes::prune(tx, NOW + 1, 100)), 2);
    assert!(crate::wakes::by_ids(s.conn(), &ids).unwrap().is_empty());
    let after = observed_post(&mut s, &c, Some(&success), "same exact event after legitimate original wake retention cleanup");
    assert_eq!(frontier(&s, &after), basis, "original persisted event identity survives legitimate source row prune");
    assert_eq!(output(s.conn(), c.pending.stable_key()).unwrap().receipt, c.pending.0);
    assert!(!ack(&mut s, &c, &success).newly_committed);
}

#[test]
fn autonomous_event_success_before_db_ack_preserves_exact_eligible_and_delayed_descendant() {
    let mut s = store();
    ok(&mut s, |tx| initialize(tx, READER));
    let (negative_dm, _) = ok(&mut s, |tx| crate::channels::ensure_dm(tx, "crew-a", &crewd_proto::MemberRef::session(READER), &crewd_proto::MemberRef::session("cap-a")));
    let (parent, id) = lifecycle_event_claim(&mut s, READER, crate::wakes::WakeKind::Timer, "timer:success-before-original-db-ack", 31);
    let original_id = parent.pending.stable_key().to_string();
    // Actual independent channel keeps a genuinely pre-success unresolved child out of the
    // receiver's tested CH prefix; no accepted same-channel HOL guard is bypassed.
    let queued = PostObservation::new(&uuid::Uuid::new_v4().to_string(), 1, Some(parent.pending.clone()), None).unwrap();
    let ctx = TxContext::new(Arc::new(FixedObservation { session: READER.into(), value: queued }));
    let negative = crate::run_with_context(&mut s, NOW, ctx, |tx| messages::post(tx, Sender::Session(READER.into()), messages::NewPost { channel_id: negative_dm.id.clone(), text: "true queued event before accepted success fence".into(), ..Default::default() })).unwrap().0.message;
    assert!(matches!(read_message(s.conn(), &negative).unwrap(), MessageCause::Unresolved { .. }));
    let success = witness(&parent);
    let first = observed_post(&mut s, &parent, Some(&success), "actual eligible event post before original DB ack");
    let repeated = observed_post(&mut s, &parent, Some(&success), "same actual event eligible repeat before DB ack");
    let first_key = format!("{MESSAGE}{}", first.id);
    let repeated_key = format!("{MESSAGE}{}", repeated.id);
    let first_raw = crewd_store::meta_get(s.conn(), &first_key).unwrap().unwrap();
    let repeated_raw = crewd_store::meta_get(s.conn(), &repeated_key).unwrap().unwrap();
    let stamp: Stamp = read(s.conn(), &first_key, "message").unwrap().unwrap();
    let Resolution::Eligible { output: output_id, frontier: event, success: stored_success } = &stamp.resolution else { panic!("S42 actual observed event before DBack must have exact Eligible ticket") };
    assert_eq!(output_id, &original_id);
    assert_eq!(stored_success.as_ref(), &success.0);
    assert_eq!(event.components().len(), 1); assert_eq!(event.components()[0].hop(), 0);
    assert!(matches!(&event.components()[0].key.root, Root::Event { output, .. } if output == &original_id));
    assert_eq!(output(s.conn(), &original_id).unwrap().status, OutputStatus::Pending);
    assert_eq!(crate::wakes::by_ids(s.conn(), &[id]).unwrap()[0].state, crate::wakes::WakeState::Inflight);
    let MessageCause::Unresolved { previous } = read_message(s.conn(), &first).unwrap() else { panic!("S42 Pending original ack stays honestly unresolved despite exact success ticket") };
    assert_eq!(&previous, event, "S42 Pending Event ticket must validate without inventing original DB commit");
    let MessageCause::Unresolved { previous: repeat_previous } = read_message(s.conn(), &repeated).unwrap() else { panic!("same pending event repeat remains unresolved") };
    assert_eq!(repeat_previous, previous);
    let snap = cr::snapshot(s.conn(), "w-a2", &[CH.into()], NOW).unwrap();
    let cause = capture(s.conn(), &snap, &[(CH.into(), repeated.seq)]).unwrap();
    assert!(cause.unresolved(), "S42 receiver conservative unread stays unresolved until exact original ack");
    let windows = snap.windows_for(&[(CH.into(), repeated.seq)]).unwrap();
    let before_busy = lifecycle_db(&s);
    assert_eq!(ok(&mut s, |tx| cr::claim_output(tx, &snap, &cause, &windows, &[], &[], cr::Owner::new(1)?)), cr::Decision::Busy, "S42 no receiver claim before original DBack");
    assert_eq!(lifecycle_db(&s), before_busy);
    // Real original acknowledgement SQL error. Known final success remains the same ticket;
    // no manufactured commit or Failed/requeue reinterpretation is used to unblock children.
    s.conn().execute_batch("CREATE TEMP TRIGGER preack_event_abort BEFORE UPDATE ON meta WHEN substr(NEW.key,1,length('causal.output.v1.'))='causal.output.v1.' BEGIN SELECT RAISE(ABORT,'synthetic original event DBack abort'); END;").unwrap();
    let before_retry = lifecycle_db(&s);
    let error = crate::run(&mut s, NOW, |tx| cr::ack_output_delivered(tx, &parent, &success, crate::wakes::Riders::Release)).unwrap_err();
    assert!(matches!(error, CollabError::Store(StoreError::Sql(rusqlite::Error::SqliteFailure(code, Some(ref message)))) if code.extended_code == rusqlite::ffi::SQLITE_CONSTRAINT_TRIGGER && message == "synthetic original event DBack abort"));
    assert_eq!(lifecycle_db(&s), before_retry);
    assert_eq!(output(s.conn(), &original_id).unwrap().status, OutputStatus::Pending);
    assert_eq!(crewd_store::meta_get(s.conn(), &first_key).unwrap().unwrap(), first_raw);
    assert_eq!(crewd_store::meta_get(s.conn(), &repeated_key).unwrap().unwrap(), repeated_raw);
    assert!(matches!(read_message(s.conn(), &first).unwrap(), MessageCause::Unresolved { .. }));
    s.conn().execute_batch("DROP TRIGGER preack_event_abort").unwrap();
    assert!(ack(&mut s, &parent, &success).newly_committed);
    assert!(!ack(&mut s, &parent, &success).newly_committed);
    assert_eq!(crate::wakes::by_ids(s.conn(), &[id]).unwrap()[0].state, crate::wakes::WakeState::Delivered);
    let committed_event = frontier(&s, &first);
    assert_eq!(&committed_event, event, "S42 exact same-fate retry preserves pre-ack Event identity/hop once");
    assert_eq!(frontier(&s, &repeated), committed_event);
    assert_eq!(crewd_store::meta_get(s.conn(), &first_key).unwrap().unwrap(), first_raw);
    assert_eq!(crewd_store::meta_get(s.conn(), &repeated_key).unwrap().unwrap(), repeated_raw);
    let snap = cr::snapshot(s.conn(), "w-a2", &[CH.into()], NOW).unwrap();
    let cause = capture(s.conn(), &snap, &[(CH.into(), repeated.seq)]).unwrap();
    assert!(!cause.unresolved()); assert_eq!(cause.frontier(), &committed_event);
    let windows = snap.windows_for(&[(CH.into(), repeated.seq)]).unwrap();
    let receiver = applied(ok(&mut s, |tx| cr::claim_output(tx, &snap, &cause, &windows, &[], &[], cr::Owner::new(1)?)));
    ack(&mut s, &receiver, &witness(&receiver));
    let descendant = post(&mut s, Sender::Session("w-a2".into()), "genuine receiver output only after original DBack");
    let downstream = frontier(&s, &descendant); assert_eq!(downstream.components().len(), 1);
    assert_eq!(downstream.components()[0].chain_id(), committed_event.components()[0].chain_id());
    assert_eq!(downstream.components()[0].hop(), 1);
    let again = observed_post(&mut s, &parent, Some(&success), "original exact successful event after descendant");
    assert_eq!(frontier(&s, &again), committed_event, "same original event cannot become another root or hop after receiver output");
    assert!(!ack(&mut s, &parent, &success).newly_committed);
    assert!(matches!(read_message(s.conn(), &negative).unwrap(), MessageCause::Unresolved { .. }), "true pre-success child cannot gain parent from later event commit");
}

#[test]
fn retired_receipt_query_keeps_live_and_witnessed_retry_semantics() {
    let (mut s, _) = fixture();
    let first = claim(&mut s, READER, 1);
    let before = durable(&s);
    assert!(!ok(&mut s, |tx| cr::output_receipt_retired(tx, &first)));
    assert_eq!(durable(&s), before, "live query is readonly");
    applied(ok(&mut s, |tx| cr::settle_output(tx, &first, cr::Terminal::Requeue)));
    let settled = durable(&s);
    assert!(ok(&mut s, |tx| cr::output_receipt_retired(tx, &first)));
    assert_eq!(durable(&s), settled, "retired query is readonly");
    let next = claim(&mut s, READER, 2);
    let live = durable(&s);
    assert!(ok(&mut s, |tx| cr::output_receipt_retired(tx, &first)));
    assert!(!ok(&mut s, |tx| cr::output_receipt_retired(tx, &next)));
    assert_eq!(durable(&s), live, "old receipt cannot alter new owner");
    let success = witness(&next);
    assert!(ack(&mut s, &next, &success).newly_committed);
    let committed = durable(&s);
    assert!(ok(&mut s, |tx| cr::output_receipt_retired(tx, &next)));
    assert_eq!(durable(&s), committed, "unwitnessed replay checks no new commit");
    assert!(!ack(&mut s, &next, &success).newly_committed);
    assert_eq!(durable(&s), committed, "exact witnessed same-fate retry unchanged");
}

// Private-envelope compatibility controls. Actual Store/shared receipt APIs with
// the existing synthetic qualified observer; not physical Task/provider tests.
#[test]
fn capture_after_committed_b_can_install_ordinary_c_on_its_own_success() {
    let (mut s, _) = fixture();
    let a = claim(&mut s, READER, 1);
    let a_success = witness(&a);
    let a_ack = ack(&mut s, &a, &a_success);
    let b = post(&mut s, Sender::Human, "FRESHNESS_B_REAL_HUMAN");
    let b_claim = claim(&mut s, READER, 2);
    let b_success = witness(&b_claim);
    let b_ack = ack(&mut s, &b_claim, &b_success);
    let b_state = state(s.conn(), READER).unwrap();
    let c = post(&mut s, Sender::Human, "FRESHNESS_C_REAL_HUMAN");
    let c_claim = claim(&mut s, READER, 3);
    let captured_previous = c_claim.pending.0.capture.previous_output.clone();
    let c_success = witness(&c_claim);
    let c_ack = ack(&mut s, &c_claim, &c_success);
    let after = state(s.conn(), READER).unwrap();
    let child = observed_post(&mut s, &c_claim, Some(&c_success), "actual ordinary C descendant");
    let child_basis = frontier(&s, &child);
    let expected_b = frontier(&s, &b).successor().unwrap();
    let expected_c = frontier(&s, &c).successor().unwrap();
    let repeated = ack(&mut s, &c_claim, &c_success);
    let after_repeat = state(s.conn(), READER).unwrap();
    let c_output = output(s.conn(), c_claim.pending.stable_key()).unwrap();
    drop(s); // owned synchronous Store closes before main semantic oracles
    assert!(a_ack.newly_committed && b_ack.newly_committed && c_ack.newly_committed);
    assert_eq!(b_state.basis, expected_b);
    assert_eq!(b_state.last_output.as_deref(), Some(b_claim.pending.stable_key()));
    assert_eq!(captured_previous.as_deref(), Some(b_claim.pending.stable_key()));
    assert_eq!(after.basis, expected_c, "fresh capture after B really installs C, not permanent freeze");
    assert_eq!(after.last_output.as_deref(), Some(c_claim.pending.stable_key()));
    assert_eq!(child_basis, expected_c);
    assert_eq!((after.epoch, after.applied), (b_state.epoch, b_state.applied));
    assert_eq!(c_output.status, OutputStatus::Committed);
    assert!(!repeated.newly_committed);
    assert_eq!(after_repeat.basis, after.basis);
    assert_eq!(after_repeat.last_output, after.last_output);
}

#[test]
fn legacy_missing_previous_output_settles_old_exact_row_without_replacing_b() {
    let (mut s, _) = fixture();
    let prior = claim(&mut s, READER, 1);
    ack(&mut s, &prior, &witness(&prior));
    let channel = ok(&mut s, |tx| {
        let (channel, _) = crate::channels::ensure_dm(tx, "crew-a",
            &crewd_proto::MemberRef::session(READER), &crewd_proto::MemberRef::session("w-a2"))?;
        crate::cursors::pin_to_end(tx, READER, &channel.id)?;
        Ok(channel.id)
    });
    let a = post(&mut s, Sender::Human, "LEGACY_A_ORIGINAL_BODY");
    let row = match ok(&mut s, |tx| crate::wakes::enqueue(tx, crate::wakes::WakeRequest {
        session: READER.into(), kind: crate::wakes::WakeKind::Message,
        source: crate::wakes::source::message(&a.id), priority: crate::wakes::WakePriority::System,
        not_before_ms: None, note: None, origin: None,
    })) { crate::wakes::Enqueued::New(id) => id, other => panic!("actual original row {other:?}") };
    let mut old = ok(&mut s, |tx| {
        let prepared = cr::prepare_causal_unread(tx, READER)?;
        let ends = vec![(CH.into(), a.seq)];
        let causes = prepared.causes.select(&ends)?;
        let windows = prepared.snapshot.windows_for(&ends)?;
        Ok(applied(cr::claim_output(tx, &prepared.snapshot, &causes, &windows,
            &[row], &[], cr::Owner::new(2)?)?))
    });
    let original_lease = old.lease.clone();
    let original_receipt = old.pending.0.clone();
    let key = format!("{OUTPUT}{}", old.pending.stable_key());
    let raw = crewd_store::meta_get(s.conn(), &key).unwrap().unwrap();
    let mut envelope: serde_json::Value = serde_json::from_str(&raw).unwrap();
    let omitted = envelope["value"]["receipt"]["capture"].as_object_mut().unwrap()
        .remove("previous_output").expect("setup real A capture must remember prior committed output");
    // Exact legacy-format omission only. No identity/window/owner/success is injected.
    crewd_store::meta_set(s.conn(), &key, &serde_json::to_string(&envelope).unwrap()).unwrap();
    let decoded = output(s.conn(), old.pending.stable_key()).unwrap();
    old.pending = PendingOutput(decoded.receipt.clone()); // load original owned legacy receipt
    let old_success = witness(&old);
    let b = ok(&mut s, |tx| messages::post(tx, Sender::Human, messages::NewPost {
        channel_id: channel.clone(), text: "LEGACY_NEW_REAL_B_HOOK".into(), ..Default::default()
    })).message;
    let (hook, held, omitted_a) = ok(&mut s, |tx| {
        let prepared = cr::prepare_causal_unread(tx, READER)?;
        let held = prepared.snapshot.blocks_channel(CH);
        let omitted_a = !prepared.blocks.iter().any(|(ch, _)| ch == CH);
        let ends = vec![(channel.clone(), b.seq)];
        let causes = prepared.causes.select(&ends)?;
        Ok((applied(cr::commit_hook_output(tx, &prepared.snapshot, &causes, &ends)?), held, omitted_a))
    });
    let before = state(s.conn(), READER).unwrap();
    let b_cursor = crate::cursors::cursor(s.conn(), READER, &channel).unwrap();
    let settled = ack(&mut s, &old, &old_success);
    let after = state(s.conn(), READER).unwrap();
    let old_record = output(s.conn(), old.pending.stable_key()).unwrap();
    let fate = crate::wakes::by_ids(s.conn(), &[row]).unwrap()[0].clone();
    let a_cursor = crate::cursors::cursor(s.conn(), READER, CH).unwrap();
    let b_cursor_after = crate::cursors::cursor(s.conn(), READER, &channel).unwrap();
    let child = post(&mut s, Sender::Session(READER.into()), "after legacy first exact ack");
    let child_basis = frontier(&s, &child);
    let b_basis = frontier(&s, &b).successor().unwrap();
    let repeated = ack(&mut s, &old, &old_success);
    let after_repeat = state(s.conn(), READER).unwrap();
    drop(s);
    assert_eq!(omitted, serde_json::Value::String(prior.pending.stable_key().into()));
    let mut expected_legacy = original_receipt;
    expected_legacy.capture.previous_output = None;
    assert_eq!(decoded.receipt, expected_legacy, "only missing optional capture field defaults");
    assert_eq!(old.lease, original_lease);
    assert!(held && omitted_a, "independent genuine hook respects A reservation");
    assert_eq!(before.active.as_deref(), Some(old.pending.stable_key()));
    assert_eq!(before.last_output, hook.output);
    assert_eq!(before.basis, b_basis);
    assert!(settled.newly_committed, "old original exact row fate still commits");
    assert_eq!(old_record.status, OutputStatus::Committed);
    assert_eq!(fate.state, crate::wakes::WakeState::Delivered);
    assert_eq!(a_cursor, Some(a.seq));
    assert_eq!(b_cursor, Some(b.seq));
    assert_eq!(b_cursor_after, b_cursor);
    assert_eq!(after.basis, before.basis, "legacy missing predecessor stays conservative after B");
    assert_eq!(after.last_output, before.last_output);
    assert!(after.active.is_none());
    assert_eq!(child_basis, b_basis);
    assert!(!repeated.newly_committed);
    assert_eq!(after_repeat.basis, b_basis);
    assert_eq!(after_repeat.last_output, before.last_output);
}

#[test]
fn present_malformed_previous_output_rejects_read_and_exact_ack_without_writes() {
    let (mut s, _) = fixture();
    let old = claim(&mut s, READER, 1);
    let success = witness(&old);
    let key = format!("{OUTPUT}{}", old.pending.stable_key());
    let raw = crewd_store::meta_get(s.conn(), &key).unwrap().unwrap();
    let mut observations = Vec::new();
    for bad in [serde_json::json!("not-a-uuid"), serde_json::json!([]), serde_json::json!({"id":"fake"}), serde_json::json!(7)] {
        let mut envelope: serde_json::Value = serde_json::from_str(&raw).unwrap();
        envelope["value"]["receipt"]["capture"]["previous_output"] = bad;
        crewd_store::meta_set(s.conn(), &key, &serde_json::to_string(&envelope).unwrap()).unwrap();
        let before = lifecycle_db(&s);
        let read_error = output(s.conn(), old.pending.stable_key()).is_err();
        let ack_error = crate::run(&mut s, NOW, |tx|
            cr::ack_output_delivered(tx, &old, &success, crate::wakes::Riders::Release)).is_err();
        let after = lifecycle_db(&s);
        observations.push((read_error, ack_error, before, after));
    }
    crewd_store::meta_set(s.conn(), &key, &raw).unwrap();
    let restored = ack(&mut s, &old, &success);
    let committed = output(s.conn(), old.pending.stable_key()).unwrap().status;
    drop(s);
    for (read_error, ack_error, before, after) in observations {
        assert!(read_error && ack_error, "present malformed freshness identity is fail loud, not legacy None");
        assert_eq!(before, after, "invalid identity cannot change rows/cursors/receipt state");
    }
    assert!(restored.newly_committed);
    assert_eq!(committed, OutputStatus::Committed);
}

use crewd_data_worker::workspace::Workspace;
use crewd_data_protocol::*;
use serde_json::{Value,json};
fn uuid()->String{uuid::Uuid::new_v4().to_string()}
fn object(value:Value)->DataObject{serde_json::from_value(value).unwrap()}
fn block(id:&str,revision:&str,content:&str)->DataObject{object(json!({"kind":"block","id":id,"revision":revision,"contentType":"text/plain","content":content,"archived":false,"provenance":{"createdBy":{"kind":"human"},"updatedBy":{"kind":"human"},"createdAt":"2026-10-15T00:00:00Z","updatedAt":"2026-10-15T00:00:00Z","sources":[],"attributes":{"unknownField":[null,1,"preserved"]}}}))}
fn apply(w:&Workspace,mutations:Vec<Mutation>)->WriteIntent{WriteIntent{request_id:uuid(),expected_head:w.head().unwrap(),author:ResourceActor{kind:"human".into(),session_id:None},message:"owned private whole version control".into(),mutations}}
#[test]
fn shared_semantic_block_repeated_placements_historical_snapshot_and_uuid_recovery(){
 let dir=tempfile::tempdir().unwrap();let path=dir.path().join("data.dolt");let ns=uuid();let w=Workspace::open(&path,&ns).unwrap();let block_id=uuid();let view_id=uuid();let p1=uuid();let p2=uuid();
 let view=object(json!({"kind":"view","id":view_id,"revision":"1","title":"assembled","archived":false,"definition":{"kind":"document","placements":[{"id":p1,"block":{"state":"visible","reference":{"namespace":ns,"kind":"block","id":block_id}},"atCommit":null,"layout":{}},{"id":p2,"block":{"state":"visible","reference":{"namespace":ns,"kind":"block","id":block_id}},"atCommit":null,"layout":{}}],"layout":{}},"provenance":{"createdBy":{"kind":"human"},"updatedBy":{"kind":"human"},"createdAt":"2026-10-15T00:00:00Z","updatedAt":"2026-10-15T00:00:00Z","sources":[],"attributes":{}}}));
 let intent=apply(&w,vec![Mutation::PutObject{object:block(&block_id,"1","original semantic content"),expected_revision:None,captures:vec![]},Mutation::PutObject{object:view,expected_revision:None,captures:vec![Capture{anchor:format!("placement:{p1}"),namespace:ns.clone(),kind:"block".into(),id:block_id.clone(),commit:"self".into(),revision:"1".into(),live:true},Capture{anchor:format!("placement:{p2}"),namespace:ns.clone(),kind:"block".into(),id:block_id.clone(),commit:"self".into(),revision:"1".into(),live:true}]},Mutation::Sql{sql:"CREATE TABLE \"研究 数据\"(id INTEGER PRIMARY KEY,value TEXT)".into(),params:vec![]},Mutation::Sql{sql:"INSERT INTO \"研究 数据\" VALUES(1,'one publication')".into(),params:vec![]}]);
 let first=w.apply(&intent).unwrap();let initial_commit=w.head().unwrap();assert_eq!(first["commit"],initial_commit);drop(w);
 let w=Workspace::open_existing(&path,&ns,false).unwrap();assert_eq!(w.apply(&intent).unwrap(),first);assert_eq!(w.engine.scalar("SELECT count(*) FROM \"研究 数据\"",&[]).unwrap(),1);
 assert_eq!(w.object("view",&view_id,None).unwrap()["object"]["definition"]["placements"].as_array().unwrap().len(),2);
 let second=apply(&w,vec![Mutation::PutObject{object:block(&block_id,"2","live update"),expected_revision:Some("1".into()),captures:vec![]}]);w.apply(&second).unwrap();assert_eq!(w.object("block",&block_id,None).unwrap()["object"]["content"],"live update");assert_eq!(w.object("block",&block_id,Some(&initial_commit)).unwrap()["object"]["content"],"original semantic content");
 let lookup=w.handle(Request{id:uuid(),namespace:ns.clone(),branch:"deleted-original-branch".into(),operation:Operation::Lookup{request_id:intent.request_id.clone()}});assert!(matches!(lookup,Response::Ok(ref v) if v==&first));
 let changed=WriteIntent{message:"different payload".into(),..intent};assert!(w.apply(&changed).is_err());
 let ro=Workspace::open_existing(&path,&ns,true).unwrap();assert_eq!(ro.object("block",&block_id,None).unwrap()["object"]["revision"],"2");assert!(ro.apply(&apply(&ro,vec![Mutation::Sql{sql:"DELETE FROM \"研究 数据\"".into(),params:vec![]}])).is_err());
}
#[test]
fn rejected_batch_rolls_back_every_object_sql_and_uuid_fact(){
 let dir=tempfile::tempdir().unwrap();let ns=uuid();let path=dir.path().join("data.dolt");let w=Workspace::open(&path,&ns).unwrap();let before=w.head().unwrap();let key=uuid();let intent=apply(&w,vec![Mutation::PutObject{object:block(&key,"1","must roll back"),expected_revision:None,captures:vec![]},Mutation::Sql{sql:"CREATE TABLE business(id INTEGER PRIMARY KEY)".into(),params:vec![]},Mutation::Sql{sql:"INSERT INTO business VALUES(1),(1)".into(),params:vec![]}]);
 let r=w.handle(Request{id:uuid(),namespace:ns.clone(),branch:"main".into(),operation:Operation::Apply(intent.clone())});assert!(matches!(r,Response::Refused{..}),"{r:?}");assert_eq!(w.head().unwrap(),before);assert!(w.object("block",&key,None).unwrap().is_null());assert_eq!(w.lookup(&intent.request_id).unwrap()["status"],"notFound");assert!(w.engine.scalar("SELECT count(*) FROM business",&[]).is_err());drop(w);
 let w=Workspace::open_existing(&path,&ns,false).unwrap();assert_eq!(w.head().unwrap(),before);assert!(w.object("block",&key,None).unwrap().is_null());
}
#[test]
fn exact_snapshot_query_pages_do_not_read_later_head_or_mutate_through_read_path(){
 let dir=tempfile::tempdir().unwrap();let ns=uuid();let path=dir.path().join("paged.dolt");let w=Workspace::open(&path,&ns).unwrap();
 let first=apply(&w,vec![Mutation::Sql{sql:"CREATE TABLE \"任意 数据\"(id INTEGER PRIMARY KEY, value TEXT)".into(),params:vec![]},Mutation::Sql{sql:"INSERT INTO \"任意 数据\" VALUES(1,'one'),(2,'two'),(3,'three'),(4,'four'),(5,'five')".into(),params:vec![]}]);
 let committed=w.apply(&first).unwrap();let head=committed["commit"].as_str().unwrap().to_owned();
 let next=apply(&w,vec![Mutation::Sql{sql:"DELETE FROM \"任意 数据\"".into(),params:vec![]}]);w.apply(&next).unwrap();let current=w.head().unwrap();
 let read=|offset,limit,sql:&str|w.handle(Request{id:uuid(),namespace:ns.clone(),branch:"main".into(),operation:Operation::ReadPage{at_commit:head.clone(),sql:sql.into(),params:vec![],offset,limit}});
 let Response::Ok(page)=read(0,2,"SELECT id,value FROM \"任意 数据\" ORDER BY id") else{panic!("first page failed")};assert_eq!(page["rows"].as_array().unwrap().len(),2);assert_eq!(page["rows"][0][0],json!({"type":"integer","value":"1"}));assert_eq!(page["truncated"],true);
 let Response::Ok(page)=read(4,2,"SELECT id,value FROM \"任意 数据\" ORDER BY id") else{panic!("last page failed")};assert_eq!(page["rows"].as_array().unwrap().len(),1);assert_eq!(page["rows"][0][1],json!({"type":"text","value":"five"}));assert_eq!(page["truncated"],false);
 assert!(matches!(read(0,2,"DELETE FROM \"任意 数据\""),Response::Refused{..}));assert!(matches!(read(1_000_001,2,"SELECT 1"),Response::Refused{..}));assert_eq!(w.head().unwrap(),current);assert_eq!(w.engine.scalar("SELECT count(*) FROM \"任意 数据\"",&[]).unwrap(),0);
}
#[test]
fn explicit_initialize_uuid_is_published_with_format_and_never_reopens_existing_as_new(){
 let dir=tempfile::tempdir().unwrap();let ns=uuid();let path=dir.path().join("first.dolt");let original=uuid();
 let w=Workspace::open_for_request(&path,&ns,&original).unwrap();let head=w.head().unwrap();let lookup=w.lookup(&original).unwrap();assert_eq!(lookup["status"],"known");assert_eq!(lookup["commit"],head);assert_eq!(lookup["result"]["initialized"],true);drop(w);
 let bytes=std::fs::read(&path).unwrap();assert!(Workspace::open_for_request(&path,&ns,&original).is_err());assert_eq!(std::fs::read(&path).unwrap(),bytes);
 let read=Workspace::open_existing(&path,&ns,true).unwrap();assert_eq!(read.lookup(&original).unwrap(),lookup);assert_eq!(read.head().unwrap(),head);
}
#[test]
fn actual_catalog_keyset_and_version_graph_preserve_snapshot_without_control_disclosure(){
 let dir=tempfile::tempdir().unwrap();let ns=uuid();let path=dir.path().join("metadata.dolt");let w=Workspace::open(&path,&ns).unwrap();let initial=w.head().unwrap();
 let committed=w.apply(&apply(&w,vec![
  Mutation::Sql{sql:"CREATE TABLE \"自由 数据\"(a TEXT NOT NULL,b INTEGER NOT NULL,v TEXT,PRIMARY KEY(a,b))".into(),params:vec![]},
  Mutation::Sql{sql:"INSERT INTO \"自由 数据\" VALUES('same',1,'one'),('same',2,'two'),('z',1,'three')".into(),params:vec![]},
  Mutation::Sql{sql:"CREATE TABLE without_key(note TEXT)".into(),params:vec![]},
  Mutation::Sql{sql:"INSERT INTO without_key VALUES('a'),('b')".into(),params:vec![]},
 ])).unwrap();let head=committed["commit"].as_str().unwrap().to_owned();
 let catalog=w.catalog(&head,None,1).unwrap();assert_eq!(catalog["objects"].as_array().unwrap().len(),1);let next=catalog["next"].as_str().unwrap();let rest=w.catalog(&head,Some(next),100).unwrap();
 let names=catalog["objects"].as_array().unwrap().iter().chain(rest["objects"].as_array().unwrap()).map(|o|o["name"].as_str().unwrap()).collect::<Vec<_>>();assert!(names.contains(&"自由 数据"));assert!(names.contains(&"without_key"));assert!(names.iter().all(|n|!n.starts_with("_pc_")&&!n.starts_with("dolt_")));
 let page=w.table(&head,"自由 数据",None,2).unwrap();assert_eq!(page["stableOrder"],true);assert_eq!(page["truncated"],true);let key:Vec<Cell>=serde_json::from_value(page["nextKey"].clone()).unwrap();
 let last=w.table(&head,"自由 数据",Some(&key),2).unwrap();assert_eq!(last["rows"].as_array().unwrap().len(),1);assert_eq!(last["rows"][0][2],json!({"type":"text","value":"three"}));assert_eq!(last["truncated"],false);
 let unstable=w.table(&head,"without_key",None,1).unwrap();assert_eq!(unstable["stableOrder"],false);assert_eq!(unstable["truncated"],true);assert!(unstable["nextKey"].is_null());assert!(w.table(&head,"_pc_requests",None,10).is_err());
 let history=w.history(None,1).unwrap();assert_eq!(history["items"][0]["hash"],head);assert_eq!(history["items"][0]["parents"][0],initial);let history_next=history["next"].as_str().unwrap();
 w.apply(&apply(&w,vec![Mutation::Sql{sql:"DELETE FROM \"自由 数据\"".into(),params:vec![]}])).unwrap();
 let old=w.history(Some(history_next),10).unwrap();assert_eq!(old["items"][0]["hash"],initial);assert_eq!(w.table(&head,"自由 数据",None,10).unwrap()["rows"].as_array().unwrap().len(),3);
 let branch=w.handle(Request{id:uuid(),namespace:ns.clone(),branch:"main".into(),operation:Operation::Branch{request_id:uuid(),name:"topic".into(),expected_head:w.head().unwrap()}});assert!(matches!(branch,Response::Ok(ref v)if v["branchCreated"]=="topic"&&v["branch"]=="main"));
 assert_eq!(w.branches(None,10).unwrap()["items"].as_array().unwrap().len(),2);
 let checkout=w.handle(Request{id:uuid(),namespace:ns.clone(),branch:"main".into(),operation:Operation::Checkout{request_id:uuid(),branch:"topic".into(),expected_head:w.head().unwrap()}});assert!(matches!(checkout,Response::Ok(ref v)if v["branch"]=="topic"&&v["head"].is_string()));
}
#[test]
fn first_apply_is_one_version_publication_and_failed_fresh_file_never_reinitializes(){
 let dir=tempfile::tempdir().unwrap();let ns=uuid();let path=dir.path().join("first-batch.dolt");let request=uuid();
 let w=Workspace::prepare_for_request(&path,&ns,&request).unwrap();assert!(!w.engine.autocommit());
 let intent=WriteIntent{request_id:request.clone(),expected_head:String::new(),author:ResourceActor{kind:"session".into(),session_id:Some("owned-fake-agent".into())},message:"explicit first content batch".into(),mutations:vec![Mutation::Sql{sql:"CREATE TABLE arbitrary(id INTEGER PRIMARY KEY,v TEXT)".into(),params:vec![]},Mutation::Sql{sql:"INSERT INTO arbitrary VALUES(1,'first content')".into(),params:vec![]}]};
 let actual=w.handle(Request{id:request.clone(),namespace:ns.clone(),branch:"main".into(),operation:Operation::FirstApply(intent.clone())});let Response::Ok(value)=actual else{panic!("first batch failed: {actual:?}")};let commit=value["commit"].as_str().unwrap().to_owned();assert_eq!(value["result"]["initialized"],true);assert!(w.engine.autocommit());drop(w);
 let read=Workspace::open_existing(&path,&ns,true).unwrap();assert_eq!(read.lookup(&request).unwrap(),value);assert_eq!(read.head().unwrap(),commit);assert_eq!(read.engine.scalar("SELECT v FROM arbitrary",&[]).unwrap(),"first content");assert_eq!(read.engine.scalar("SELECT count(*) FROM dolt_log WHERE message LIKE 'pc:%'",&[]).unwrap(),1);drop(read);
 let bytes=std::fs::read(&path).unwrap();assert!(Workspace::prepare_for_request(&path,&ns,&request).is_err());assert_eq!(std::fs::read(&path).unwrap(),bytes);
 let path=dir.path().join("failed-first.dolt");let bad=uuid();let w=Workspace::prepare_for_request(&path,&ns,&bad).unwrap();let intent=WriteIntent{request_id:bad.clone(),mutations:vec![Mutation::Sql{sql:"CREATE TABLE must_rollback(id INTEGER PRIMARY KEY)".into(),params:vec![]},Mutation::Sql{sql:"INSERT INTO must_rollback VALUES(1),(1)".into(),params:vec![]}],..intent};
 let actual=w.handle(Request{id:bad.clone(),namespace:ns.clone(),branch:"main".into(),operation:Operation::FirstApply(intent)});assert!(matches!(actual,Response::Unknown{..}));drop(w);assert!(path.is_file());let bytes=std::fs::read(&path).unwrap();assert!(Workspace::open_existing(&path,&ns,false).is_err());assert!(Workspace::prepare_for_request(&path,&ns,&bad).is_err());assert_eq!(std::fs::read(&path).unwrap(),bytes);
}

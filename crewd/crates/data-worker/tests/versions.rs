//! Real pinned DoltLite behavior, no Core, provider, system store or SQLite substitute.
use crewd_data_worker::engine::{Engine,Policy};
use crewd_data_protocol::Cell;
use serde_json::Value;
fn commit(db:&Engine,message:&str)->String {
 db.scalar("SELECT dolt_commit('-A','-m',?,'--author','fixture <fixture@pendingcrew.invalid>')",&[Cell::Text(message.into())]).unwrap().as_str().unwrap().into()
}
fn head(db:&Engine)->Value{db.scalar("SELECT dolt_hashof('HEAD')",&[]).unwrap()}
#[test]
fn real_branches_merge_conflict_resolution_and_historical_snapshot() {
 let dir=tempfile::tempdir().unwrap();let path=dir.path().join("knowledge.dolt");let db=Engine::open(&path,false).unwrap();
 db.query("CREATE TABLE \"semantic rows\"(id INTEGER PRIMARY KEY, content TEXT)",&[],Policy::AgentWrite).unwrap();
 db.query("INSERT INTO \"semantic rows\" VALUES(1,'base')",&[],Policy::AgentWrite).unwrap();
 let base=commit(&db,"base");
 db.scalar("SELECT dolt_branch('topic')",&[]).unwrap();
 db.scalar("SELECT dolt_checkout('topic')",&[]).unwrap();
 db.query("UPDATE \"semantic rows\" SET content='topic' WHERE id=1",&[],Policy::AgentWrite).unwrap();let topic=commit(&db,"topic");
 db.scalar("SELECT dolt_checkout('main')",&[]).unwrap();
 db.query("UPDATE \"semantic rows\" SET content='main' WHERE id=1",&[],Policy::AgentWrite).unwrap();let main=commit(&db,"main");
 db.execute("BEGIN IMMEDIATE",&[]).unwrap();
 assert!(db.scalar("SELECT dolt_merge('--no-ff','--no-commit','topic')",&[]).is_err());
 assert!(!db.autocommit());assert_eq!(head(&db),main);
 let conflicts=db.execute("SELECT * FROM dolt_conflicts",&[]).unwrap();assert_eq!(conflicts["rows"].as_array().unwrap().len(),1);
 assert!(db.scalar("SELECT dolt_commit('-A','-m','invalid unresolved')",&[]).is_err());
 db.scalar("SELECT dolt_conflicts_resolve('--theirs','semantic rows')",&[]).unwrap();
 let merged=commit(&db,"resolved merge");assert!(db.autocommit());assert_ne!(merged,main);assert_ne!(merged,topic);
 assert_eq!(db.scalar("SELECT content FROM \"semantic rows\" WHERE id=1",&[]).unwrap(),"topic");drop(db);
 let snapshot=Engine::open(&path.join(&base),true).unwrap();assert_eq!(snapshot.scalar("SELECT content FROM \"semantic rows\" WHERE id=1",&[]).unwrap(),"base");assert!(snapshot.query("DELETE FROM \"semantic rows\"",&[],Policy::AgentWrite).is_err());drop(snapshot);
 let db=Engine::open(&path,false).unwrap();assert_eq!(head(&db),merged);
}
#[test]
fn real_ref_changes_escape_sql_rollback_and_reset_preserves_history() {
 let dir=tempfile::tempdir().unwrap();let path=dir.path().join("knowledge.dolt");let db=Engine::open(&path,false).unwrap();
 db.query("CREATE TABLE user_data(id INTEGER PRIMARY KEY, value TEXT)",&[],Policy::AgentWrite).unwrap();db.query("INSERT INTO user_data VALUES(1,'first')",&[],Policy::AgentWrite).unwrap();let first=commit(&db,"first");
 db.execute("BEGIN",&[]).unwrap();db.scalar("SELECT dolt_branch('durable-ref')",&[]).unwrap();db.execute("ROLLBACK",&[]).unwrap();
 assert_eq!(db.execute("SELECT name FROM dolt_branches WHERE name='durable-ref'",&[]).unwrap()["rows"].as_array().unwrap().len(),1);
 db.query("UPDATE user_data SET value='second'",&[],Policy::AgentWrite).unwrap();let second=commit(&db,"second");
 // A recovery branch preserves the prior tip before an explicit destructive reset.
 db.scalar("SELECT dolt_branch('before-reset')",&[]).unwrap();db.scalar("SELECT dolt_reset('--hard',?)",&[Cell::Text(first.clone())]).unwrap();assert_eq!(head(&db),first);drop(db);
 let db=Engine::open(&path,false).unwrap();assert_eq!(head(&db),first);assert_eq!(db.scalar("SELECT value FROM user_data",&[]).unwrap(),"first");assert_eq!(db.scalar("SELECT hash FROM dolt_branches WHERE name='before-reset'",&[]).unwrap(),second);
}

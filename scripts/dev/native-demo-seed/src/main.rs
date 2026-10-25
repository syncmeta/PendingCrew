//! One-shot offline sample assembler. No server, provider, runner, network or task start.
//! Refuses an existing destination. The ready marker is written only after all checks.
use crewd_collab::{self as collab, Sender, crews, channels, sessions, messages};
use crewd_proto::{*, team_data::*};
use crewd_store::{Store, team_data::{self as knowledge, Knowledge}};
use serde_json::{Value, json};
use std::{collections::BTreeMap, error::Error, fs, path::{Path, PathBuf}};
use std::os::unix::fs::{DirBuilderExt, OpenOptionsExt};
use std::io::Write;
// Reuse the exact production edit compiler, not a second data schema/importer.
#[path = "../../../../crewd/crates/server/src/team_data/compile.rs"]
mod compile;
type Result<T> = std::result::Result<T, Box<dyn Error>>;
const NOTICE: &str = "【演示快照】来自 HTML Demo；成员已停止，不代表真实执行或当前审批。";
fn uid() -> String { uuid::Uuid::new_v4().to_string() }
fn text(v: &Value, key: &str) -> String { v[key].as_str().unwrap_or("").to_owned() }
fn items<'a>(v: &'a Value, key: &str) -> &'a [Value] { v[key].as_array().map(Vec::as_slice).unwrap_or(&[]) }
fn private(path: &Path) -> Result<()> { fs::DirBuilder::new().mode(0o700).create(path)?; Ok(()) }
fn write_new(path: &Path, value: &Value) -> Result<()> {
    let mut f=fs::OpenOptions::new().write(true).create_new(true).mode(0o600).open(path)?;
    f.write_all(serde_json::to_string_pretty(value)?.as_bytes())?; f.sync_all()?; Ok(())
}
struct Port<'a>(&'a Knowledge);
impl compile::Port for Port<'_> {
    fn object(&self, r: &DataRef, at: Option<&str>) -> std::result::Result<compile::Stored,String> {
        self.0.get(r,at).map(|v|compile::Stored{object:v.object,captures:v.captures}).map_err(|e|e.to_string())
    }
    fn target(&self, _: &DataRef, _: Option<&str>) -> std::result::Result<compile::Target,String> {
        Err("demo does not import cross-authority snapshots".into())
    }
}
fn document(store:&mut Store, authority:&Path, crew:&str, channel:&str, doc:&Value) -> Result<()> {
    let actor=collab::team_data::authorize(store.conn(),&Sender::Human,crew,true)?;
    let db=crewd_store::meta_get(store.conn(),"dbId")?.ok_or("missing dbId")?;
    let ns=knowledge::namespace(&db,crew);
    let base=DataContext{db_id:db.clone(),authority_id:db,restore_epoch:"initial".into(),crew_id:crew.into(),namespace:ns.clone(),generation:"0".into()};
    let mut k=Knowledge::open(&authority.join("knowledge").join(format!("{ns}.sqlite")),base,true)?;
    let context=k.context()?;
    let mut body=format!("{NOTICE}\n原 HTML 对象：{}\n\n{}",text(doc,"id"),text(doc,"body"));
    if !doc["dataView"].is_null() { body.push_str(&format!("\n\n## 原演示数据视图（静态样例）\n```json\n{}\n```",serde_json::to_string_pretty(&doc["dataView"])?)); }
    if !doc["sources"].is_null() { body.push_str(&format!("\n\n## 来源\n{}",serde_json::to_string_pretty(&doc["sources"])?)); }
    let edits:Vec<DataEdit>=serde_json::from_value(json!([
        {"kind":"createBlock","alias":"body","contentType":"text/plain","content":body,"sources":[],"attributes":{"demoSourceId":doc["id"]}},
        {"kind":"createView","alias":"document","title":doc["title"],"definition":{"kind":"document","placements":[{"kind":"set","id":null,"alias":"bodyPlacement","block":{"kind":"created","alias":"body"},"atRevision":null,"layout":{}}],"layout":{}},"sources":[],"attributes":{"demoSourceId":doc["id"]}}
    ]))?;
    let created=k.allocate(&compile::allocations(&context,&edits)?)?;
    let mutations=compile::compile(&context,&created,&actor,&crewd_store::now_ts(),&edits,&Port(&k))?;
    let p=DataSubmitParams{request_id:uid(),context:context.clone(),change:DataChange::Apply{message:"HTML Demo 原生文档样例".into(),edits}};
    let result=k.apply(&p.request_id,&context,"human",&knowledge::digest(&p)?,created,mutations,false,None)?;
    if !matches!(result.phase,DataPhase::Committed) { return Err("document did not commit".into()); }
    let reference=match result.created.get("document").ok_or("document id absent")? {
        DataCreated::Object{object}=>object.reference.clone(), _=>return Err("not a view".into())
    };
    let source=ChannelKnowledgeReference{context,reference};
    collab::run(store,crewd_store::now_ms(),|tx|{
        let by=collab::channel_work::authorize(tx.conn(),&Sender::Human,channel,true)?;
        let rev=crewd_store::channel_work::revision(tx.conn(),channel)?;
        crewd_store::knowledge_channels::change(tx.conn(),channel,&source,rev,true,&by,&tx.now_ts())?;
        Ok(())
    })?;
    Ok(())
}
fn main() -> Result<()> {
    if std::env::args().len()!=1 { return Err("no arguments accepted; this seeder only creates the fixed absent Demo root".into()); }
    let home=PathBuf::from(std::env::var("HOME")?);
    if !home.is_absolute() { return Err("absolute home required".into()); }
    let root=home.join("Library/Application Support/PendingCrewDemo");
    for parent in root.parent().ok_or("parent absent")?.ancestors() {
        let m=fs::symlink_metadata(parent)?;
        if !m.is_dir() || m.file_type().is_symlink() { return Err("unsafe parent alias".into()); }
    }
    // create() fails on every existing entry, including a symlink or unfinished seed.
    private(&root)?;
    let authority=root.join("authority");private(&authority)?;
    private(&root.join("legacy"))?;private(&root.join("workspace"))?;
    private(&authority.join("knowledge"))?;
    let content:Value=serde_json::from_str(include_str!("../content.json"))?;
    if content["kind"]!="pendingcrew.native-html-demo.v1" { return Err("unknown content".into()); }
    let mut store=Store::open_data_dir(&authority)?;
    let mut mapping=json!({"kind":"pendingcrew.native-html-demo.mapping.v1","crews":[],"notice":NOTICE});
    for crew in items(&content,"crews") {
        let (native,_) = collab::run(&mut store,crewd_store::now_ms(),|tx|crews::create(tx,crews::NewCrew{
            title:format!("{} · 演示",text(crew,"title")),working_directory:root.join("workspace").to_string_lossy().into_owned(),..Default::default()
        }))?;
        let mut roles=BTreeMap::<String,String>::new();
        for role in items(crew,"sessions") {
            let name=text(role,"name");
            let (session,_) = collab::run(&mut store,crewd_store::now_ms(),|tx|sessions::register(tx,sessions::NewSession{
                crew_id:native.id.clone(),role:Role::Worker,executor:if text(role,"detail").contains("Claude"){"claude"}else{"codex"}.into(),
                title:Some(name.clone()),display_name:Some(name.clone()),brief:Some(format!("{NOTICE}\n{}",text(role,"detail"))),..Default::default()
            }))?;
            roles.insert(name,session.id);
        }
        let mut mapped=json!({"sourceId":crew["id"],"nativeId":native.id,"sessions":roles,"channels":[]});
        for ch in items(crew,"channels") {
            let source_id=text(ch,"id");
            let channel=if source_id==format!("{}-main",text(crew,"id")) {
                crewd_store::channels::lobby_id(&native.id)
            } else {
                let (c,_) = collab::run(&mut store,crewd_store::now_ms(),|tx|channels::create(tx,&Sender::Human,channels::NewChannel{
                    crew_id:native.id.clone(),kind:ChannelKind::Topic,name:text(ch,"title"),purpose:Some(format!("{NOTICE}\n{}",text(ch,"subtitle"))),members:roles.values().map(|id|MemberRef::Session{id:id.clone()}).collect()
                }))?; c.id
            };
            collab::run(&mut store,crewd_store::now_ms(),|tx|messages::post(tx,Sender::Human,messages::NewPost{channel_id:channel.clone(),text:format!("{NOTICE}\n{}\n{}",text(ch,"title"),text(ch,"subtitle")),..Default::default()}))?;
            for message in items(ch,"messages") {
                let name=text(message,"sender");
                // A missing role stays an attributed Human quotation, never a made-up agent.
                let (sender,body)=match roles.get(&name) {Some(id)=>(Sender::Session(id.clone()),format!("【演示历史】{}",text(message,"text"))),None=>(Sender::Human,format!("【HTML 演示原文 · {name}】{}",text(message,"text")))};
                collab::run(&mut store,crewd_store::now_ms(),|tx|messages::post(tx,sender,messages::NewPost{channel_id:channel.clone(),text:body,..Default::default()}))?;
            }
            let mut todo_ids=BTreeMap::new();
            for n in items(ch,"requests") {
                let (r,_) = collab::run(&mut store,crewd_store::now_ms(),|tx|collab::requirements::create(tx,&Sender::Human,TodoCreateParams{
                    request_id:uid(),crew_id:native.id.clone(),channel_id:Some(channel.clone()),text:format!("{}\n\n【演示来源 {}；原状态 {}，此处仅待办样例】",text(n,"text"),text(n,"id"),text(n,"status"))
                }))?; todo_ids.insert(text(n,"id"),r.todo.requirement_id.ok_or("missing public requirement id")?);
            }
            for ask in items(ch,"asks") {
                collab::run(&mut store,crewd_store::now_ms(),|tx|collab::todo_attributes::create_human(tx,&Sender::Human,HumanTaskCreateParams{
                    request_id:uid(),crew_id:native.id.clone(),channel_id:channel.clone(),text:format!("{}\n\n{}\n选项：{}\n【演示来源 {}，非当前审批】",text(ask,"question"),text(ask,"context"),ask["options"],text(ask,"id"))
                }))?;
            }
            let mut plan_ids=BTreeMap::new();
            for plan in items(ch,"plans") {
                let p:WorkCreateParams=serde_json::from_value(json!({"requestId":uid(),"channelId":channel,"crewId":native.id,"title":plan["title"],
                    "goal":format!("{}\n\n{NOTICE}\n原对象 {} / 原状态 {} / 原负责人 {}",text(plan,"goal"),text(plan,"id"),text(plan,"status"),text(plan,"owner")),"acceptance":text(plan,"acceptance"),
                    "parent":plan_ids.get(&text(plan,"parent")),"sources":items(plan,"sources").iter().filter_map(|x|todo_ids.get(x.as_str().unwrap_or(""))).collect::<Vec<_>>() }))?;
                let (p,_) = collab::run(&mut store,crewd_store::now_ms(),|tx|collab::work::create(tx,&Sender::Human,p))?;
                plan_ids.insert(text(plan,"id"),p.id);
            }
            for doc in items(ch,"docs") { document(&mut store,&authority,&native.id,&channel,doc)?; }
            mapped["channels"].as_array_mut().unwrap().push(json!({"sourceId":source_id,"nativeId":channel,"requirements":todo_ids,"plans":plan_ids}));
        }
        // Choose the captain only after inert content is assembled: requirements do
        // not enqueue a new-captain wake during the seed. Persist the original
        // Human-stop floor on every role so automatic work remains paused on boot.
        if let Some(first)=items(crew,"sessions").first() {
            let captain=roles.get(&text(first,"name")).ok_or("missing captain")?;
            collab::run(&mut store,crewd_store::now_ms(),|tx|crews::set_captain(tx,&native.id,captain))?;
        }
        for role in roles.values() {
            collab::run(&mut store,crewd_store::now_ms(),|tx|collab::wakes::human_stop(tx,role))?;
        }
        mapping["crews"].as_array_mut().unwrap().push(mapped);
    }
    let conn=store.conn();
    let running:i64=conn.query_row("SELECT COUNT(*) FROM sessions WHERE desired != 'stopped' OR was_working != 0",[],|r|r.get(0))?;
    if running!=0 { return Err("seed must have zero desired/working sessions".into()); }
    let pending:i64=conn.query_row("SELECT COUNT(*) FROM wakes WHERE state IN ('pending','inflight')",[],|r|r.get(0))?;
    if pending!=0 { return Err("seed must have no pending or inflight wakes".into()); }
    for table in ["native_execution_runtimes","native_execution_inputs","native_execution_receipts","codex_native_inputs"] {
        let count:i64=conn.query_row(&format!("SELECT COUNT(*) FROM {table}"),[],|r|r.get(0))?;
        if count!=0 {return Err(format!("nonempty execution table {table}").into());}
    }
    let check:String=conn.query_row("PRAGMA quick_check",[],|r|r.get(0))?;
    if check!="ok" { return Err(check.into()); }
    conn.execute_batch("PRAGMA wal_checkpoint(TRUNCATE)")?;
    drop(store);
    write_new(&root.join("source-map.json"),&mapping)?;
    write_new(&root.join("demo-ready.json"),&json!({"kind":"pendingcrew.native-html-demo.v1","bundleIdentifier":"com.pendingname.pendingcrew.demo","ready":true,"notice":NOTICE,"contentDigest":knowledge::digest(&content)?,"source":content["sources"]}))?;
    println!("{}",json!({"ready":true,"root":root,"crews":mapping["crews"].as_array().unwrap().len(),"executionRows":0}));
    Ok(())
}

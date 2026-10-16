//! Read-only, bounded source cut for Core15 -> native document authority import.
//! Audit history is scanned/hashed one row at a time, never collected with every
//! historical body into a single in-memory document or IPC frame.
use crate::{Result,StoreError,documents};
use crewd_proto::{DocumentSummary,ResourceActor};
use rusqlite::{Connection,params};
use serde::{Serialize,Deserialize};
#[derive(Clone,Debug,Serialize,Deserialize)]
#[serde(rename_all="camelCase",deny_unknown_fields)]
pub struct CreateWitness{pub request_id:String,pub payload_sha256:String}
#[derive(Clone,Debug,Serialize,Deserialize)]
#[serde(rename_all="camelCase",deny_unknown_fields)]
pub struct DocumentEvent{pub sequence:String,pub revision:String,pub actor:ResourceActor,pub actor_json:String,pub kind:String,pub title:String,pub body:Option<String>,pub at:String}
#[derive(Clone,Debug,Serialize,Deserialize)]
#[serde(rename_all="camelCase",deny_unknown_fields)]
pub struct LegacyDocument{
 pub summary:DocumentSummary,pub body:String,pub created_by_json:String,pub updated_by_json:String,
 pub audit_rows:u64,pub create_witnesses:u64,pub audit_source_sha256:String,
}
#[derive(Clone,Debug,Serialize,Deserialize,PartialEq,Eq)]
#[serde(rename_all="camelCase",deny_unknown_fields)]
pub struct SourceWitness{pub document_id:String,pub crew_id:String,pub revision:String,pub body_sha256:String,pub complete_source_sha256:String}
fn invalid(s:&str)->StoreError{StoreError::Invalid(s.into())}
fn digest(bytes:&[u8])->String{ring::digest::digest(&ring::digest::SHA256,bytes).as_ref().iter().map(|b|format!("{b:02x}")).collect()}
fn hex(bytes:&[u8])->String{bytes.iter().map(|b|format!("{b:02x}")).collect()}
fn hash_row(hash:&mut ring::digest::Context,kind:&str,row:&impl Serialize)->Result<()>{
 let encoded=serde_json::to_vec(&serde_json::to_value(row)?)?;if encoded.len()>8*1024*1024{return Err(invalid("legacy audit row exceeds lossless import frame bound"))}
 hash.update(kind.as_bytes());hash.update(&(encoded.len() as u64).to_be_bytes());hash.update(&encoded);Ok(())
}
impl LegacyDocument{
 pub fn witness(&self)->Result<SourceWitness>{Ok(SourceWitness{document_id:self.summary.id.clone(),crew_id:self.summary.crew_id.clone(),revision:self.summary.revision.clone(),body_sha256:digest(self.body.as_bytes()),complete_source_sha256:digest(&serde_json::to_vec(self)?)})}
}
fn decode_event(row:&rusqlite::Row<'_>)->rusqlite::Result<(DocumentEvent,String)>{
 let seq:i64=row.get(0)?;let revision:i64=row.get(1)?;let actor_json:String=row.get(2)?;
 let actor:ResourceActor=serde_json::from_str(&actor_json).map_err(|e|rusqlite::Error::FromSqlConversionFailure(2,rusqlite::types::Type::Text,Box::new(e)))?;
 Ok((DocumentEvent{sequence:seq.to_string(),revision:revision.to_string(),actor,actor_json,kind:row.get(3)?,title:row.get(4)?,body:row.get(5)?,at:row.get(6)?},row.get(7)?))
}
const EVENTS:&str="SELECT seq,revision,actor,kind,title,body,at,crew_id FROM document_events WHERE document_id=?1";
/// The caller owns DataDirLock and one stable SQLite snapshot. Deleted documents
/// remain in the import, including their original current body and audit rows.
pub fn read(conn:&Connection,id:&str)->Result<LegacyDocument>{
 let summary=documents::get(conn,id)?.ok_or_else(||invalid("legacy document source missing"))?;
 let body=documents::body(conn,id)?.ok_or_else(||invalid("legacy document body missing"))?;
 if body.len()>1048576||body.len() as u64!=summary.body_length{return Err(invalid("legacy document length witness changed"))}
 let(created_by_json,updated_by_json):(String,String)=conn.query_row("SELECT created_by,updated_by FROM documents WHERE id=?1",[id],|r|Ok((r.get(0)?,r.get(1)?)))?;
 let mut hash=ring::digest::Context::new(&ring::digest::SHA256);let mut create_witnesses=0u64;
 {let mut q=conn.prepare("SELECT request_id,payload_digest FROM document_requests WHERE document_id=?1 ORDER BY request_id")?;
  for row in q.query_map([id],|r|Ok((r.get::<_,String>(0)?,r.get::<_,Vec<u8>>(1)?)))?{
   let(request_id,payload)=row?;if payload.len()!=32{return Err(invalid("legacy create digest corrupt"))}
   hash_row(&mut hash,"request",&CreateWitness{request_id,payload_sha256:hex(&payload)})?;
   create_witnesses=create_witnesses.checked_add(1).ok_or_else(||invalid("legacy request count overflow"))?;
  }
 }
 let mut first=None;let mut last=None;let mut last_body=None;let mut audit_rows=0u64;
 {let mut q=conn.prepare(&format!("{EVENTS} ORDER BY revision,seq"))?;
  for row in q.query_map([id],decode_event)?{
   let(event,crew)=row?;
   let revision=event.revision.parse::<u64>().map_err(|_|invalid("legacy revision corrupt"))?;
   if !event.sequence.parse::<i64>().is_ok_and(|n|n>0)||revision!=audit_rows+1||crew!=summary.crew_id||!matches!(event.kind.as_str(),"created"|"updated"|"deleted")||event.body.as_ref().is_some_and(|b|b.len()>1048576){return Err(invalid("legacy audit ordering or source corrupt"))}
   if !matches!(event.actor.kind.as_str(),"human"|"session")||((event.actor.kind=="session")!=event.actor.session_id.is_some()){return Err(invalid("legacy audit actor corrupt"))}
   hash_row(&mut hash,"event",&event)?;audit_rows=revision;
   if first.is_none(){first=Some(event.clone())}
   if let Some(body)=&event.body{last_body=Some(body.clone())}last=Some(event);
  }
 }
 let first=first.ok_or_else(||invalid("legacy creation audit missing"))?;
 let last=last.ok_or_else(||invalid("legacy final audit missing"))?;
 if audit_rows.to_string()!=summary.revision||last.title!=summary.title||last.at!=summary.updated_at||serde_json::to_value(&last.actor)?!=serde_json::to_value(&summary.updated_by)?||((last.kind=="deleted")!=summary.deleted_at.is_some())||last.body.as_ref().is_some_and(|text|text!=&body){return Err(invalid("legacy audit/current source mismatch"))}
 if first.kind!="created"||first.at!=summary.created_at||serde_json::to_value(&first.actor)?!=serde_json::to_value(&summary.created_by)?||last_body.as_ref()!=Some(&body)||summary.deleted_at.as_ref().is_some_and(|at|at!=&last.at){return Err(invalid("legacy original source mismatch"))}
 Ok(LegacyDocument{summary,body,created_by_json,updated_by_json,audit_rows,create_witnesses,audit_source_sha256:hex(hash.finish().as_ref())})
}
pub fn page(conn:&Connection,after:Option<&str>,limit:u32)->Result<Vec<LegacyDocument>>{
 if limit==0||limit>32{return Err(invalid("legacy source page bound"))}
 let ids={let mut q=conn.prepare("SELECT id FROM documents WHERE (?1 IS NULL OR id>?1) ORDER BY id LIMIT ?2")?;q.query_map(params![after,limit],|r|r.get::<_,String>(0))?.collect::<std::result::Result<Vec<_>,_>>()?};
 ids.iter().map(|id|read(conn,id)).collect()
}
/// Separate lossless pages are consumed by the private native import path;
/// source verification hashes the complete ordered stream independently.
pub fn events(conn:&Connection,id:&str,after_revision:u64,limit:u32)->Result<Vec<DocumentEvent>>{
 if limit==0||limit>32||after_revision>i64::MAX as u64{return Err(invalid("legacy audit page bound"))}
 let mut q=conn.prepare(&format!("{EVENTS} AND revision>?2 ORDER BY revision,seq LIMIT ?3"))?;
 q.query_map(params![id,after_revision as i64,limit],decode_event)?.map(|r|r.map(|v|v.0).map_err(StoreError::from)).collect()
}
pub fn requests(conn:&Connection,id:&str,after:Option<&str>,limit:u32)->Result<Vec<CreateWitness>>{
 if limit==0||limit>128{return Err(invalid("legacy create-witness page bound"))}
 let mut q=conn.prepare("SELECT request_id,payload_digest FROM document_requests WHERE document_id=?1 AND (?2 IS NULL OR request_id>?2) ORDER BY request_id LIMIT ?3")?;
 q.query_map(params![id,after,limit],|r|Ok((r.get::<_,String>(0)?,r.get::<_,Vec<u8>>(1)?)))?.map(|r|{let(request_id,payload)=r?;if payload.len()!=32{return Err(invalid("legacy create digest corrupt"))}Ok(CreateWitness{request_id,payload_sha256:hex(&payload)})}).collect()
}
pub fn verify(conn:&Connection,expected:&SourceWitness)->Result<()>{if read(conn,&expected.document_id)?.witness()?!=*expected{return Err(invalid("legacy source changed since native import prepare"))}Ok(())}

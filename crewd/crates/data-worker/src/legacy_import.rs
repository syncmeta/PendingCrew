//! Protected pre-activation import, unreachable from public Apply or Agent SQL.
//! Each chunk has its own original UUID/native commit. Only the Core15 locked
//! coordinator can mark all sources verified and seal the prepared namespace.
use super::*;
fn sha(s:&str)->bool{s.len()==64&&s.bytes().all(|b|b.is_ascii_digit()||(b'a'..=b'f').contains(&b))}
impl Workspace{
 fn import_open(&self)->Result<(),String>{if self.engine.scalar("SELECT value FROM _pc_meta WHERE name='importMode'",&[])?.as_str()!=Some("open"){return Err("namespace is not an unactivated import".into())}Ok(())}
 pub fn import_batch(&self,batch:&ImportBatch)->Result<Value,String>{
  id(&batch.request_id)?;id(&batch.document_id)?;if !sha(&batch.source_sha256)||batch.objects.len()>64||batch.rows.len()>32||(batch.rows.is_empty()&&batch.objects.is_empty()&&!batch.finish_document){return Err("bounded original import chunk required".into())}
  let payload=hash(batch)?;let old=self.lookup(&batch.request_id)?;
  if old["status"]=="known"{if old["payloadSHA256"]!=payload{return Err(conflict("import UUID payload changed"))}return Ok(old)}if old["status"]=="unknown"{return Err("import UUID remains Unknown".into())}
  self.import_open()?;self.expected(&batch.expected_head)?;self.clean()?;self.engine.execute("BEGIN IMMEDIATE",&[])?;
  let result=(||{
   let old=self.engine.execute("SELECT source_sha,complete FROM _pc_import_documents WHERE id=?",&[text(&batch.document_id)])?;
   if let Some(row)=old["rows"].get(0){if row[0].as_str()!=Some(batch.source_sha256.as_str())||row[1].as_i64()!=Some(0){return Err("import source changed or document already finalized".into())}}
   else{self.engine.execute("INSERT INTO _pc_import_documents VALUES(?,?,0)",&[text(&batch.document_id),text(&batch.source_sha256)])?;}
   for object in &batch.objects{self.put_inner(&object.object,None,&object.captures,true)?;}
   for row in &batch.rows{
    if row.document_id!=batch.document_id||!matches!(row.kind.as_str(),"source"|"request"|"event")||row.key.is_empty()||row.key.len()>128||row.key.contains('\0'){return Err("invalid preserved legacy row identity".into())}
    if row.kind=="source"&&row.key!="current"{return Err("invalid original source row".into())}
    if row.kind=="request"{id(&row.key)?;}
    let encoded=serde_json::to_string(&row.payload).map_err(|_|"legacy row encoding")?;if encoded.len()>MAX_CONTENT_BYTES{return Err("legacy row exceeds lossless frame bound".into())}
    self.engine.execute("INSERT INTO _pc_legacy VALUES(?,?,?,?)",&[text(&row.document_id),text(&row.kind),text(&row.key),text(encoded)])?;
   }
   if batch.finish_document{
    // Finalization is not accepted without the original stable view and one
    // preserved complete current-source envelope. Core independently compares
    // every read-back object/row/hash before the authority cutover.
    if Self::get_from(&self.engine,"view",&batch.document_id)?.is_none(){return Err("imported original document view absent".into())}
    if self.engine.scalar("SELECT count(*) FROM _pc_legacy WHERE document_id=? AND kind='source' AND row_key='current'",&[text(&batch.document_id)])?.as_i64()!=Some(1){return Err("original document source envelope absent".into())}
    self.engine.execute("UPDATE _pc_import_documents SET complete=1 WHERE id=?",&[text(&batch.document_id)])?;
   }
   let result=json!({"documentId":batch.document_id,"sourceSHA256":batch.source_sha256,"objects":batch.objects.len(),"rows":batch.rows.len(),"complete":batch.finish_document,"branch":self.branch()?});self.remember(&batch.request_id,&payload,&result)?;
   let commit=self.commit(&batch.request_id,&payload,"Preserve original document authority source",&ResourceActor{kind:"system".into(),session_id:None})?;
   Ok(json!({"status":"known","requestId":batch.request_id,"payloadSHA256":payload,"commit":commit,"result":result}))
  })();if result.is_err()&&!self.engine.autocommit(){let _=self.engine.execute("ROLLBACK",&[]);}result
 }
 pub fn imported_rows(&self,document:&str,kind:&str,after:Option<&str>,limit:u32)->Result<Value,String>{
  id(document)?;if !matches!(kind,"source"|"request"|"event")||limit==0||limit>32{return Err("bounded original-source page required".into())}
  let rows=self.engine.execute("SELECT row_key,data FROM _pc_legacy WHERE document_id=? AND kind=? AND row_key>? ORDER BY row_key LIMIT ?",&[text(document),text(kind),text(after.unwrap_or("")),Cell::Integer((limit+1).to_string())])?;
  let values=rows["rows"].as_array().ok_or("invalid original-source page")?;let items=values.iter().take(limit as usize).map(|r|Ok(json!({"key":r[0],"payload":serde_json::from_str::<Value>(r[1].as_str().ok_or("invalid preserved row")?).map_err(|_|"invalid preserved source JSON")?}))).collect::<Result<Vec<_>,String>>()?;
  let next=if values.len()>limit as usize{items.last().map(|r|r["key"].clone())}else{None};Ok(json!({"items":items,"next":next}))
 }
 pub fn seal_import(&self,request:&str,head:&str,manifest:&str)->Result<Value,String>{
  id(request)?;if !sha(manifest){return Err("verified import manifest digest required".into())}
  let payload=hash(&(request,head,manifest))?;let old=self.lookup(request)?;if old["status"]=="known"{if old["payloadSHA256"]!=payload{return Err(conflict("seal UUID payload changed"))}return Ok(old)}
  self.import_open()?;self.expected(head)?;self.clean()?;
  if self.engine.scalar("SELECT count(*) FROM _pc_import_documents WHERE complete=0",&[])?.as_i64()!=Some(0){return Err("unverified import documents remain".into())}
  self.engine.execute("BEGIN IMMEDIATE",&[])?;
  let result=(||{
   self.engine.execute("UPDATE _pc_meta SET value='sealed' WHERE name='importMode'",&[])?;
   self.engine.execute("INSERT INTO _pc_meta VALUES('importManifestSHA256',?)",&[text(manifest)])?;
   let result=json!({"sealed":true,"manifestSHA256":manifest,"branch":self.branch()?});self.remember(request,&payload,&result)?;
   let commit=self.commit(request,&payload,"Seal verified original document authority",&ResourceActor{kind:"system".into(),session_id:None})?;
   Ok(json!({"status":"known","requestId":request,"payloadSHA256":payload,"commit":commit,"result":result}))
  })();if result.is_err()&&!self.engine.autocommit(){let _=self.engine.execute("ROLLBACK",&[]);}result
 }
}

impl Workspace{
 /// Native full-history backup, not stock SQLite or a current-table export.
 /// The controller precreates only the private sibling staging directory; the
 /// request supplies a UUID basename, never an arbitrary filesystem path.
 pub fn snapshot_file(&self,file_id:&str)->Result<Value,String>{
  use std::os::unix::fs::{MetadataExt,OpenOptionsExt};
  use std::io::Read;
  id(file_id)?;
  let root=self.path.parent().ok_or("native snapshot parent missing")?.join(".snapshots");let meta=std::fs::symlink_metadata(&root).map_err(|_|"native snapshot staging unavailable")?;
  if std::fs::canonicalize(&root).map_err(|_|"snapshot staging unknown")?!=root||!meta.is_dir()||meta.uid()!=unsafe{libc::geteuid()}||meta.mode()&0o777!=0o700{return Err("private canonical snapshot staging required".into())}
  let path=root.join(format!("{file_id}.dolt"));let reservation=std::fs::OpenOptions::new().read(true).write(true).create_new(true).mode(0o600).custom_flags(libc::O_NOFOLLOW|libc::O_CLOEXEC|libc::O_NONBLOCK).open(&path).map_err(|_|"fresh snapshot reservation refused")?;
  let original=reservation.metadata().map_err(|_|"snapshot inode unavailable")?;let head=self.head()?;let branch=self.branch()?;
  self.engine.execute("VACUUM INTO ?",&[text(path.to_str().ok_or("non UTF8 snapshot path")?)])?;
  let current=std::fs::symlink_metadata(&path).map_err(|_|"native snapshot disappeared")?;
  if !current.is_file()||current.nlink()!=1||current.dev()!=original.dev()||current.ino()!=original.ino()||self.head()?!=head||self.branch()?!=branch{return Err("native snapshot identity or source head changed".into())}
  reservation.sync_all().map_err(|_|"native snapshot sync failed")?;
  std::fs::File::open(&root).and_then(|p|p.sync_all()).map_err(|_|"native snapshot directory sync failed")?;
  let check=Self::open_existing(&path,&self.namespace,true)?;
  if check.engine.scalar("SELECT hash FROM dolt_branches WHERE name=?",&[text(&branch)])?.as_str()!=Some(head.as_str()){return Err("native snapshot branch/head mismatch".into())}drop(check);
  let mut reader=&reservation;let mut hash=ring::digest::Context::new(&ring::digest::SHA256);let mut buffer=[0;65536];let mut bytes=0u64;
  loop{let n=reader.read(&mut buffer).map_err(|_|"native snapshot read failed")?;if n==0{break}hash.update(&buffer[..n]);bytes=bytes.checked_add(n as u64).ok_or("native snapshot length overflow")?;}
  let end=reservation.metadata().map_err(|_|"native snapshot terminal metadata unavailable")?;
  if bytes!=current.len()||end.len()!=current.len()||end.mtime()!=current.mtime()||end.mtime_nsec()!=current.mtime_nsec()||end.ctime()!=current.ctime()||end.ctime_nsec()!=current.ctime_nsec(){return Err("native snapshot changed during proof".into())}
  Ok(json!({"fileId":file_id,"namespace":self.namespace,"branch":branch,"head":head,"bytes":bytes,"sha256":hash.finish().as_ref().iter().map(|b|format!("{b:02x}")).collect::<String>()}))
 }
}

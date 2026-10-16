//! This crate is a separate executable: never link it into the SQLite Core.
use crewd_data_protocol::{Cell, MAX_CONTENT_BYTES, MAX_LOGICAL_BYTES, MAX_QUERY_ROWS, ENGINE_VERSION};
use serde_json::{Value, json};
use std::{collections::BTreeSet,cell::Cell as Flag, ffi::{CStr, CString, c_char, c_int, c_void}, path::Path, ptr, time::{Duration, Instant}};

enum Db {}
enum Stmt {}
unsafe extern "C" {
    fn sqlite3_open_v2(path: *const c_char, db: *mut *mut Db, flags: c_int, vfs: *const c_char) -> c_int;
    fn sqlite3_close_v2(db: *mut Db) -> c_int;
    fn sqlite3_errmsg(db: *mut Db) -> *const c_char;
    fn sqlite3_prepare_v2(db: *mut Db, sql: *const c_char, n: c_int, stmt: *mut *mut Stmt, tail: *mut *const c_char) -> c_int;
    fn sqlite3_bind_null(s: *mut Stmt, n: c_int) -> c_int;
    fn sqlite3_bind_int64(s: *mut Stmt, n: c_int, v: i64) -> c_int;
    fn sqlite3_bind_double(s: *mut Stmt, n: c_int, v: f64) -> c_int;
    fn sqlite3_bind_text(s: *mut Stmt, n: c_int, v: *const c_char, len: c_int, destructor: isize) -> c_int;
    fn sqlite3_bind_blob(s: *mut Stmt, n: c_int, v: *const c_void, len: c_int, destructor: isize) -> c_int;
    fn sqlite3_bind_parameter_count(s: *mut Stmt) -> c_int;
    fn sqlite3_step(s: *mut Stmt) -> c_int;
    fn sqlite3_finalize(s: *mut Stmt) -> c_int;
    fn sqlite3_column_count(s: *mut Stmt) -> c_int;
    fn sqlite3_column_type(s: *mut Stmt, n: c_int) -> c_int;
    fn sqlite3_column_name(s: *mut Stmt, n: c_int) -> *const c_char;
    fn sqlite3_column_int64(s: *mut Stmt, n: c_int) -> i64;
    fn sqlite3_column_double(s: *mut Stmt, n: c_int) -> f64;
    fn sqlite3_column_text(s: *mut Stmt, n: c_int) -> *const u8;
    fn sqlite3_column_blob(s: *mut Stmt, n: c_int) -> *const c_void;
    fn sqlite3_column_bytes(s: *mut Stmt, n: c_int) -> c_int;
    fn sqlite3_stmt_readonly(s: *mut Stmt) -> c_int;
    fn sqlite3_get_autocommit(db: *mut Db) -> c_int;
    fn sqlite3_busy_timeout(db: *mut Db, ms: c_int) -> c_int;
    fn sqlite3_set_authorizer(db: *mut Db, callback: Option<unsafe extern "C" fn(*mut c_void,c_int,*const c_char,*const c_char,*const c_char,*const c_char)->c_int>, arg: *mut c_void) -> c_int;
    fn sqlite3_progress_handler(db: *mut Db, steps: c_int, callback: Option<unsafe extern "C" fn(*mut c_void)->c_int>, arg: *mut c_void);
}
#[derive(Clone, Copy, PartialEq)]
pub enum Policy { Internal, AgentWrite, AgentRead }
struct Guard { policy: Flag<Policy>, deadline: Flag<Option<Instant>>, request_deadline:Flag<Option<Instant>>,safe_functions:BTreeSet<String>,unsafe_modules:BTreeSet<String> }
unsafe fn text<'a>(p: *const c_char) -> &'a str {
    if p.is_null() { "" } else { unsafe { CStr::from_ptr(p) }.to_str().unwrap_or("") }
}
unsafe extern "C" fn authorize(p: *mut c_void, action:c_int, a:*const c_char, b:*const c_char, db:*const c_char, _: *const c_char) -> c_int {
    let g=unsafe {&*p.cast::<Guard>()};if g.policy.get()==Policy::Internal {return 0}
    let a=unsafe{text(a)};let b=unsafe{text(b)};let database=unsafe{text(db)};
    // Authorizer action values are the pinned bundled SQLite public C ABI.
    // No attach/extension/PRAGMA, virtual tables, explicit transaction control or remote functions.
    if [19,22,24,25,29,30,32].contains(&action) {return 1}
    if action==31 {
        let name=b.to_ascii_lowercase();
        if !g.safe_functions.contains(&name) || name.starts_with("dolt_") || name.starts_with("pragma_") || ["load_extension","readfile","writefile","edit"].contains(&name.as_str()) {return 1}
    }
    if g.unsafe_modules.contains(&a.to_ascii_lowercase()){return 1}
    if !database.is_empty() && database!="main" && database!="temp" {return 1}
    if ["_pc_","dolt_","pragma_"].iter().any(|prefix| a.to_ascii_lowercase().starts_with(prefix)) || (action!=20 && b.to_ascii_lowercase().starts_with("_pc_")) {return 1}
    if g.policy.get()==Policy::AgentRead && ![20,21,31,33].contains(&action) {return 1}
    0
}
unsafe extern "C" fn progress(p: *mut c_void) -> c_int {
    let g=unsafe {&*p.cast::<Guard>()}; i32::from(g.deadline.get().is_some_and(|t|Instant::now()>=t))
}
pub struct Engine { db:*mut Db, guard:Box<Guard> }
struct Statement(*mut Stmt);
impl Drop for Statement {fn drop(&mut self){unsafe{sqlite3_finalize(self.0);}}}
impl Drop for Engine {fn drop(&mut self){unsafe{sqlite3_close_v2(self.db);}}}
impl Engine {
    pub fn open(path:&Path, readonly:bool)->Result<Self,String> {
        // Forces the pinned crate's C engine into this worker, never the Core executable.
        let _=doltlite::version();
        let path=CString::new(path.to_str().ok_or("non UTF-8 engine path")?).map_err(|_|"NUL path")?;
        let mut db=ptr::null_mut();let rc=unsafe{sqlite3_open_v2(path.as_ptr(),&mut db,if readonly {1}else{2|4},ptr::null())};
        if rc!=0 {let e=if db.is_null(){"engine open failed".into()}else{unsafe{text(sqlite3_errmsg(db))}.to_owned()};if !db.is_null(){unsafe{sqlite3_close_v2(db);}}return Err(e)}
        let mut this=Self{db,guard:Box::new(Guard{policy:Flag::new(Policy::Internal),deadline:Flag::new(None),request_deadline:Flag::new(None),safe_functions:BTreeSet::new(),unsafe_modules:BTreeSet::new()})};
        let guard=(&mut *this.guard as *mut Guard).cast();
        unsafe{sqlite3_busy_timeout(db,250);sqlite3_set_authorizer(db,Some(authorize),guard);sqlite3_progress_handler(db,1000,Some(progress),guard);}
        if this.scalar("SELECT doltlite_engine()",&[])?.as_str()!=Some("prolly") {return Err("expected actual DoltLite prolly engine, stock SQLite refused".into())}
        if this.scalar("SELECT dolt_version()",&[])?.as_str()!=Some(format!("v{ENGINE_VERSION}").as_str()){return Err("actual native engine version does not match pinned package".into())}
        // SQLITE_INNOCUOUS is the pinned public C-ABI 0x200000 flag. Do not
        // authorize a newly registered extension function merely by its name.
        let functions=this.execute("PRAGMA function_list",&[])?;
        for row in functions["rows"].as_array().ok_or("function inventory unavailable")? {
            let name=row[0].as_str().ok_or("function inventory corrupt")?;
            let flags=row[5].as_i64().ok_or("function flags unavailable")?;
            if flags & 0x200000 !=0 {this.guard.safe_functions.insert(name.to_ascii_lowercase());}
        }
        let modules=this.execute("PRAGMA module_list",&[])?;
        for row in modules["rows"].as_array().ok_or("module inventory unavailable")? {
            let name=row[0].as_str().ok_or("module inventory corrupt")?.to_ascii_lowercase();
            if !["json_each","json_tree","generate_series"].contains(&name.as_str()) {this.guard.unsafe_modules.insert(name);}
        }
        Ok(this)
    }
    pub fn set_request_deadline(&self,deadline:Option<Instant>){self.guard.request_deadline.set(deadline)}
    pub fn autocommit(&self)->bool {unsafe{sqlite3_get_autocommit(self.db)!=0}}
    pub fn scalar(&self,sql:&str,args:&[Cell])->Result<Value,String> {
        let v=self.query(sql,args,Policy::Internal)?;
        v["rows"].get(0).and_then(|r|r.get(0)).cloned().ok_or_else(||"missing scalar".into())
    }
    pub fn execute(&self,sql:&str,args:&[Cell])->Result<Value,String>{self.query(sql,args,Policy::Internal)}
    pub fn query(&self,sql:&str,args:&[Cell],policy:Policy)->Result<Value,String> {
        if sql.len()>MAX_LOGICAL_BYTES || args.len()>1000 {return Err("bounded SQL input exceeded".into())}
        let now=Instant::now();let deadline=self.guard.request_deadline.get().map(|d|d.min(now+Duration::from_secs(5))).unwrap_or(now+Duration::from_secs(5));
        if deadline<=now{return Err("request deadline exhausted".into())}
        unsafe{sqlite3_busy_timeout(self.db,(deadline-now).as_millis().min(250) as c_int);}
        self.guard.policy.set(policy);self.guard.deadline.set(Some(deadline));
        let result=self.run(sql,args,policy,0,MAX_QUERY_ROWS);self.guard.policy.set(Policy::Internal);self.guard.deadline.set(None);result
    }
    pub fn query_page(&self,sql:&str,args:&[Cell],offset:u64,limit:u32)->Result<Value,String>{
        if sql.len()>MAX_LOGICAL_BYTES||args.len()>1000||offset>1_000_000||limit==0||limit as usize>MAX_QUERY_ROWS{return Err("bounded query page exceeded".into())}
        let now=Instant::now();let deadline=self.guard.request_deadline.get().map(|d|d.min(now+Duration::from_secs(5))).unwrap_or(now+Duration::from_secs(5));
        if deadline<=now{return Err("request deadline exhausted".into())}
        unsafe{sqlite3_busy_timeout(self.db,(deadline-now).as_millis().min(250) as c_int);}
        self.guard.policy.set(Policy::AgentRead);self.guard.deadline.set(Some(deadline));
        // Prepare the caller's original statement under the real authorizer;
        // do not concatenate it into an untrusted SQL wrapper to paginate.
        let result=self.run(sql,args,Policy::AgentRead,offset,limit as usize);self.guard.policy.set(Policy::Internal);self.guard.deadline.set(None);result
    }
    fn error(&self)->String {unsafe{text(sqlite3_errmsg(self.db))}.to_owned()}
    fn run(&self,sql:&str,args:&[Cell],policy:Policy,offset:u64,limit:usize)->Result<Value,String> {
        let sql=CString::new(sql).map_err(|_|"SQL NUL")?;let mut stmt=ptr::null_mut();let mut tail=ptr::null();
        if unsafe{sqlite3_prepare_v2(self.db,sql.as_ptr(),-1,&mut stmt,&mut tail)}!=0{return Err(self.error())}
        let stmt=Statement(stmt);if stmt.0.is_null(){return Err("empty SQL statement".into())}
        if !unsafe{text(tail)}.trim().is_empty(){return Err("one SQL statement per operation required".into())}
        if policy==Policy::AgentRead && unsafe{sqlite3_stmt_readonly(stmt.0)}==0{return Err("read query cannot mutate".into())}
        if unsafe{sqlite3_bind_parameter_count(stmt.0)} as usize!=args.len(){return Err("SQL parameter count mismatch".into())}
        for (n,v) in args.iter().enumerate() {
            let n=n as c_int+1;let rc=match v {
                Cell::Null=>unsafe{sqlite3_bind_null(stmt.0,n)},Cell::Integer(v)=>{let value=v.parse::<i64>().map_err(|_|"invalid integer")?;if value.to_string()!=*v{return Err("noncanonical integer".into())}unsafe{sqlite3_bind_int64(stmt.0,n,value)}},
                Cell::Real(v) if v.is_finite()=>unsafe{sqlite3_bind_double(stmt.0,n,*v)},Cell::Real(_)=>return Err("nonfinite number".into()),
                Cell::Text(v) if v.len()<=if policy==Policy::Internal {MAX_CONTENT_BYTES}else{MAX_LOGICAL_BYTES}=>unsafe{sqlite3_bind_text(stmt.0,n,v.as_ptr().cast(),v.len() as c_int,-1)},
                Cell::Text(_)=>return Err("text bound exceeded".into()),
                Cell::BlobHex(v)=>{if v.len()>MAX_LOGICAL_BYTES*2 || v.len()%2!=0 || !v.bytes().all(|b|b.is_ascii_digit()||(b'a'..=b'f').contains(&b)){return Err("invalid bounded blob hex".into())}let bytes=(0..v.len()).step_by(2).map(|i|u8::from_str_radix(&v[i..i+2],16)).collect::<Result<Vec<_>,_>>().map_err(|_|"invalid hex")?;unsafe{sqlite3_bind_blob(stmt.0,n,bytes.as_ptr().cast(),bytes.len() as c_int,-1)}}
            };if rc!=0{return Err(self.error())}
        }
        let count=unsafe{sqlite3_column_count(stmt.0)};let columns=(0..count).map(|n|unsafe{text(sqlite3_column_name(stmt.0,n))}.to_owned()).collect::<Vec<_>>();let mut rows=Vec::new();let mut bytes=0;let mut truncated=false;let mut skipped=0;
        loop {match unsafe{sqlite3_step(stmt.0)} {
            101=>return Ok(json!({"columns":columns,"rows":rows,"truncated":truncated})),
            100=>{if skipped<offset{skipped+=1;continue}if rows.len()==limit{truncated=true;if policy==Policy::AgentWrite {continue}else{return Ok(json!({"columns":columns,"rows":rows,"truncated":true}))}}
                let mut row=Vec::new();for n in 0..count {
                    let size=unsafe{sqlite3_column_bytes(stmt.0,n)};if size<0 || size as usize>if policy==Policy::Internal {MAX_CONTENT_BYTES}else{MAX_LOGICAL_BYTES}{return Err("SQL cell bound exceeded".into())}
                    bytes+=size as usize;if bytes>3*MAX_CONTENT_BYTES{return Err("SQL result byte bound exceeded".into())}
                    let mut value=match unsafe{sqlite3_column_type(stmt.0,n)} {
                        1=>serde_json::to_value(Cell::Integer(unsafe{sqlite3_column_int64(stmt.0,n)}.to_string())).unwrap(),2=>{let v=unsafe{sqlite3_column_double(stmt.0,n)};if !v.is_finite(){return Err("nonfinite result".into())}serde_json::to_value(Cell::Real(v)).map_err(|_|"nonfinite result")?},
                        3=>{let p=unsafe{sqlite3_column_text(stmt.0,n)};if p.is_null(){Value::Null}else{let data=unsafe{std::slice::from_raw_parts(p,size as usize)};Value::String(std::str::from_utf8(data).map_err(|_|"nonUTF8 text")?.into())}},
                        4=>{let p=unsafe{sqlite3_column_blob(stmt.0,n)};let data=if size==0{&[][..]}else{if p.is_null(){return Err("unreadable blob".into())}unsafe{std::slice::from_raw_parts(p.cast::<u8>(),size as usize)}};json!({"blobHex":data.iter().map(|b|format!("{b:02x}")).collect::<String>()})},
                        _=>Value::Null,
                    };
                    if policy==Policy::Internal {
                        if value["type"]=="integer" {value=Value::Number(value["value"].as_str().unwrap().parse::<i64>().unwrap().into())}
                        else if value["type"]=="real" {value=value["value"].clone()}
                    } else if value.is_null() {value=serde_json::to_value(Cell::Null).unwrap()}
                    else if value.is_string() {value=serde_json::to_value(Cell::Text(value.as_str().unwrap().into())).unwrap()}
                    else if let Some(hex)=value.get("blobHex").and_then(Value::as_str) {value=serde_json::to_value(Cell::BlobHex(hex.into())).unwrap()}
                    row.push(value);
                }rows.push(Value::Array(row));
            },_=>return Err(self.error())}}
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn actual_engine_commit_is_the_sql_transaction_publication_point() {
        let dir=tempfile::tempdir().unwrap();let path=dir.path().join("knowledge.dolt");
        let db=Engine::open(&path,false).unwrap();
        db.execute("CREATE TABLE ordinary(id INTEGER PRIMARY KEY, note TEXT)",&[]).unwrap();
        db.execute("BEGIN IMMEDIATE",&[]).unwrap();
        db.execute("INSERT INTO ordinary VALUES(1,'actual DoltLite only')",&[]).unwrap();
        let head=db.scalar("SELECT dolt_commit('-A','-m','private real engine first commit','--author','tester <test@pendingcrew.invalid>')",&[]).unwrap();
        assert!(head.as_str().is_some_and(|s|s.len()>=20));assert!(db.autocommit());
        assert!(db.execute("ROLLBACK",&[]).is_err());drop(db);
        let db=Engine::open(&path,false).unwrap();assert_eq!(db.scalar("SELECT note FROM ordinary WHERE id=1",&[]).unwrap(),"actual DoltLite only");
        assert_eq!(db.scalar("SELECT dolt_hashof('HEAD')",&[]).unwrap(),head);
    }
    #[test]
    fn actual_authorizer_protects_control_and_nested_paths_but_allows_free_schema() {
        let dir=tempfile::tempdir().unwrap();let db=Engine::open(&dir.path().join("knowledge.dolt"),false).unwrap();
        db.execute("CREATE TABLE _pc_control(secret TEXT)",&[]).unwrap();
        db.execute("INSERT INTO _pc_control VALUES('private synthetic control')",&[]).unwrap();
        db.query("CREATE TABLE business(id INTEGER PRIMARY KEY, data TEXT, count INTEGER)",&[],Policy::AgentWrite).unwrap();
        db.query("ALTER TABLE business ADD COLUMN optional TEXT",&[],Policy::AgentWrite).unwrap();
        db.query("INSERT INTO business VALUES(1,?,9223372036854775807,NULL)",&[Cell::Text("agent schema".into())],Policy::AgentWrite).unwrap();
        let rows=db.query("SELECT count,optional FROM business",&[],Policy::AgentRead).unwrap();
        assert_eq!(rows["rows"][0][0],json!({"type":"integer","value":"9223372036854775807"}));
        assert_eq!(rows["rows"][0][1],json!({"type":"null"}));
        for sql in ["SELECT * FROM _pc_control","UPDATE _pc_control SET secret='changed'","SELECT dolt_commit('-A','-m','not authorized')","SELECT dolt_reset('--hard','HEAD')","ATTACH ':memory:' AS escaped","PRAGMA writable_schema=ON","SELECT load_extension('unknown')","CREATE TABLE _PC_spoof(id)","SELECT * FROM dbstat","SELECT * FROM pragma_function_list","SELECT * FROM pragma_table_info('_pc_control')","SELECT * FROM dolt_log","CREATE TRIGGER control_trigger AFTER INSERT ON _pc_control BEGIN SELECT 1; END"] {
            assert!(db.query(sql,&[],Policy::AgentWrite).is_err(),"{sql}");
        }
        assert_eq!(db.query("SELECT length('abc'),json_extract('{\"n\":2}','$.n')",&[],Policy::AgentRead).unwrap()["rows"][0][0],json!({"type":"integer","value":"3"}));
        let view=db.query("CREATE VIEW covert AS SELECT * FROM _pc_control",&[],Policy::AgentWrite);
        if view.is_ok(){assert!(db.query("SELECT * FROM covert",&[],Policy::AgentRead).is_err());}
        let created=db.query("CREATE TRIGGER nested_escape AFTER INSERT ON business BEGIN UPDATE _pc_control SET secret='escaped'; END",&[],Policy::AgentWrite);
        if created.is_ok(){assert!(db.query("INSERT INTO business(id) VALUES(2)",&[],Policy::AgentWrite).is_err());}
        assert_eq!(db.scalar("SELECT secret FROM _pc_control",&[]).unwrap(),"private synthetic control");
        assert_eq!(db.scalar("SELECT count(*) FROM business",&[]).unwrap(),1);
        assert!(!dir.path().join("escaped").exists());
    }
}

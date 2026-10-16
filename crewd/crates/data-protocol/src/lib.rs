//! Private worker ABI. Contains no database implementation or principal admission.
use serde::{Deserialize, Serialize};
use serde_json::Value;
pub use crewd_proto::team_data::DataCell as Cell;
pub use crewd_proto::team_data::{DataObject,DataConflictResolution};
pub use crewd_proto::ResourceActor;
pub const IPC_VERSION:u32=1;
pub const MAX_FRAME_BYTES:usize=16*1024*1024;
pub const MAX_CONTENT_BYTES:usize=8*1024*1024;
pub const MAX_LOGICAL_BYTES:usize=1024*1024;
pub const MAX_QUERY_ROWS:usize=1000;
pub const ENGINE_VERSION:&str="0.51.0";
/// Captures are protected runtime facts, never user SQL and never unfiltered API output.
#[derive(Clone,Debug,Serialize,Deserialize)]
#[serde(rename_all="camelCase",deny_unknown_fields)]
pub struct Capture { pub anchor:String,pub namespace:String,pub kind:String,pub id:String,pub commit:String,pub revision:String,pub live:bool }
#[derive(Clone,Debug,Serialize,Deserialize)]
#[serde(tag="kind",rename_all="camelCase",rename_all_fields="camelCase",deny_unknown_fields)]
pub enum Mutation {
 PutObject { object:DataObject,expected_revision:Option<String>,captures:Vec<Capture> },
 Sql {sql:String,params:Vec<Cell>},
}
#[derive(Clone,Debug,Serialize,Deserialize)]
#[serde(rename_all="camelCase",deny_unknown_fields)]
pub struct WriteIntent { pub request_id:String,pub expected_head:String,pub author:ResourceActor,pub message:String,pub mutations:Vec<Mutation> }
#[derive(Clone,Debug,Serialize,Deserialize)]
#[serde(rename_all="camelCase",deny_unknown_fields)]
pub struct ImportedObject{pub object:DataObject,pub captures:Vec<Capture>}
#[derive(Clone,Debug,Serialize,Deserialize)]
#[serde(rename_all="camelCase",deny_unknown_fields)]
pub struct LegacyRow{pub document_id:String,pub kind:String,pub key:String,pub payload:Value}
#[derive(Clone,Debug,Serialize,Deserialize)]
#[serde(rename_all="camelCase",deny_unknown_fields)]
pub struct ImportBatch{pub request_id:String,pub expected_head:String,pub document_id:String,pub source_sha256:String,pub objects:Vec<ImportedObject>,pub rows:Vec<LegacyRow>,pub finish_document:bool}
#[derive(Clone,Debug,Serialize,Deserialize)]
#[serde(tag="operation",content="params",rename_all="camelCase",rename_all_fields="camelCase",deny_unknown_fields)]
pub enum Operation {
 Inspect, Lookup{request_id:String},
 Snapshot{file_id:String},
 Import(ImportBatch),
 ImportedRows{document_id:String,kind:String,after:Option<String>,limit:u32},
 SealImport{request_id:String,expected_head:String,manifest_sha256:String},
 Catalog{at_commit:String,after:Option<String>,limit:u32},
 Table{at_commit:String,name:String,after_key:Option<Vec<Cell>>,limit:u32},
 Branches{after:Option<String>,limit:u32},
 History{after:Option<String>,limit:u32},
 Object{kind:String,id:String,at_commit:Option<String>},
 Objects{kind:String,after:Option<String>,limit:u32,at_commit:Option<String>},
 Read{at_commit:Option<String>,sql:String,params:Vec<Cell>},
 ReadPage{at_commit:String,sql:String,params:Vec<Cell>,offset:u64,limit:u32},
 Apply(WriteIntent), FirstApply(WriteIntent),
 Log{limit:u32}, Diff{from:String,to:String,table:String,limit:u32},
 Branch{request_id:String,name:String,expected_head:String},
 Checkout{request_id:String,branch:String,expected_head:String},
 Merge{request_id:String,branch:String,expected_head:String,expected_source_head:String,author:ResourceActor,message:String,resolutions:Vec<DataConflictResolution>},
 Restore{request_id:String,target_commit:String,expected_head:String},
}
#[derive(Clone,Debug,Serialize,Deserialize)]
#[serde(rename_all="camelCase",deny_unknown_fields)]
pub struct Request{pub id:String,pub namespace:String,pub branch:String,pub operation:Operation}
#[derive(Clone,Debug,Serialize,Deserialize)]
#[serde(tag="status",content="result",rename_all="camelCase")]
pub enum Response{Ok(Value),Refused{code:String,message:String},Unknown{request_id:Option<String>,reason:String}}

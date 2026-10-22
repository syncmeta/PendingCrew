//! Native APNs notifications. None of these values grant execution permission.
use schemars::JsonSchema;
use serde::{Deserialize, Serialize};

#[derive(Debug,Clone,Copy,PartialEq,Eq,Serialize,Deserialize,JsonSchema)]
#[serde(rename_all="camelCase")]
pub enum PushEnvironment { Sandbox, Production }
#[derive(Debug,Clone,Copy,PartialEq,Eq,Serialize,Deserialize,JsonSchema)]
#[serde(rename_all="camelCase")]
pub enum NotificationKind { Done, Reported, Blocked, AskOpened, ReplyReady, ResponseFailed, HumanApprovalRequired, HumanMention }
#[derive(Debug,Clone,PartialEq,Eq,Serialize,Deserialize,JsonSchema)]
#[serde(tag="type",rename_all="camelCase",rename_all_fields="camelCase",deny_unknown_fields)]
pub enum NotificationSource {
    WorkEvent { event_seq:String, work_id:Option<String>, ask_id:Option<String>, entry_no:Option<String>, object_id:String },
    CodexTurn { session_id:String, thread_id:String, turn_id:String },
    ManagedResponse { session_id:String, runtime_id:String, request_id:String, prompt_id:String, receipt_revision:String },
    Decision { session_id:String, generation:String, decision_id:String },
    Message { message_id:String, message_seq:String, channel_id:String },
}
// rename_all on enum variants does not rename struct fields; make the wire exact.
// Fields are explicitly renamed by the separate serde rename_all_fields setting.
#[derive(Debug,Clone,PartialEq,Eq,Serialize,Deserialize,JsonSchema)]
#[serde(rename_all="camelCase",deny_unknown_fields)]
pub struct NotificationContext { pub action:String,pub authority_id:String,pub db_id:String,pub crew_id:String,pub kind:NotificationKind,pub source:NotificationSource }
#[derive(Clone,Serialize,Deserialize,JsonSchema)]
#[serde(rename_all="camelCase",deny_unknown_fields)]
pub struct NotificationProvider {pub team_id:String,pub key_id:String,pub environment:PushEnvironment,pub topic:String,pub private_key_pkcs8_base64:String}
impl std::fmt::Debug for NotificationProvider {fn fmt(&self,f:&mut std::fmt::Formatter<'_>)->std::fmt::Result{f.write_str("NotificationProvider([redacted])")}}
#[derive(Debug,Clone,Serialize,Deserialize,JsonSchema)]
#[serde(rename_all="camelCase",deny_unknown_fields)]
pub struct NotificationStatus { pub available:bool,pub enabled:bool,pub reason:Option<String>,pub revision:String,pub environment:Option<PushEnvironment>,pub topic:Option<String> }
#[derive(Debug,Clone,Serialize,Deserialize,JsonSchema)]
#[serde(rename_all="camelCase",deny_unknown_fields)]
pub struct NotificationConfigureParams {pub request_id:String,pub expected_revision:String,pub enabled:bool,pub provider:Option<NotificationProvider>}
#[derive(Clone,Serialize,Deserialize,JsonSchema)]
#[serde(rename_all="camelCase",deny_unknown_fields)]
pub struct NotificationRegisterParams {pub request_id:String,pub installation_id:String,pub expected_generation:String,pub device_token:String,pub environment:PushEnvironment,pub topic:String,pub crew_ids:Vec<String>,pub kinds:Vec<NotificationKind>}
impl std::fmt::Debug for NotificationRegisterParams {fn fmt(&self,f:&mut std::fmt::Formatter<'_>)->std::fmt::Result{f.write_str("NotificationRegisterParams([token redacted])")}}
#[derive(Debug,Clone,Serialize,Deserialize,JsonSchema)]
#[serde(rename_all="camelCase",deny_unknown_fields)]
pub struct NotificationRevokeParams {pub request_id:String,pub installation_id:String,pub expected_generation:String}
#[derive(Debug,Clone,Serialize,Deserialize,JsonSchema)]
#[serde(rename_all="camelCase",deny_unknown_fields)]
pub struct NotificationGetParams {pub installation_id:String}
#[derive(Debug,Clone,PartialEq,Eq,Serialize,Deserialize,JsonSchema)]
#[serde(rename_all="camelCase",deny_unknown_fields)]
pub struct NotificationInstallation {pub installation_id:String,pub generation:String,pub revoked:bool,pub environment:PushEnvironment,pub topic:String,pub crew_ids:Vec<String>,pub kinds:Vec<NotificationKind>}
#[derive(Debug,Clone,Serialize,Deserialize,JsonSchema)]
#[serde(rename_all="camelCase",deny_unknown_fields)]
pub struct NotificationLookupParams {pub request_id:String}
#[derive(Debug,Clone,Serialize,Deserialize,JsonSchema)]
#[serde(tag="kind",rename_all="camelCase",rename_all_fields="camelCase",deny_unknown_fields)]
pub enum NotificationRequestLookup {NotFound,Unknown{operation:String},Known{operation:String,result:serde_json::Value}}
#[derive(Debug,Clone,Serialize,Deserialize,JsonSchema)]
#[serde(rename_all="camelCase",deny_unknown_fields)]
pub struct NotificationDeliveryListParams {pub installation_id:String,pub after_delivery_id:Option<String>,pub limit:u32}
#[derive(Debug,Clone,Copy,PartialEq,Eq,Serialize,Deserialize,JsonSchema)]
#[serde(rename_all="camelCase")]
pub enum NotificationDeliveryState { Pending, Unknown, AcceptedByApns, Rejected, Suppressed }
#[derive(Debug,Clone,Serialize,Deserialize,JsonSchema)]
#[serde(rename_all="camelCase",deny_unknown_fields)]
pub struct NotificationDelivery {pub delivery_id:String,pub context:NotificationContext,pub state:NotificationDeliveryState,pub apns_id:Option<String>,pub reason:Option<String>,pub expires_at:String}
#[derive(Debug,Clone,Serialize,Deserialize,JsonSchema)]
#[serde(rename_all="camelCase",deny_unknown_fields)]
pub struct NotificationDeliveryPage {pub items:Vec<NotificationDelivery>,pub next_after_delivery_id:Option<String>}

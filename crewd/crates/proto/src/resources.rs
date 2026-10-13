//! Bounded persistent documents and independent per-view classifications.
use schemars::JsonSchema;
use serde::{Deserialize, Serialize};
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "camelCase")]
pub struct ResourceActor {
    pub kind: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub session_id: Option<String>,
}
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "camelCase")]
pub struct DocumentSummary {
    pub id: String,
    pub crew_id: String,
    pub title: String,
    pub revision: String,
    pub body_length: u64,
    pub created_by: ResourceActor,
    pub updated_by: ResourceActor,
    pub created_at: String,
    pub updated_at: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub deleted_at: Option<String>,
}
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "camelCase")]
pub struct DocumentDetail {
    #[serde(flatten)]
    pub document: DocumentSummary,
    pub body: String,
}
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "camelCase")]
pub struct DocumentCreateResult {
    pub document: DocumentSummary,
    pub request_id: String,
    pub reused: bool,
}
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "camelCase")]
pub struct DocumentPage {
    pub items: Vec<DocumentSummary>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub next: Option<String>,
}
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct DocumentCreateParams {
    pub request_id: String,
    pub crew_id: String,
    pub title: String,
    pub body: String,
}
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct DocumentGetParams {
    pub id: String,
}
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct DocumentListParams {
    pub crew_id: String,
    pub after: Option<String>,
    pub limit: Option<u32>,
    #[serde(default)]
    pub include_deleted: bool,
}
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct DocumentUpdateParams {
    pub id: String,
    pub expected_revision: String,
    pub title: String,
    pub body: String,
}
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct DocumentDeleteParams {
    pub id: String,
    pub expected_revision: String,
}
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "snake_case")]
pub enum FolderView {
    Messages,
    Todo,
    Documents,
    Progress,
}
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "camelCase")]
pub enum FolderObjectKind {
    Channel,
    Requirement,
    Work,
    HumanTask,
    Document,
}
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct FolderObjectRef {
    pub kind: FolderObjectKind,
    pub id: String,
}
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "camelCase")]
pub struct ViewFolder {
    pub id: String,
    pub name: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub parent_id: Option<String>,
    pub position: u32,
}
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "camelCase")]
pub struct FolderAssignment {
    pub object: FolderObjectRef,
    pub folder_id: String,
}
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "camelCase")]
pub struct FolderSnapshot {
    pub crew_id: String,
    pub view: FolderView,
    pub tree_revision: String,
    pub folders: Vec<ViewFolder>,
    pub assignments: Vec<FolderAssignment>,
}
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct FolderListParams {
    pub crew_id: String,
    pub view: FolderView,
}
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct FolderCreateParams {
    pub crew_id: String,
    pub view: FolderView,
    pub request_id: String,
    pub expected_tree_revision: String,
    pub name: String,
    pub parent_id: Option<String>,
}
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct FolderUpdateParams {
    pub crew_id: String,
    pub view: FolderView,
    pub id: String,
    pub expected_tree_revision: String,
    pub name: String,
    pub parent_id: Option<String>,
    pub position: u32,
}
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct FolderDeleteParams {
    pub crew_id: String,
    pub view: FolderView,
    pub id: String,
    pub expected_tree_revision: String,
}
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct FolderAssignParams {
    pub crew_id: String,
    pub view: FolderView,
    pub expected_tree_revision: String,
    pub object: FolderObjectRef,
    pub folder_id: Option<String>,
}
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "camelCase")]
pub struct DocumentChanged {
    pub crew_id: String,
    pub id: String,
    pub revision: String,
    pub deleted: bool,
}
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "camelCase")]
pub struct FolderChanged {
    pub crew_id: String,
    pub view: FolderView,
    pub tree_revision: String,
}

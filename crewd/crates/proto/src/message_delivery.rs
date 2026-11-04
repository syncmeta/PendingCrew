//! Scoped observations of actual message routing and retained queue evidence.
use schemars::JsonSchema;
use serde::{Deserialize, Serialize};

pub const EXPLAIN: &str = "message.deliveryExplain";

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct MessageDeliveryParams {
    pub crew_id: String,
    pub channel_id: String,
    pub message_id: String,
}
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "snake_case")]
pub enum MessageWakeReason {
    ExplicitMention,
    ReplyToSender,
    DirectedUnknown,
    DmPeer,
    Subscription,
    JevSelected,
    ChannelOwnerFallback,
    CrewCaptainFallback,
}
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "snake_case")]
pub enum MessageWakePath {
    Live,
    Jev,
    Backlog,
}
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "snake_case")]
pub enum MessageDeliveryRouting {
    Unknown,
    NoEligibleTarget,
    TargetsRecorded,
}
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "snake_case")]
pub enum MessageDeliveryState {
    Queued,
    Delivering,
    DeliveredOrAlreadyRead,
    Dropped,
    Unknown,
}
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct MessageDeliveryItem {
    #[serde(skip_serializing_if = "Option::is_none")]
    pub session_id: Option<String>,
    pub priority: crate::orchestra::WakePriorityView,
    pub path: MessageWakePath,
    pub reasons: Vec<MessageWakeReason>,
    pub state: MessageDeliveryState,
}
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct MessageDeliveryView {
    pub crew_id: String,
    pub channel_id: String,
    pub message_id: String,
    pub routing: MessageDeliveryRouting,
    pub items: Vec<MessageDeliveryItem>,
    pub truncated: bool,
}

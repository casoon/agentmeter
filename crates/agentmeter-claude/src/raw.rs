//! Weakly typed first stage: only the fields the second stage needs, everything else ignored.

use serde::Deserialize;
use serde::de::IgnoredAny;
use serde_json::value::RawValue;
use std::collections::HashMap;

#[derive(Deserialize)]
pub struct RawLine<'a> {
    #[serde(rename = "type", default)]
    pub kind: Option<String>,
    #[serde(default)]
    pub timestamp: Option<String>,
    #[serde(rename = "sessionId", default)]
    pub session_id: Option<String>,
    #[serde(rename = "requestId", default)]
    pub request_id: Option<String>,
    #[serde(rename = "isSidechain", default)]
    pub is_sidechain: Option<bool>,
    #[serde(default)]
    pub uuid: Option<String>,
    #[serde(rename = "parentUuid", default)]
    pub parent_uuid: Option<String>,
    #[serde(default)]
    pub version: Option<String>,
    /// Kept unparsed; message content is never looked at, and only assistant lines are opened.
    #[serde(default, borrow)]
    pub message: Option<&'a RawValue>,
    #[serde(rename = "compactMetadata", default, borrow)]
    pub compact_metadata: Option<&'a RawValue>,
}

#[derive(Deserialize)]
pub struct RawMessage {
    #[serde(default)]
    pub id: Option<String>,
    #[serde(default)]
    pub model: Option<String>,
    #[serde(default)]
    pub usage: Option<RawUsage>,
}

#[derive(Deserialize)]
pub struct RawUsage {
    #[serde(default)]
    pub input_tokens: Option<u64>,
    #[serde(default)]
    pub output_tokens: Option<u64>,
    #[serde(default)]
    pub cache_read_input_tokens: Option<u64>,
    #[serde(default)]
    pub cache_creation_input_tokens: Option<u64>,
    #[serde(default)]
    pub cache_creation: Option<RawCacheCreation>,
    /// Every other field, by name only.
    #[serde(flatten)]
    pub rest: HashMap<String, IgnoredAny>,
}

#[derive(Deserialize)]
pub struct RawCacheCreation {
    #[serde(rename = "ephemeral_5m_input_tokens", default)]
    pub five_minutes: Option<u64>,
    #[serde(rename = "ephemeral_1h_input_tokens", default)]
    pub one_hour: Option<u64>,
}

#[derive(Deserialize)]
pub struct RawCompact {
    #[serde(default)]
    pub trigger: Option<String>,
    #[serde(rename = "preTokens", default)]
    pub pre_tokens: Option<u64>,
    #[serde(rename = "postTokens", default)]
    pub post_tokens: Option<u64>,
}

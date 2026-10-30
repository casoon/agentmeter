//! Weakly typed first stage for Codex rollout lines.

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
    #[serde(default)]
    pub ordinal: Option<u64>,
    /// Kept unparsed; only the payloads that carry usage or session facts are opened.
    #[serde(default, borrow)]
    pub payload: Option<&'a RawValue>,
}

#[derive(Deserialize)]
pub struct RawMeta {
    #[serde(default)]
    pub id: Option<String>,
    #[serde(default)]
    pub cli_version: Option<String>,
    #[serde(default)]
    pub cwd: Option<String>,
    #[serde(default)]
    pub thread_source: Option<String>,
    #[serde(default)]
    pub forked_from_id: Option<String>,
}

#[derive(Deserialize)]
pub struct RawTurnContext {
    #[serde(default)]
    pub model: Option<String>,
}

#[derive(Deserialize)]
pub struct RawEvent {
    #[serde(rename = "type", default)]
    pub kind: Option<String>,
    #[serde(default)]
    pub info: Option<RawInfo>,
}

#[derive(Deserialize)]
pub struct RawInfo {
    #[serde(default)]
    pub total_token_usage: Option<RawTokens>,
    #[serde(default)]
    pub last_token_usage: Option<RawTokens>,
    #[serde(default)]
    pub model_context_window: Option<u64>,
}

#[derive(Deserialize, Default)]
pub struct RawTokens {
    #[serde(default)]
    pub input_tokens: Option<u64>,
    #[serde(default)]
    pub cached_input_tokens: Option<u64>,
    #[serde(default)]
    pub cache_write_input_tokens: Option<u64>,
    #[serde(default)]
    pub output_tokens: Option<u64>,
    /// Every other field, by name only (`reasoning_output_tokens` and `total_tokens` are known).
    #[serde(flatten)]
    pub rest: HashMap<String, IgnoredAny>,
}

#[derive(Deserialize)]
pub struct RawCompacted {
    #[serde(default)]
    pub compaction_response_id: Option<String>,
}

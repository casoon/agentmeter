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
    #[serde(default)]
    pub entrypoint: Option<String>,
    #[serde(default)]
    pub cwd: Option<String>,
    /// Kept unparsed; message content is never looked at, and only assistant lines are opened.
    #[serde(default, borrow)]
    pub message: Option<&'a RawValue>,
    #[serde(rename = "compactMetadata", default, borrow)]
    pub compact_metadata: Option<&'a RawValue>,
    /// Structured result of a tool call on a user line; only the run figures of an agent are read.
    #[serde(rename = "toolUseResult", default, borrow)]
    pub tool_use_result: Option<&'a RawValue>,
    #[serde(default, borrow)]
    pub attachment: Option<&'a RawValue>,
}

/// The run figures in the result of a delegated agent that ran in the foreground.
#[derive(Deserialize)]
pub struct RawAgentResult {
    #[serde(rename = "agentId", default)]
    pub agent_id: Option<String>,
    #[serde(rename = "totalDurationMs", default)]
    pub total_duration_ms: Option<u64>,
}

/// An attachment line; of interest is the notice that a background agent finished.
#[derive(Deserialize)]
pub struct RawAttachment<'a> {
    #[serde(rename = "type", default)]
    pub kind: Option<String>,
    #[serde(rename = "commandMode", default)]
    pub command_mode: Option<String>,
    #[serde(default)]
    pub timestamp: Option<String>,
    #[serde(default)]
    pub usage: Option<RawRunUsage>,
    /// The notice text, kept unparsed; only the task id in it is looked for, and only when tool
    /// calls are read.
    #[serde(default, borrow)]
    pub prompt: Option<&'a RawValue>,
}

#[derive(Deserialize)]
pub struct RawRunUsage {
    #[serde(rename = "durationMs", default)]
    pub duration_ms: Option<u64>,
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

/// Message content, opened only for `--tool-calls`.
#[derive(Deserialize)]
pub struct RawToolMessage {
    #[serde(default)]
    pub content: ToolContent,
}

/// A message's content is a string (plain text) or a list of blocks.
#[derive(Deserialize, Default)]
#[serde(untagged)]
pub enum ToolContent {
    Blocks(Vec<RawBlock>),
    #[default]
    Other,
}

#[derive(Deserialize)]
pub struct RawBlock {
    #[serde(rename = "type", default)]
    pub kind: Option<String>,
    #[serde(default)]
    pub id: Option<String>,
    #[serde(default)]
    pub name: Option<String>,
    /// Parameters of a call; held in memory only while the line is read.
    #[serde(default)]
    pub input: Option<serde_json::Map<String, serde_json::Value>>,
    #[serde(default)]
    pub tool_use_id: Option<String>,
    #[serde(default)]
    pub is_error: Option<bool>,
    #[serde(default)]
    pub content: Option<ResultContent>,
}

/// A result's content: text or a list of blocks.
#[derive(Deserialize)]
#[serde(untagged)]
pub enum ResultContent {
    Text(String),
    Blocks(Vec<ResultBlock>),
    Other(IgnoredAny),
}

#[derive(Deserialize)]
pub struct ResultBlock {
    #[serde(default)]
    pub text: Option<String>,
}

impl ResultContent {
    /// Bytes of the text; images and other blocks add nothing.
    pub fn text_bytes(&self) -> u64 {
        match self {
            ResultContent::Other(_) => 0,
            ResultContent::Text(t) => t.len() as u64,
            ResultContent::Blocks(b) => b
                .iter()
                .map(|b| b.text.as_deref().map_or(0, str::len) as u64)
                .sum(),
        }
    }
}

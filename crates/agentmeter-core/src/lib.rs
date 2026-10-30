//! Normalised usage schema, aggregation and finding rules.
//!
//! Adapters turn an agent's session logs into a [`Dataset`]; everything in this crate works on
//! that and never touches a file.

pub mod check;
pub mod findings;
pub mod format;
pub mod model;
pub mod report;
pub mod time;

pub use findings::{Evaluation, Finding, Level, evaluate};
pub use model::{Compaction, DataQuality, Dataset, ReadStats, Request, SessionStart};

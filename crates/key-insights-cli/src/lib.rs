//! Deterministic analysis primitives shared by the CLI and its tests.

mod event;
mod validator;

pub use event::{Event, Mode, SequenceMode};
pub use validator::{
    MAX_EVENT_LINE_BYTES, ValidationError, ValidationErrorKind, ValidationSummary, validate_jsonl,
};

/// Version of the collector/analyzer event contract.
pub const SCHEMA_VERSION: u32 = 1;

/// Sensitive data collection switches.
///
/// Defaults are deliberately conservative. Future configuration loading must
/// preserve these values unless the user explicitly opts in.
#[derive(Debug, Default, Clone, Copy, PartialEq, Eq)]
pub struct PrivacyPolicy {
    pub raw_keylog: bool,
    pub capture_insert_text: bool,
    pub capture_command_text: bool,
    pub capture_search_text: bool,
    pub store_file_paths: bool,
}

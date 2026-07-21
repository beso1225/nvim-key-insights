use serde::Deserialize;

#[derive(Debug, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum Mode {
    Normal,
    Visual,
    OperatorPending,
    Insert,
    Command,
    Search,
    Other,
}

#[derive(Debug, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum SequenceMode {
    Normal,
    Visual,
    OperatorPending,
}

/// One privacy-sanitized collector event.
///
/// Unknown fields are rejected so accidental additions such as `text`, `path`,
/// `command`, or `search` fail closed at the analyzer boundary.
#[derive(Debug, Deserialize, PartialEq, Eq)]
#[serde(tag = "event_type", rename_all = "snake_case", deny_unknown_fields)]
pub enum Event {
    SessionStart {
        schema_version: u32,
        session_id: String,
        elapsed_ms: u64,
        #[serde(default)]
        project_id: Option<String>,
    },
    SessionEnd {
        schema_version: u32,
        session_id: String,
        elapsed_ms: u64,
    },
    KeySequence {
        schema_version: u32,
        session_id: String,
        elapsed_ms: u64,
        mode: SequenceMode,
        keys: Vec<String>,
        duration_ms: u64,
    },
    TextRun {
        schema_version: u32,
        session_id: String,
        elapsed_ms: u64,
        key_count: u32,
        duration_ms: u64,
    },
    ModeTransition {
        schema_version: u32,
        session_id: String,
        elapsed_ms: u64,
        from: Mode,
        to: Mode,
    },
    MappingUse {
        schema_version: u32,
        session_id: String,
        elapsed_ms: u64,
        mode: SequenceMode,
        mapping_id: String,
        typed_keys: Vec<String>,
    },
}

impl Event {
    pub(crate) fn schema_version(&self) -> u32 {
        match self {
            Self::SessionStart { schema_version, .. }
            | Self::SessionEnd { schema_version, .. }
            | Self::KeySequence { schema_version, .. }
            | Self::TextRun { schema_version, .. }
            | Self::ModeTransition { schema_version, .. }
            | Self::MappingUse { schema_version, .. } => *schema_version,
        }
    }

    pub(crate) fn session_id(&self) -> &str {
        match self {
            Self::SessionStart { session_id, .. }
            | Self::SessionEnd { session_id, .. }
            | Self::KeySequence { session_id, .. }
            | Self::TextRun { session_id, .. }
            | Self::ModeTransition { session_id, .. }
            | Self::MappingUse { session_id, .. } => session_id,
        }
    }

    pub(crate) fn elapsed_ms(&self) -> u64 {
        match self {
            Self::SessionStart { elapsed_ms, .. }
            | Self::SessionEnd { elapsed_ms, .. }
            | Self::KeySequence { elapsed_ms, .. }
            | Self::TextRun { elapsed_ms, .. }
            | Self::ModeTransition { elapsed_ms, .. }
            | Self::MappingUse { elapsed_ms, .. } => *elapsed_ms,
        }
    }
}

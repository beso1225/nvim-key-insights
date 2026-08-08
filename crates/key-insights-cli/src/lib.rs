//! Deterministic analysis primitives shared by the CLI and its tests.

mod analyzer;
mod ergonomics;
mod event;
mod keymap_snapshot;
mod validator;

pub use analyzer::{
    AnalysisError, AnalysisInputsError, AnalysisSummary, KeyCount, MAX_DISTINCT_ITEMS,
    MAX_RANKED_ITEMS, MAX_RETAINED_TOKEN_BYTES, MappingAttribution, MappingAttributionEntry,
    MappingAttributionStatus, MappingCollision, MappingCount, ModeStats, RepeatedKeyStats,
    analyze_jsonl, analyze_jsonl_inputs, analyze_jsonl_inputs_with_snapshot,
    analyze_jsonl_with_snapshot, render_markdown, render_summary_json,
};
pub use ergonomics::{
    CandidateGuard, ERGONOMICS_CONTRACT_VERSION, ErgonomicCandidate, ErgonomicDistributions,
    ErgonomicSummary, ErgonomicThresholds, HISTOGRAM_VERSION, HistogramBucket,
    MAX_ERGONOMIC_CANDIDATES, MIN_CANDIDATE_OBSERVATIONS, MIN_CANDIDATE_SEQUENCE_KEYS,
    MIN_CANDIDATE_SESSIONS,
};
pub use event::{Event, Mode, SequenceMode};
pub use keymap_snapshot::{
    KeymapSnapshot, MAX_SNAPSHOT_BYTES, MAX_SNAPSHOT_MAPPINGS, SNAPSHOT_VERSION, SnapshotError,
    SnapshotMapping, SnapshotMode, SnapshotScope, parse_keymap_snapshot,
};
pub use validator::{
    MAX_EVENT_LINE_BYTES, MAX_SESSION_ID_BYTES, MAX_SESSIONS_PER_LOG, ValidationError,
    ValidationErrorKind, ValidationSummary, validate_jsonl,
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

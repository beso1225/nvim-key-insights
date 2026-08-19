use std::{
    collections::BTreeSet,
    io::{self, Write},
};

use serde::Serialize;

use crate::{
    AnalysisSummary, CANDIDATE_KIND_VERSION, COUNTABLE_TOKEN_SET_VERSION,
    DIRECTIONAL_MOTION_TOKEN_SET_VERSION, ERGONOMICS_CONTRACT_VERSION, HISTOGRAM_VERSION,
    KeymapSnapshot, MAX_DISTINCT_ITEMS, MAX_ERGONOMIC_CANDIDATES, MAX_RANKED_ITEMS,
    MAX_SNAPSHOT_MAPPINGS, MappingAttributionStatus, OPERATION_TOKEN_SET_VERSION, SNAPSHOT_VERSION,
    SnapshotMapping, ergonomics, keymap_snapshot,
};

/// Version of the sanitized subprocess payload contract.
pub const CODEX_PAYLOAD_SCHEMA_VERSION: u32 = 1;
/// Hard upper bound for bytes sent to an optional Codex subprocess.
pub const MAX_CODEX_PAYLOAD_BYTES: usize = 256 * 1024;

const PURPOSE: &str = "analyze-neovim-usage";
const ACTION_KINDS: [&str; 4] = [
    "learn_existing",
    "add_mapping",
    "change_mapping",
    "no_change",
];

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum CodexPayloadError {
    Serialization,
    UnsupportedSummarySchema { found: u32 },
    UnsupportedSnapshotVersion { found: u32 },
    InvalidSummaryContract { field: &'static str },
    InvalidSnapshot,
    TooLarge { bytes: usize, maximum: usize },
}

impl std::fmt::Display for CodexPayloadError {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::Serialization => formatter.write_str("failed to serialize Codex payload"),
            Self::UnsupportedSummarySchema { found } => {
                write!(formatter, "unsupported summary schema version {found}")
            }
            Self::UnsupportedSnapshotVersion { found } => {
                write!(formatter, "unsupported keymap snapshot version {found}")
            }
            Self::InvalidSummaryContract { field } => {
                write!(formatter, "invalid sanitized summary field {field}")
            }
            Self::InvalidSnapshot => formatter.write_str("invalid sanitized keymap snapshot"),
            Self::TooLarge { bytes, maximum } => write!(
                formatter,
                "Codex payload is {bytes} bytes, exceeding the {maximum}-byte limit"
            ),
        }
    }
}

impl std::error::Error for CodexPayloadError {}

#[derive(Serialize)]
struct CodexPayload<'a> {
    payload_schema_version: u32,
    purpose: &'static str,
    instructions: CodexInstructions,
    summary: &'a AnalysisSummary,
    #[serde(skip_serializing_if = "Option::is_none")]
    keymap_snapshot: Option<CodexKeymapSnapshot<'a>>,
}

#[derive(Serialize)]
struct CodexInstructions {
    action_kinds: [&'static str; 4],
    evidence_required: bool,
    collision_check_required: bool,
    privacy_boundary: &'static str,
}

#[derive(Serialize)]
struct CodexKeymapSnapshot<'a> {
    snapshot_version: u32,
    mappings: &'a [SnapshotMapping],
}

/// Render the exact compact JSON payload that an optional Codex subprocess may receive.
///
/// The function accepts only an already aggregated summary and an optional parsed,
/// sanitized keymap snapshot. It has no path, raw-log, or report input by design.
pub fn render_codex_payload_json(
    summary: &AnalysisSummary,
    snapshot: Option<&KeymapSnapshot>,
) -> Result<String, CodexPayloadError> {
    if summary.schema_version != 3 {
        return Err(CodexPayloadError::UnsupportedSummarySchema {
            found: summary.schema_version,
        });
    }
    if let Some(snapshot) = snapshot
        && snapshot.snapshot_version != SNAPSHOT_VERSION
    {
        return Err(CodexPayloadError::UnsupportedSnapshotVersion {
            found: snapshot.snapshot_version,
        });
    }
    if let Some(snapshot) = snapshot {
        keymap_snapshot::validate_snapshot(snapshot)
            .map_err(|_| CodexPayloadError::InvalidSnapshot)?;
    }
    validate_summary(summary, snapshot)?;
    let payload = CodexPayload {
        payload_schema_version: CODEX_PAYLOAD_SCHEMA_VERSION,
        purpose: PURPOSE,
        instructions: CodexInstructions {
            action_kinds: ACTION_KINDS,
            evidence_required: true,
            collision_check_required: true,
            privacy_boundary: "Use only aggregate evidence and the optional sanitized keymap snapshot; do not request or infer raw input.",
        },
        summary,
        keymap_snapshot: snapshot.map(|value| CodexKeymapSnapshot {
            snapshot_version: value.snapshot_version,
            mappings: &value.mappings,
        }),
    };
    let mut writer = LimitedWriter::new(MAX_CODEX_PAYLOAD_BYTES);
    let mut serializer = serde_json::Serializer::new(&mut writer);
    payload.serialize(&mut serializer).map_err(|_error| {
        if writer.exceeded {
            CodexPayloadError::TooLarge {
                bytes: writer.attempted,
                maximum: MAX_CODEX_PAYLOAD_BYTES,
            }
        } else {
            CodexPayloadError::Serialization
        }
    })?;
    let bytes = writer.bytes;
    if bytes.len() > MAX_CODEX_PAYLOAD_BYTES {
        return Err(CodexPayloadError::TooLarge {
            bytes: bytes.len(),
            maximum: MAX_CODEX_PAYLOAD_BYTES,
        });
    }
    String::from_utf8(bytes).map_err(|_| CodexPayloadError::Serialization)
}

struct LimitedWriter {
    bytes: Vec<u8>,
    limit: usize,
    attempted: usize,
    exceeded: bool,
}

impl LimitedWriter {
    fn new(limit: usize) -> Self {
        Self {
            bytes: Vec::with_capacity(limit.min(8 * 1024)),
            limit,
            attempted: 0,
            exceeded: false,
        }
    }
}

impl Write for LimitedWriter {
    fn write(&mut self, bytes: &[u8]) -> io::Result<usize> {
        self.attempted = self.attempted.saturating_add(bytes.len());
        if self.bytes.len().saturating_add(bytes.len()) > self.limit {
            self.exceeded = true;
            return Err(io::Error::new(
                io::ErrorKind::WriteZero,
                "Codex payload size limit exceeded",
            ));
        }
        self.bytes.extend_from_slice(bytes);
        Ok(bytes.len())
    }

    fn flush(&mut self) -> io::Result<()> {
        Ok(())
    }
}

fn validate_summary(
    summary: &AnalysisSummary,
    snapshot: Option<&KeymapSnapshot>,
) -> Result<(), CodexPayloadError> {
    if summary.modes.len() > MAX_RANKED_ITEMS
        || summary.keys.len() > MAX_RANKED_ITEMS
        || summary.mappings.len() > MAX_RANKED_ITEMS
        || summary.repeated_keys.len() > MAX_RANKED_ITEMS
        || summary.ergonomics.candidates.len() > MAX_ERGONOMIC_CANDIDATES
    {
        return Err(invalid("summary.ranking_limits"));
    }
    if snapshot.is_some() != summary.mapping_attribution.is_some() {
        return Err(invalid("mapping_attribution.presence"));
    }
    if summary.ranking_limit != MAX_RANKED_ITEMS {
        return Err(invalid("ranking_limit"));
    }
    let ergonomics = &summary.ergonomics;
    if ergonomics.contract_version != ERGONOMICS_CONTRACT_VERSION {
        return Err(invalid("ergonomics.contract_version"));
    }
    if ergonomics.candidate_limit != MAX_ERGONOMIC_CANDIDATES
        || ergonomics.thresholds.minimum_candidate_sessions != crate::MIN_CANDIDATE_SESSIONS
        || ergonomics.thresholds.minimum_candidate_sequence_keys
            != crate::MIN_CANDIDATE_SEQUENCE_KEYS
        || ergonomics.thresholds.minimum_candidate_observations != crate::MIN_CANDIDATE_OBSERVATIONS
    {
        return Err(invalid("ergonomics.thresholds"));
    }
    if ergonomics.distributions.histogram_version != HISTOGRAM_VERSION
        || !valid_buckets(
            &ergonomics.distributions.session_duration_ms,
            &ergonomics::SESSION_DURATION_BUCKETS,
        )
        || !valid_buckets(
            &ergonomics.distributions.sequence_length_keys,
            &ergonomics::SEQUENCE_LENGTH_BUCKETS,
        )
        || !valid_buckets(
            &ergonomics.distributions.average_inter_key_latency_ms,
            &ergonomics::LATENCY_BUCKETS,
        )
    {
        return Err(invalid("ergonomics.distributions"));
    }
    if ergonomics.operations.token_set_version != OPERATION_TOKEN_SET_VERSION
        || ergonomics.count_prefixes.token_set_version != COUNTABLE_TOKEN_SET_VERSION
        || ergonomics.repeated_motions.token_set_version != DIRECTIONAL_MOTION_TOKEN_SET_VERSION
    {
        return Err(invalid("ergonomics.token_sets"));
    }
    if ergonomics.mapping_coverage.snapshot_version != snapshot.map(|value| value.snapshot_version)
    {
        return Err(invalid("ergonomics.mapping_coverage.snapshot_version"));
    }
    if let Some(snapshot) = snapshot {
        let coverage = &ergonomics.mapping_coverage;
        if coverage.total_snapshot_mappings != snapshot.mappings.len() as u64
            || coverage.observed_mappings + coverage.unobserved_mappings
                != coverage.total_snapshot_mappings
        {
            return Err(invalid("ergonomics.mapping_coverage.counts"));
        }
    }
    for mode in &summary.modes {
        if !valid_mode(&mode.mode) {
            return Err(invalid("modes.mode"));
        }
    }
    for key in &summary.keys {
        validate_token(&key.key, "keys.key")?;
    }
    for key in &summary.repeated_keys {
        validate_token(&key.key, "repeated_keys.key")?;
    }
    for mapping in &summary.mappings {
        validate_mapping_id(&mapping.mapping_id, "mappings.mapping_id")?;
    }
    for motion in &ergonomics.repeated_motions.items {
        if !matches!(motion.motion, "h" | "j" | "k" | "l" | "w" | "b" | "e") {
            return Err(invalid("ergonomics.repeated_motions.motion"));
        }
    }
    for transition in &ergonomics.mode_transitions {
        if !valid_mode(transition.from) || !valid_mode(transition.to) {
            return Err(invalid("ergonomics.mode_transitions"));
        }
    }
    for candidate in &ergonomics.candidates {
        if candidate.kind_version != CANDIDATE_KIND_VERSION
            || candidate.guard.required_sessions != crate::MIN_CANDIDATE_SESSIONS
            || candidate.guard.required_sequence_keys != crate::MIN_CANDIDATE_SEQUENCE_KEYS
            || candidate.guard.required_observations != crate::MIN_CANDIDATE_OBSERVATIONS
        {
            return Err(invalid("ergonomics.candidates.guard"));
        }
        match candidate.kind.as_str() {
            "repeated_motion" => {
                let Some(motion) = candidate.candidate_id.strip_prefix("repeated-motion-") else {
                    return Err(invalid("ergonomics.candidates.candidate_id"));
                };
                if !matches!(motion, "h" | "j" | "k" | "l" | "w" | "b" | "e") {
                    return Err(invalid("ergonomics.candidates.candidate_id"));
                }
                if candidate
                    .measurements
                    .keys()
                    .any(|key| key != "presses" && key != "runs")
                {
                    return Err(invalid("ergonomics.candidates.measurements"));
                }
            }
            "current_mapping_unobserved_in_sample" => {
                let Some(mapping_id) = candidate
                    .candidate_id
                    .strip_prefix("mapping-unobserved-v1:")
                else {
                    return Err(invalid("ergonomics.candidates.candidate_id"));
                };
                validate_mapping_id(mapping_id, "ergonomics.candidates.candidate_id")?;
                if snapshot.is_none_or(|value| {
                    !value
                        .mappings
                        .iter()
                        .any(|mapping| mapping.mapping_id == mapping_id)
                }) {
                    return Err(invalid("ergonomics.candidates.candidate_id"));
                }
                if candidate
                    .measurements
                    .keys()
                    .any(|key| key != "observed_uses" && key != "sampled_sessions")
                {
                    return Err(invalid("ergonomics.candidates.measurements"));
                }
            }
            _ => return Err(invalid("ergonomics.candidates.kind")),
        }
    }
    if let Some(attribution) = &summary.mapping_attribution {
        if snapshot.is_none() || attribution.snapshot_version != SNAPSHOT_VERSION {
            return Err(invalid("mapping_attribution.snapshot_version"));
        }
        let snapshot = snapshot.expect("presence checked above");
        if attribution.mappings.len() > MAX_SNAPSHOT_MAPPINGS + MAX_DISTINCT_ITEMS
            || attribution.collisions.len() > MAX_SNAPSHOT_MAPPINGS
            || attribution.mappings.len() < snapshot.mappings.len()
        {
            return Err(invalid("mapping_attribution.bounds"));
        }
        let mut attribution_ids = BTreeSet::new();
        for mapping in &attribution.mappings {
            validate_mapping_id(&mapping.mapping_id, "mapping_attribution.mapping_id")?;
            if !attribution_ids.insert(mapping.mapping_id.as_str()) {
                return Err(invalid("mapping_attribution.duplicate_mapping_id"));
            }

            if let Some(mode) = &mapping.mode
                && !valid_sequence_mode(mode)
            {
                return Err(invalid("mapping_attribution.mode"));
            }
            if let Some(scope) = &mapping.scope
                && !matches!(scope.as_str(), "buffer" | "global")
            {
                return Err(invalid("mapping_attribution.scope"));
            }
            if let Some(lhs) = &mapping.lhs {
                for token in lhs {
                    validate_token(token, "mapping_attribution.lhs")?;
                }
            }
            for collision in &mapping.collision_mapping_ids {
                validate_mapping_id(collision, "mapping_attribution.collision_mapping_ids")?;
                if !snapshot
                    .mappings
                    .iter()
                    .any(|candidate| candidate.mapping_id == *collision)
                {
                    return Err(invalid("mapping_attribution.collision_mapping_ids"));
                }
            }
            let snapshot_mapping = snapshot
                .mappings
                .iter()
                .find(|candidate| candidate.mapping_id == mapping.mapping_id);
            match (snapshot_mapping, mapping.status) {
                (Some(expected), MappingAttributionStatus::Observed)
                    if mapping.count > 0
                        && mapping.mode.as_deref() == Some(expected.mode.as_str())
                        && mapping.scope.as_deref() == Some(expected.scope.as_str())
                        && mapping.lhs.as_deref() == Some(expected.lhs.as_slice()) => {}
                (Some(expected), MappingAttributionStatus::UnobservedInSample)
                    if mapping.count == 0
                        && mapping.mode.as_deref() == Some(expected.mode.as_str())
                        && mapping.scope.as_deref() == Some(expected.scope.as_str())
                        && mapping.lhs.as_deref() == Some(expected.lhs.as_slice()) => {}
                (None, MappingAttributionStatus::ObservedNotInSnapshot)
                    if mapping.count > 0
                        && mapping.mode.is_none()
                        && mapping.scope.is_none()
                        && mapping.lhs.is_none() => {}
                _ => return Err(invalid("mapping_attribution.status")),
            }
        }
        for collision in &attribution.collisions {
            if collision.kind != "potential_buffer_shadowing"
                || !valid_sequence_mode(&collision.mode)
            {
                return Err(invalid("mapping_attribution.collisions"));
            }
            for token in &collision.lhs {
                validate_token(token, "mapping_attribution.collisions.lhs")?;
            }
            validate_mapping_id(
                &collision.global_mapping_id,
                "mapping_attribution.collisions.global_mapping_id",
            )?;
            validate_mapping_id(
                &collision.buffer_mapping_id,
                "mapping_attribution.collisions.buffer_mapping_id",
            )?;
            if !snapshot
                .mappings
                .iter()
                .any(|mapping| mapping.mapping_id == collision.global_mapping_id)
                || !snapshot
                    .mappings
                    .iter()
                    .any(|mapping| mapping.mapping_id == collision.buffer_mapping_id)
            {
                return Err(invalid("mapping_attribution.collisions.mapping_id"));
            }
            let Some(global) = snapshot
                .mappings
                .iter()
                .find(|mapping| mapping.mapping_id == collision.global_mapping_id)
            else {
                return Err(invalid("mapping_attribution.collisions.global_mapping_id"));
            };
            let Some(buffer) = snapshot
                .mappings
                .iter()
                .find(|mapping| mapping.mapping_id == collision.buffer_mapping_id)
            else {
                return Err(invalid("mapping_attribution.collisions.buffer_mapping_id"));
            };
            if global.mode.as_str() != collision.mode
                || buffer.mode != global.mode
                || global.scope.as_str() != "global"
                || buffer.scope.as_str() != "buffer"
                || global.lhs != collision.lhs
                || buffer.lhs != collision.lhs
            {
                return Err(invalid("mapping_attribution.collisions.tuple"));
            }
        }
    }
    Ok(())
}

fn valid_buckets(buckets: &[crate::HistogramBucket], expected: &[&str]) -> bool {
    buckets.len() == expected.len()
        && buckets
            .iter()
            .zip(expected)
            .all(|(bucket, expected)| bucket.bucket == *expected)
}

fn validate_token(token: &str, field: &'static str) -> Result<(), CodexPayloadError> {
    if !keymap_snapshot::is_canonical_token(token) {
        return Err(invalid(field));
    }
    Ok(())
}

fn validate_mapping_id(mapping_id: &str, field: &'static str) -> Result<(), CodexPayloadError> {
    let Some(hash) = mapping_id.strip_prefix("mapping-v1:") else {
        return Err(invalid(field));
    };
    if hash.len() != 64
        || !hash
            .bytes()
            .all(|byte| matches!(byte, b'0'..=b'9' | b'a'..=b'f'))
    {
        return Err(invalid(field));
    }
    Ok(())
}

fn valid_mode(mode: &str) -> bool {
    matches!(
        mode,
        "normal" | "visual" | "operator_pending" | "insert" | "command" | "search" | "other"
    )
}

fn valid_sequence_mode(mode: &str) -> bool {
    matches!(mode, "normal" | "visual" | "operator_pending")
}

fn invalid(field: &'static str) -> CodexPayloadError {
    CodexPayloadError::InvalidSummaryContract { field }
}

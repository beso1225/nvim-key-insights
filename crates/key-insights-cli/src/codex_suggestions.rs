use std::collections::BTreeSet;

use serde::Deserialize;

use crate::{AnalysisSummary, KeymapSnapshot, MAX_CODEX_PAYLOAD_BYTES};

/// Version of the structured response expected from the optional Codex step.
pub const CODEX_SUGGESTIONS_SCHEMA_VERSION: u32 = 1;
pub const MAX_CODEX_SUGGESTIONS: usize = 100;
pub const MAX_SUGGESTION_EVIDENCE: usize = 32;
pub const MAX_SUGGESTION_CONFLICTS: usize = 4096;
const MAX_JSON_DEPTH: usize = 128;

const MEASUREMENT_KEYS: &[&str] = &[
    "sessions",
    "events",
    "total_session_duration_ms",
    "key_sequences",
    "sequence_keys",
    "text_runs",
    "text_keys",
    "mode_transitions",
    "mapping_uses",
    "repeated_key_runs",
    "repeated_key_presses",
    "unique_keys",
    "unique_mappings",
    "unique_repeated_keys",
    "observed_mappings",
    "unobserved_mappings",
    "total_snapshot_mappings",
    "count_prefix_occurrences",
    "count_prefix_digit_presses",
    "session_duration_ms",
    "sequence_length_keys",
    "average_inter_key_latency_ms",
];

#[derive(Debug, Clone, PartialEq, Eq, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct CodexSuggestionDocument {
    pub schema_version: u32,
    pub suggestions: Vec<CodexSuggestion>,
}

/// A document that has been checked against the exact deterministic input
/// used for the Codex request. Raw parsed documents intentionally cannot be
/// passed to the Markdown renderer.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ValidatedCodexSuggestions {
    document: CodexSuggestionDocument,
}

impl ValidatedCodexSuggestions {
    pub fn as_document(&self) -> &CodexSuggestionDocument {
        &self.document
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct CodexSuggestion {
    pub action: SuggestionAction,
    pub title: String,
    pub rationale: String,
    pub evidence: Vec<SuggestionEvidence>,
    pub collision_check: CollisionCheck,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum SuggestionAction {
    LearnExisting,
    AddMapping,
    ChangeMapping,
    NoChange,
}

#[derive(Debug, Clone, PartialEq, Eq, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct SuggestionEvidence {
    pub metric: String,
    pub value: u64,
}

#[derive(Debug, Clone, PartialEq, Eq, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct CollisionCheck {
    pub checked: bool,
    pub conflicting_mapping_ids: Vec<String>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum CodexSuggestionError {
    InvalidJson,
    UnsupportedSchema { found: u32 },
    InvalidContract { field: &'static str },
    TooLarge { bytes: usize, maximum: usize },
}

impl std::fmt::Display for CodexSuggestionError {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::InvalidJson => formatter.write_str("Codex suggestions are not strict JSON"),
            Self::UnsupportedSchema { found } => {
                write!(
                    formatter,
                    "unsupported Codex suggestions schema version {found}"
                )
            }
            Self::InvalidContract { field } => {
                write!(formatter, "invalid Codex suggestions field {field}")
            }
            Self::TooLarge { bytes, maximum } => {
                write!(
                    formatter,
                    "Codex suggestions are {bytes} bytes, exceeding {maximum}"
                )
            }
        }
    }
}

impl std::error::Error for CodexSuggestionError {}

/// Parse and validate a Codex response before it can be rendered or persisted.
pub fn validate_codex_suggestions_json(
    bytes: &[u8],
) -> Result<CodexSuggestionDocument, CodexSuggestionError> {
    if bytes.len() > MAX_CODEX_PAYLOAD_BYTES {
        return Err(CodexSuggestionError::TooLarge {
            bytes: bytes.len(),
            maximum: MAX_CODEX_PAYLOAD_BYTES,
        });
    }
    match has_duplicate_object_keys(bytes) {
        Ok(true) => return Err(CodexSuggestionError::InvalidJson),
        Ok(false) => {}
        Err(()) => return Err(CodexSuggestionError::InvalidJson),
    }
    let document: CodexSuggestionDocument =
        serde_json::from_slice(bytes).map_err(|_| CodexSuggestionError::InvalidJson)?;
    validate_document(&document)?;
    Ok(document)
}

/// Validate suggestions against the exact deterministic summary and snapshot
/// that produced the Codex request. This is the only API that authorizes
/// evidence-bearing mapping proposals.
pub fn validate_codex_suggestions_json_for_summary(
    bytes: &[u8],
    summary: &AnalysisSummary,
    snapshot: Option<&KeymapSnapshot>,
) -> Result<ValidatedCodexSuggestions, CodexSuggestionError> {
    crate::codex_payload::render_codex_payload_json(summary, snapshot)
        .map_err(|_| invalid("summary"))?;
    if let Some(snapshot) = snapshot {
        crate::keymap_snapshot::validate_snapshot(snapshot)
            .map_err(|_| invalid("collision_check.snapshot"))?;
        let Some(attribution) = summary.mapping_attribution.as_ref() else {
            return Err(invalid("collision_check.snapshot"));
        };
        if attribution.snapshot_version != snapshot.snapshot_version {
            return Err(invalid("collision_check.snapshot"));
        }
        let snapshot_ids: BTreeSet<&str> = snapshot
            .mappings
            .iter()
            .map(|mapping| mapping.mapping_id.as_str())
            .collect();
        for mapping in &snapshot.mappings {
            if !attribution
                .mappings
                .iter()
                .any(|entry| entry.mapping_id == mapping.mapping_id)
            {
                return Err(invalid("collision_check.snapshot"));
            }
        }
        for entry in &attribution.mappings {
            if !matches!(
                entry.status,
                crate::MappingAttributionStatus::ObservedNotInSnapshot
            ) && !snapshot_ids.contains(entry.mapping_id.as_str())
            {
                return Err(invalid("collision_check.snapshot"));
            }
        }
    } else if summary.mapping_attribution.is_some() {
        return Err(invalid("collision_check.snapshot"));
    }
    let document = validate_codex_suggestions_json(bytes)?;
    for suggestion in &document.suggestions {
        if matches!(
            suggestion.action,
            SuggestionAction::AddMapping | SuggestionAction::ChangeMapping
        ) && snapshot.is_none()
        {
            return Err(invalid("collision_check.snapshot"));
        }
        for evidence in &suggestion.evidence {
            let Some(expected) = measurement_value(&evidence.metric, summary, snapshot) else {
                return Err(invalid("evidence.metric"));
            };
            if evidence.value != expected {
                return Err(invalid("evidence.value"));
            }
        }
        if let Some(snapshot) = snapshot {
            let attribution = summary
                .mapping_attribution
                .as_ref()
                .expect("validated above");
            let collision_ids: BTreeSet<&str> = attribution
                .collisions
                .iter()
                .flat_map(|collision| {
                    std::iter::once(collision.global_mapping_id.as_str())
                        .chain(std::iter::once(collision.buffer_mapping_id.as_str()))
                })
                .collect();
            if matches!(
                suggestion.action,
                SuggestionAction::AddMapping | SuggestionAction::ChangeMapping
            ) && suggestion
                .collision_check
                .conflicting_mapping_ids
                .iter()
                .map(String::as_str)
                .collect::<BTreeSet<_>>()
                != collision_ids
            {
                return Err(invalid("collision_check.conflicting_mapping_ids"));
            }
            for mapping_id in &suggestion.collision_check.conflicting_mapping_ids {
                if !snapshot
                    .mappings
                    .iter()
                    .any(|mapping| mapping.mapping_id == *mapping_id)
                    || !collision_ids.contains(mapping_id.as_str())
                {
                    return Err(invalid("collision_check.conflicting_mapping_ids"));
                }
            }
        } else if !suggestion
            .collision_check
            .conflicting_mapping_ids
            .is_empty()
        {
            return Err(invalid("collision_check.conflicting_mapping_ids"));
        }
    }
    Ok(ValidatedCodexSuggestions { document })
}

fn validate_document(document: &CodexSuggestionDocument) -> Result<(), CodexSuggestionError> {
    if document.schema_version != CODEX_SUGGESTIONS_SCHEMA_VERSION {
        return Err(CodexSuggestionError::UnsupportedSchema {
            found: document.schema_version,
        });
    }
    if document.suggestions.len() > MAX_CODEX_SUGGESTIONS {
        return Err(invalid("suggestions"));
    }
    for suggestion in &document.suggestions {
        validate_text(&suggestion.title, 256, "suggestion.title")?;
        validate_text(&suggestion.rationale, 4096, "suggestion.rationale")?;
        if suggestion.evidence.is_empty() || suggestion.evidence.len() > MAX_SUGGESTION_EVIDENCE {
            return Err(invalid("evidence"));
        }
        for evidence in &suggestion.evidence {
            if !MEASUREMENT_KEYS.contains(&evidence.metric.as_str()) {
                return Err(invalid("evidence.metric"));
            }
        }
        if !suggestion.collision_check.checked {
            return Err(invalid("collision_check.checked"));
        }
        if suggestion.collision_check.conflicting_mapping_ids.len() > MAX_SUGGESTION_CONFLICTS {
            return Err(invalid("collision_check.conflicting_mapping_ids"));
        }
        for mapping_id in &suggestion.collision_check.conflicting_mapping_ids {
            if !is_mapping_id(mapping_id) {
                return Err(invalid("collision_check.conflicting_mapping_ids"));
            }
        }
    }
    Ok(())
}

/// Render only a previously validated document into deterministic Markdown.
pub fn render_codex_suggestions_markdown(
    validated: &ValidatedCodexSuggestions,
) -> Result<String, CodexSuggestionError> {
    let document = &validated.document;
    validate_document(document)?;
    let mut output = String::from("# Codex suggestions\n\n");
    if document.suggestions.is_empty() {
        output.push_str("No suggestions were returned.\n");
        return Ok(output);
    }
    for (index, suggestion) in document.suggestions.iter().enumerate() {
        output.push_str(&format!(
            "## {}. {}\n\n",
            index + 1,
            escape_markdown(&suggestion.title)
        ));
        output.push_str("- **Action:** `");
        output.push_str(action_name(suggestion.action));
        output.push_str("`\n- **Rationale:** ");
        output.push_str(&escape_markdown(&suggestion.rationale));
        output.push_str("\n- **Evidence:**\n");
        for evidence in &suggestion.evidence {
            output.push_str("  - `");
            output.push_str(&evidence.metric);
            output.push_str("`: ");
            output.push_str(&evidence.value.to_string());
            output.push('\n');
        }
        output.push_str("- **Collision check:** passed");
        if suggestion
            .collision_check
            .conflicting_mapping_ids
            .is_empty()
        {
            output.push_str(" (no conflicting mappings reported)\n\n");
        } else {
            output.push_str("; conflicting mapping IDs: ");
            let ids = suggestion
                .collision_check
                .conflicting_mapping_ids
                .iter()
                .map(|mapping_id| format!("`{mapping_id}`"))
                .collect::<Vec<_>>()
                .join(", ");
            output.push_str(&ids);
            output.push_str("\n\n");
        }
    }
    Ok(output)
}

fn measurement_value(
    metric: &str,
    summary: &AnalysisSummary,
    _snapshot: Option<&KeymapSnapshot>,
) -> Option<u64> {
    let histogram_count = |buckets: &[crate::ergonomics::HistogramBucket]| {
        buckets
            .iter()
            .try_fold(0_u64, |total, bucket| total.checked_add(bucket.count))
    };
    match metric {
        "sessions" => Some(summary.sessions),
        "events" => Some(summary.events),
        "total_session_duration_ms" => Some(summary.total_session_duration_ms),
        "key_sequences" => Some(summary.key_sequences),
        "sequence_keys" => Some(summary.sequence_keys),
        "text_runs" => Some(summary.text_runs),
        "text_keys" => Some(summary.text_keys),
        "mode_transitions" => Some(summary.mode_transitions),
        "mapping_uses" => Some(summary.mapping_uses),
        "repeated_key_runs" => Some(summary.repeated_key_runs),
        "repeated_key_presses" => Some(summary.repeated_key_presses),
        "unique_keys" => Some(summary.unique_keys),
        "unique_mappings" => Some(summary.unique_mappings),
        "unique_repeated_keys" => Some(summary.unique_repeated_keys),
        "observed_mappings" => Some(summary.ergonomics.mapping_coverage.observed_mappings),
        "unobserved_mappings" => Some(summary.ergonomics.mapping_coverage.unobserved_mappings),
        "total_snapshot_mappings" => {
            Some(summary.ergonomics.mapping_coverage.total_snapshot_mappings)
        }
        "count_prefix_occurrences" => Some(summary.ergonomics.count_prefixes.occurrences),
        "count_prefix_digit_presses" => Some(summary.ergonomics.count_prefixes.digit_presses),
        "session_duration_ms" => {
            histogram_count(&summary.ergonomics.distributions.session_duration_ms)
        }
        "sequence_length_keys" => {
            histogram_count(&summary.ergonomics.distributions.sequence_length_keys)
        }
        "average_inter_key_latency_ms" => histogram_count(
            &summary
                .ergonomics
                .distributions
                .average_inter_key_latency_ms,
        ),
        _ => None,
    }
}

fn action_name(action: SuggestionAction) -> &'static str {
    match action {
        SuggestionAction::LearnExisting => "learn_existing",
        SuggestionAction::AddMapping => "add_mapping",
        SuggestionAction::ChangeMapping => "change_mapping",
        SuggestionAction::NoChange => "no_change",
    }
}

fn escape_markdown(value: &str) -> String {
    value
        .chars()
        .flat_map(|character| match character {
            '\\' | '`' | '*' | '_' | '[' | ']' | '#' | '>' | '<' | '&' => {
                vec!['\\', character]
            }
            character => vec![character],
        })
        .collect()
}

fn invalid(field: &'static str) -> CodexSuggestionError {
    CodexSuggestionError::InvalidContract { field }
}

fn validate_text(
    value: &str,
    maximum: usize,
    field: &'static str,
) -> Result<(), CodexSuggestionError> {
    let lower = value.to_ascii_lowercase();
    if value.is_empty()
        || value.len() > maximum
        || value.chars().any(|character| character.is_control())
        || value.chars().any(|character| {
            matches!(character, '\u{2028}' | '\u{2029}' | '\u{202a}'..='\u{202e}' | '\u{2066}'..='\u{2069}')
        })
        || contains_non_standalone_slash(value)
        || value.contains('\\')
        || lower.contains(".env")
        || lower.contains("secret")
        || lower.contains("credential")
        || lower.contains("password")
        || lower.contains("api_key")
        || lower.contains("raw_log")
        || lower.contains("session_id")
        || lower.contains("project_id")
        || lower.contains("file://")
    {
        return Err(invalid(field));
    }
    Ok(())
}

fn contains_non_standalone_slash(value: &str) -> bool {
    let characters = value.chars().collect::<Vec<_>>();
    characters.iter().enumerate().any(|(index, character)| {
        *character == '/'
            && (!is_left_search_key_boundary(
                index.checked_sub(1).map(|previous| characters[previous]),
            ) || !is_right_search_key_boundary(characters.get(index + 1).copied()))
    })
}

fn is_left_search_key_boundary(character: Option<char>) -> bool {
    character.is_none_or(|character| {
        character.is_whitespace() || matches!(character, '`' | '\'' | '"' | '(' | '[' | '{' | '<')
    })
}

fn is_right_search_key_boundary(character: Option<char>) -> bool {
    character.is_none_or(|character| {
        character.is_whitespace()
            || matches!(
                character,
                '`' | '\'' | '"' | ')' | ']' | '}' | '>' | ',' | '.' | ';' | ':' | '!' | '?'
            )
    })
}

fn has_duplicate_object_keys(bytes: &[u8]) -> Result<bool, ()> {
    let mut index = 0;
    let duplicate = scan_value(bytes, &mut index, 0)?;
    skip_whitespace(bytes, &mut index);
    if index != bytes.len() {
        return Err(());
    }
    Ok(duplicate)
}

fn scan_value(bytes: &[u8], index: &mut usize, depth: usize) -> Result<bool, ()> {
    if depth > MAX_JSON_DEPTH {
        return Err(());
    }
    skip_whitespace(bytes, index);
    match bytes.get(*index) {
        Some(b'{') => scan_object(bytes, index, depth + 1),
        Some(b'[') => {
            *index += 1;
            let mut duplicate = false;
            skip_whitespace(bytes, index);
            if bytes.get(*index) == Some(&b']') {
                *index += 1;
                return Ok(false);
            }
            loop {
                duplicate |= scan_value(bytes, index, depth + 1)?;
                skip_whitespace(bytes, index);
                match bytes.get(*index) {
                    Some(b',') => *index += 1,
                    Some(b']') => {
                        *index += 1;
                        return Ok(duplicate);
                    }
                    _ => return Err(()),
                }
            }
        }
        Some(b'"') => {
            scan_string(bytes, index)?;
            Ok(false)
        }
        Some(b't') if bytes.get(*index..*index + 4) == Some(b"true") => {
            *index += 4;
            Ok(false)
        }
        Some(b'f') if bytes.get(*index..*index + 5) == Some(b"false") => {
            *index += 5;
            Ok(false)
        }
        Some(b'n') if bytes.get(*index..*index + 4) == Some(b"null") => {
            *index += 4;
            Ok(false)
        }
        Some(b'-' | b'0'..=b'9') => {
            while bytes.get(*index).is_some_and(|byte| {
                !matches!(byte, b',' | b']' | b'}' | b' ' | b'\n' | b'\r' | b'\t')
            }) {
                *index += 1;
            }
            Ok(false)
        }
        _ => Err(()),
    }
}

fn scan_object(bytes: &[u8], index: &mut usize, depth: usize) -> Result<bool, ()> {
    *index += 1;
    let mut keys = BTreeSet::new();
    let mut duplicate = false;
    skip_whitespace(bytes, index);
    if bytes.get(*index) == Some(&b'}') {
        *index += 1;
        return Ok(false);
    }
    loop {
        skip_whitespace(bytes, index);
        let key = scan_string(bytes, index)?;
        duplicate |= !keys.insert(key);
        skip_whitespace(bytes, index);
        if bytes.get(*index) != Some(&b':') {
            return Err(());
        }
        *index += 1;
        duplicate |= scan_value(bytes, index, depth + 1)?;
        skip_whitespace(bytes, index);
        match bytes.get(*index) {
            Some(b',') => *index += 1,
            Some(b'}') => {
                *index += 1;
                return Ok(duplicate);
            }
            _ => return Err(()),
        }
    }
}

fn scan_string(bytes: &[u8], index: &mut usize) -> Result<String, ()> {
    let start = *index;
    if bytes.get(*index) != Some(&b'"') {
        return Err(());
    }
    *index += 1;
    let mut escaped = false;
    while let Some(byte) = bytes.get(*index) {
        *index += 1;
        if escaped {
            escaped = false;
        } else if *byte == b'\\' {
            escaped = true;
        } else if *byte == b'"' {
            return serde_json::from_slice(&bytes[start..*index]).map_err(|_| ());
        }
    }
    Err(())
}

fn skip_whitespace(bytes: &[u8], index: &mut usize) {
    while bytes
        .get(*index)
        .is_some_and(|byte| matches!(byte, b' ' | b'\n' | b'\r' | b'\t'))
    {
        *index += 1;
    }
}

fn is_mapping_id(value: &str) -> bool {
    let Some(hex) = value.strip_prefix("mapping-v1:") else {
        return false;
    };
    hex.len() == 64
        && hex
            .bytes()
            .all(|byte| byte.is_ascii_hexdigit() && !byte.is_ascii_uppercase())
}

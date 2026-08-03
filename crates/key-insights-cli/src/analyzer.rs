use std::{cmp::Reverse, collections::BTreeMap, fmt::Write, io::BufRead};

use serde::Serialize;
use unicode_general_category::{GeneralCategory, get_general_category};

use crate::{Event, SCHEMA_VERSION, SequenceMode, ValidationError, validator::JsonlValidator};

pub const MAX_RANKED_ITEMS: usize = 100;
pub const MAX_DISTINCT_ITEMS: usize = 4096;
pub const MAX_RETAINED_TOKEN_BYTES: usize = 1024 * 1024;

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum AnalysisError {
    Validation(ValidationError),
    NoSessions,
    TooManyDistinctKeys,
    TooManyDistinctMappings,
    TooManyDistinctRepeatedKeys,
    RetainedTokenBytesExceeded,
    SessionDurationOverflow,
}

impl std::fmt::Display for AnalysisError {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::Validation(error) => error.fmt(formatter),
            Self::NoSessions => formatter.write_str("analysis input contains no complete sessions"),
            Self::TooManyDistinctKeys => write!(
                formatter,
                "analysis input exceeds the distinct key limit of {MAX_DISTINCT_ITEMS}"
            ),
            Self::TooManyDistinctMappings => write!(
                formatter,
                "analysis input exceeds the distinct mapping limit of {MAX_DISTINCT_ITEMS}"
            ),
            Self::TooManyDistinctRepeatedKeys => write!(
                formatter,
                "analysis input exceeds the distinct repeated-key limit of {MAX_DISTINCT_ITEMS}"
            ),
            Self::RetainedTokenBytesExceeded => write!(
                formatter,
                "analysis input exceeds the retained token budget of {MAX_RETAINED_TOKEN_BYTES} bytes"
            ),
            Self::SessionDurationOverflow => {
                formatter.write_str("total session duration exceeds u64::MAX milliseconds")
            }
        }
    }
}

impl std::error::Error for AnalysisError {}

impl From<ValidationError> for AnalysisError {
    fn from(error: ValidationError) -> Self {
        Self::Validation(error)
    }
}

/// An analysis failure with the ordered input that caused it, when applicable.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct AnalysisInputsError {
    /// Zero-based index in the iterator passed to [`analyze_jsonl_inputs`].
    ///
    /// This is `None` when the iterator itself contained no inputs.
    pub input_index: Option<usize>,
    /// The underlying validation or deterministic analysis failure.
    pub error: AnalysisError,
}

impl std::fmt::Display for AnalysisInputsError {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self.input_index {
            Some(index) => write!(formatter, "analysis input {}: {}", index + 1, self.error),
            None => self.error.fmt(formatter),
        }
    }
}

impl std::error::Error for AnalysisInputsError {
    fn source(&self) -> Option<&(dyn std::error::Error + 'static)> {
        Some(&self.error)
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct AnalysisSummary {
    pub schema_version: u32,
    pub ranking_limit: usize,
    pub sessions: u64,
    pub events: u64,
    pub total_session_duration_ms: u64,
    pub key_sequences: u64,
    pub sequence_keys: u64,
    pub text_runs: u64,
    pub text_keys: u64,
    pub mode_transitions: u64,
    pub mapping_uses: u64,
    pub repeated_key_runs: u64,
    pub repeated_key_presses: u64,
    pub unique_keys: u64,
    pub unique_mappings: u64,
    pub unique_repeated_keys: u64,
    pub modes: Vec<ModeStats>,
    pub keys: Vec<KeyCount>,
    pub mappings: Vec<MappingCount>,
    pub repeated_keys: Vec<RepeatedKeyStats>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct ModeStats {
    pub mode: String,
    pub sequences: u64,
    pub keys: u64,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct KeyCount {
    pub key: String,
    pub count: u64,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct MappingCount {
    pub mapping_id: String,
    pub count: u64,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct RepeatedKeyStats {
    pub key: String,
    pub runs: u64,
    pub presses: u64,
}

#[derive(Default)]
struct ModeAccumulator {
    sequences: u64,
    keys: u64,
}

#[derive(Default)]
struct RepeatedAccumulator {
    runs: u64,
    presses: u64,
}

#[derive(Default)]
struct Accumulator {
    analysis_error: Option<AnalysisError>,
    retained_token_bytes: usize,
    total_session_duration_ms: u64,
    key_sequences: u64,
    sequence_keys: u64,
    text_runs: u64,
    text_keys: u64,
    mode_transitions: u64,
    mapping_uses: u64,
    repeated_key_runs: u64,
    repeated_key_presses: u64,
    modes: BTreeMap<String, ModeAccumulator>,
    keys: BTreeMap<String, u64>,
    mappings: BTreeMap<String, u64>,
    repeated_keys: BTreeMap<String, RepeatedAccumulator>,
}

pub fn analyze_jsonl<R: BufRead>(reader: R) -> Result<AnalysisSummary, AnalysisError> {
    analyze_jsonl_inputs(std::iter::once(reader)).map_err(|error| error.error)
}

/// Analyzes complete JSONL sources as one bounded deterministic dataset.
///
/// Every source must contain at least one complete session. Session identities
/// and analyzer resource limits are shared across the ordered input set.
pub fn analyze_jsonl_inputs<I, R>(readers: I) -> Result<AnalysisSummary, AnalysisInputsError>
where
    I: IntoIterator<Item = R>,
    R: BufRead,
{
    let mut accumulator = Accumulator::default();
    let mut analysis_error_input = None;
    let mut validator = JsonlValidator::new();
    for (input_index, reader) in readers.into_iter().enumerate() {
        let source_sessions = validator
            .consume(reader, |event| accumulator.observe(event))
            .map_err(|error| AnalysisInputsError {
                input_index: Some(input_index),
                error: AnalysisError::Validation(error),
            })?;
        if source_sessions == 0 {
            return Err(AnalysisInputsError {
                input_index: Some(input_index),
                error: AnalysisError::NoSessions,
            });
        }
        if accumulator.analysis_error.is_some() && analysis_error_input.is_none() {
            analysis_error_input = Some(input_index);
        }
    }
    let validation = validator.finish();
    if validation.sessions == 0 {
        return Err(AnalysisInputsError {
            input_index: None,
            error: AnalysisError::NoSessions,
        });
    }
    if let Some(error) = accumulator.analysis_error.take() {
        return Err(AnalysisInputsError {
            input_index: analysis_error_input,
            error,
        });
    }
    Ok(accumulator.finish(validation.sessions, validation.events))
}

impl Accumulator {
    fn observe(&mut self, event: &Event) {
        match event {
            Event::SessionEnd { elapsed_ms, .. } => {
                match self.total_session_duration_ms.checked_add(*elapsed_ms) {
                    Some(total) => self.total_session_duration_ms = total,
                    None => self.record_error(AnalysisError::SessionDurationOverflow),
                }
            }
            Event::KeySequence { mode, keys, .. } => {
                self.key_sequences = self.key_sequences.saturating_add(1);
                self.sequence_keys = self.sequence_keys.saturating_add(keys.len() as u64);
                let mode = sequence_mode_name(mode).to_owned();
                let stats = self.modes.entry(mode).or_default();
                stats.sequences = stats.sequences.saturating_add(1);
                stats.keys = stats.keys.saturating_add(keys.len() as u64);
                for key in keys {
                    match increment_bounded(&mut self.keys, &mut self.retained_token_bytes, key, 1)
                    {
                        Ok(()) => {}
                        Err(BoundedEntryError::TooManyItems) => {
                            self.record_error(AnalysisError::TooManyDistinctKeys);
                        }
                        Err(BoundedEntryError::TooManyBytes) => {
                            self.record_error(AnalysisError::RetainedTokenBytesExceeded);
                        }
                    }
                }
                self.observe_repeated_keys(keys);
            }
            Event::TextRun { key_count, .. } => {
                self.text_runs = self.text_runs.saturating_add(1);
                self.text_keys = self.text_keys.saturating_add(u64::from(*key_count));
            }
            Event::ModeTransition { .. } => {
                self.mode_transitions = self.mode_transitions.saturating_add(1);
            }
            Event::MappingUse { mapping_id, .. } => {
                self.mapping_uses = self.mapping_uses.saturating_add(1);
                match increment_bounded(
                    &mut self.mappings,
                    &mut self.retained_token_bytes,
                    mapping_id,
                    1,
                ) {
                    Ok(()) => {}
                    Err(BoundedEntryError::TooManyItems) => {
                        self.record_error(AnalysisError::TooManyDistinctMappings);
                    }
                    Err(BoundedEntryError::TooManyBytes) => {
                        self.record_error(AnalysisError::RetainedTokenBytesExceeded);
                    }
                }
            }
            Event::SessionStart { .. } => {}
        }
    }

    fn observe_repeated_keys(&mut self, keys: &[String]) {
        let mut index = 0;
        while index < keys.len() {
            let mut end = index + 1;
            while end < keys.len() && keys[end] == keys[index] {
                end += 1;
            }
            let presses = (end - index) as u64;
            if presses >= 2 {
                self.repeated_key_runs = self.repeated_key_runs.saturating_add(1);
                self.repeated_key_presses = self.repeated_key_presses.saturating_add(presses);
                match bounded_entry(
                    &mut self.repeated_keys,
                    &mut self.retained_token_bytes,
                    &keys[index],
                ) {
                    Ok(repeated) => {
                        repeated.runs = repeated.runs.saturating_add(1);
                        repeated.presses = repeated.presses.saturating_add(presses);
                    }
                    Err(BoundedEntryError::TooManyItems) => {
                        self.record_error(AnalysisError::TooManyDistinctRepeatedKeys);
                    }
                    Err(BoundedEntryError::TooManyBytes) => {
                        self.record_error(AnalysisError::RetainedTokenBytesExceeded);
                    }
                }
            }
            index = end;
        }
    }

    fn record_error(&mut self, error: AnalysisError) {
        if self.analysis_error.is_none() {
            self.analysis_error = Some(error);
        }
    }

    fn finish(self, sessions: u64, events: u64) -> AnalysisSummary {
        let unique_keys = self.keys.len() as u64;
        let unique_mappings = self.mappings.len() as u64;
        let unique_repeated_keys = self.repeated_keys.len() as u64;
        let modes = self
            .modes
            .into_iter()
            .map(|(mode, stats)| ModeStats {
                mode,
                sequences: stats.sequences,
                keys: stats.keys,
            })
            .collect();
        let keys = ranked(self.keys)
            .into_iter()
            .map(|(key, count)| KeyCount { key, count })
            .collect();
        let mappings = ranked(self.mappings)
            .into_iter()
            .map(|(mapping_id, count)| MappingCount { mapping_id, count })
            .collect();
        let mut repeated_keys: Vec<_> = self
            .repeated_keys
            .into_iter()
            .map(|(key, stats)| RepeatedKeyStats {
                key,
                runs: stats.runs,
                presses: stats.presses,
            })
            .collect();
        repeated_keys.sort_by_key(|stats| (Reverse(stats.runs), stats.key.clone()));
        repeated_keys.truncate(MAX_RANKED_ITEMS);

        AnalysisSummary {
            schema_version: SCHEMA_VERSION,
            ranking_limit: MAX_RANKED_ITEMS,
            sessions,
            events,
            total_session_duration_ms: self.total_session_duration_ms,
            key_sequences: self.key_sequences,
            sequence_keys: self.sequence_keys,
            text_runs: self.text_runs,
            text_keys: self.text_keys,
            mode_transitions: self.mode_transitions,
            mapping_uses: self.mapping_uses,
            repeated_key_runs: self.repeated_key_runs,
            repeated_key_presses: self.repeated_key_presses,
            unique_keys,
            unique_mappings,
            unique_repeated_keys,
            modes,
            keys,
            mappings,
            repeated_keys,
        }
    }
}

enum BoundedEntryError {
    TooManyItems,
    TooManyBytes,
}

fn increment_bounded(
    counts: &mut BTreeMap<String, u64>,
    retained_bytes: &mut usize,
    key: &str,
    amount: u64,
) -> Result<(), BoundedEntryError> {
    let count = bounded_entry(counts, retained_bytes, key)?;
    *count = count.saturating_add(amount);
    Ok(())
}

fn bounded_entry<'a, T: Default>(
    values: &'a mut BTreeMap<String, T>,
    retained_bytes: &mut usize,
    key: &str,
) -> Result<&'a mut T, BoundedEntryError> {
    if values.contains_key(key) {
        return Ok(values.get_mut(key).expect("existing key is retrievable"));
    }
    if values.len() >= MAX_DISTINCT_ITEMS {
        return Err(BoundedEntryError::TooManyItems);
    }
    let new_retained_bytes = retained_bytes
        .checked_add(key.len())
        .filter(|bytes| *bytes <= MAX_RETAINED_TOKEN_BYTES)
        .ok_or(BoundedEntryError::TooManyBytes)?;
    *retained_bytes = new_retained_bytes;
    Ok(values.entry(key.to_owned()).or_default())
}

fn ranked(counts: BTreeMap<String, u64>) -> Vec<(String, u64)> {
    let mut counts: Vec<_> = counts.into_iter().collect();
    counts.sort_by(|left, right| right.1.cmp(&left.1).then_with(|| left.0.cmp(&right.0)));
    counts.truncate(MAX_RANKED_ITEMS);
    counts
}

fn sequence_mode_name(mode: &SequenceMode) -> &'static str {
    match mode {
        SequenceMode::Normal => "normal",
        SequenceMode::Visual => "visual",
        SequenceMode::OperatorPending => "operator_pending",
    }
}

pub fn render_summary_json(summary: &AnalysisSummary) -> String {
    let mut output =
        serde_json::to_string_pretty(summary).expect("analysis summary is serializable");
    output.push('\n');
    output
}

pub fn render_markdown(summary: &AnalysisSummary) -> String {
    let mut output = String::new();
    writeln!(output, "# Neovim Key Insights\n").unwrap();
    writeln!(output, "## Overview\n").unwrap();
    writeln!(output, "- Sessions: {}", summary.sessions).unwrap();
    writeln!(output, "- Events: {}", summary.events).unwrap();
    writeln!(
        output,
        "- Total session duration: {} ms",
        summary.total_session_duration_ms
    )
    .unwrap();
    writeln!(output, "- Key sequences: {}", summary.key_sequences).unwrap();
    writeln!(output, "- Sequence keys: {}", summary.sequence_keys).unwrap();
    writeln!(output, "- Text runs: {}", summary.text_runs).unwrap();
    writeln!(output, "- Text keys: {}", summary.text_keys).unwrap();
    writeln!(output, "- Mode transitions: {}", summary.mode_transitions).unwrap();
    writeln!(output, "- Mapping uses: {}", summary.mapping_uses).unwrap();
    writeln!(
        output,
        "- Repeated key runs: {} ({} presses)\n",
        summary.repeated_key_runs, summary.repeated_key_presses
    )
    .unwrap();
    writeln!(
        output,
        "_Ranked tables show at most {} items._\n",
        summary.ranking_limit
    )
    .unwrap();

    render_modes(&mut output, summary);
    render_keys(&mut output, summary);
    render_mappings(&mut output, summary);
    render_repeated_keys(&mut output, summary);
    output
}

fn render_modes(output: &mut String, summary: &AnalysisSummary) {
    writeln!(output, "## Sequence modes\n").unwrap();
    if summary.modes.is_empty() {
        writeln!(output, "_No key sequences recorded._\n").unwrap();
        return;
    }
    writeln!(output, "| Mode | Sequences | Keys |").unwrap();
    writeln!(output, "| --- | ---: | ---: |").unwrap();
    for mode in &summary.modes {
        writeln!(
            output,
            "| {} | {} | {} |",
            mode.mode, mode.sequences, mode.keys
        )
        .unwrap();
    }
    output.push('\n');
}

fn render_keys(output: &mut String, summary: &AnalysisSummary) {
    writeln!(output, "## Frequent keys\n").unwrap();
    if summary.keys.is_empty() {
        writeln!(output, "_No sequence keys recorded._\n").unwrap();
        return;
    }
    writeln!(output, "| Key | Count |").unwrap();
    writeln!(output, "| --- | ---: |").unwrap();
    for key in &summary.keys {
        writeln!(output, "| {} | {} |", html_code(&key.key), key.count).unwrap();
    }
    output.push('\n');
}

fn render_mappings(output: &mut String, summary: &AnalysisSummary) {
    writeln!(output, "## Mapping usage\n").unwrap();
    if summary.mappings.is_empty() {
        writeln!(output, "_No mapping usage recorded._\n").unwrap();
        return;
    }
    writeln!(output, "| Mapping ID | Count |").unwrap();
    writeln!(output, "| --- | ---: |").unwrap();
    for mapping in &summary.mappings {
        writeln!(
            output,
            "| {} | {} |",
            html_code(&mapping.mapping_id),
            mapping.count
        )
        .unwrap();
    }
    output.push('\n');
}

fn render_repeated_keys(output: &mut String, summary: &AnalysisSummary) {
    writeln!(output, "## Repeated keys\n").unwrap();
    if summary.repeated_keys.is_empty() {
        writeln!(output, "_No repeated key runs recorded._").unwrap();
        return;
    }
    writeln!(output, "| Key | Runs | Presses |").unwrap();
    writeln!(output, "| --- | ---: | ---: |").unwrap();
    for key in &summary.repeated_keys {
        writeln!(
            output,
            "| {} | {} | {} |",
            html_code(&key.key),
            key.runs,
            key.presses
        )
        .unwrap();
    }
}

fn html_code(value: &str) -> String {
    let mut escaped = String::with_capacity(value.len());
    for character in value.chars() {
        match character {
            '&' => escaped.push_str("&amp;"),
            '<' => escaped.push_str("&lt;"),
            '>' => escaped.push_str("&gt;"),
            '|' => escaped.push_str("&#124;"),
            '\n' => escaped.push_str("\\n"),
            '\r' => escaped.push_str("\\r"),
            character if must_escape_unicode_category(character) => {
                escaped.extend(character.escape_default());
            }
            _ => escaped.push(character),
        }
    }
    format!("<code>{escaped}</code>")
}

fn must_escape_unicode_category(character: char) -> bool {
    matches!(
        get_general_category(character),
        GeneralCategory::Control
            | GeneralCategory::Format
            | GeneralCategory::LineSeparator
            | GeneralCategory::ParagraphSeparator
    )
}

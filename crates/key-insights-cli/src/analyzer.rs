use std::{cmp::Reverse, collections::BTreeMap, fmt::Write, io::BufRead};

use serde::Serialize;
use unicode_general_category::{GeneralCategory, get_general_category};

use crate::{
    ErgonomicSummary, Event, KeymapSnapshot, SequenceMode, SnapshotMapping, SnapshotMode,
    ValidationError, ergonomics, ergonomics::ErgonomicAccumulator, keymap_snapshot::mapping_order,
    validator::JsonlValidator,
};

const SUMMARY_SCHEMA_VERSION: u32 = 3;

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
    SnapshotEventMismatch,
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
            Self::SnapshotEventMismatch => formatter
                .write_str("mapping event mode or typed keys conflict with the keymap snapshot"),
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
    pub ergonomics: ErgonomicSummary,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub mapping_attribution: Option<MappingAttribution>,
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
pub struct MappingAttribution {
    pub snapshot_version: u32,
    pub mappings: Vec<MappingAttributionEntry>,
    pub collisions: Vec<MappingCollision>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct MappingAttributionEntry {
    pub mapping_id: String,
    pub status: MappingAttributionStatus,
    pub count: u64,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub mode: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub scope: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub lhs: Option<Vec<String>>,
    #[serde(skip_serializing_if = "Vec::is_empty")]
    pub collision_mapping_ids: Vec<String>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum MappingAttributionStatus {
    Observed,
    ObservedNotInSnapshot,
    UnobservedInSample,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct MappingCollision {
    pub kind: String,
    pub mode: String,
    pub lhs: Vec<String>,
    pub global_mapping_id: String,
    pub buffer_mapping_id: String,
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
    ergonomics: ErgonomicAccumulator,
}

pub fn analyze_jsonl<R: BufRead>(reader: R) -> Result<AnalysisSummary, AnalysisError> {
    analyze_jsonl_inputs(std::iter::once(reader)).map_err(|error| error.error)
}

pub fn analyze_jsonl_with_snapshot<R: BufRead>(
    reader: R,
    snapshot: &KeymapSnapshot,
) -> Result<AnalysisSummary, AnalysisError> {
    analyze_jsonl_inputs_with_snapshot(std::iter::once(reader), snapshot)
        .map_err(|error| error.error)
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
    analyze_jsonl_inputs_impl(readers, None)
}

pub fn analyze_jsonl_inputs_with_snapshot<I, R>(
    readers: I,
    snapshot: &KeymapSnapshot,
) -> Result<AnalysisSummary, AnalysisInputsError>
where
    I: IntoIterator<Item = R>,
    R: BufRead,
{
    analyze_jsonl_inputs_impl(readers, Some(snapshot))
}

fn analyze_jsonl_inputs_impl<I, R>(
    readers: I,
    snapshot: Option<&KeymapSnapshot>,
) -> Result<AnalysisSummary, AnalysisInputsError>
where
    I: IntoIterator<Item = R>,
    R: BufRead,
{
    let mut accumulator = Accumulator::default();
    let mut analysis_error_input = None;
    let mut validator = JsonlValidator::new();
    for (input_index, reader) in readers.into_iter().enumerate() {
        let source_sessions = validator
            .consume(reader, |event| accumulator.observe(event, snapshot))
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
    Ok(accumulator.finish(validation.sessions, validation.events, snapshot))
}

impl Accumulator {
    fn observe(&mut self, event: &Event, snapshot: Option<&KeymapSnapshot>) {
        match event {
            Event::SessionEnd { elapsed_ms, .. } => {
                self.ergonomics.observe_session_duration(*elapsed_ms);
                match self.total_session_duration_ms.checked_add(*elapsed_ms) {
                    Some(total) => self.total_session_duration_ms = total,
                    None => self.record_error(AnalysisError::SessionDurationOverflow),
                }
            }
            Event::KeySequence {
                mode,
                keys,
                duration_ms,
                ..
            } => {
                self.ergonomics.observe_sequence(keys.len(), *duration_ms);
                self.ergonomics.observe_operations(keys);
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
            Event::ModeTransition { from, to, .. } => {
                self.ergonomics.observe_mode_transition(from, to);
                self.mode_transitions = self.mode_transitions.saturating_add(1);
            }
            Event::MappingUse {
                mode,
                mapping_id,
                typed_keys,
                ..
            } => {
                self.mapping_uses = self.mapping_uses.saturating_add(1);
                if let Some(mapping) = snapshot.and_then(|value| {
                    value
                        .by_id
                        .get(mapping_id)
                        .map(|index| &value.mappings[*index])
                }) && (mapping.mode.as_str() != sequence_mode_name(mode)
                    || mapping.lhs != *typed_keys)
                {
                    self.record_error(AnalysisError::SnapshotEventMismatch);
                }
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

    fn finish(
        self,
        sessions: u64,
        events: u64,
        snapshot: Option<&KeymapSnapshot>,
    ) -> AnalysisSummary {
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
        let mapping_attribution =
            snapshot.map(|value| build_mapping_attribution(value, &self.mappings));
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
            schema_version: SUMMARY_SCHEMA_VERSION,
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
            ergonomics: self.ergonomics.finish(),
            mapping_attribution,
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

fn build_mapping_attribution(
    snapshot: &KeymapSnapshot,
    counts: &BTreeMap<String, u64>,
) -> MappingAttribution {
    let mut collision_ids: BTreeMap<String, Vec<String>> = BTreeMap::new();
    let mut collision_groups: BTreeMap<(SnapshotMode, Vec<String>), Vec<&SnapshotMapping>> =
        BTreeMap::new();
    for mapping in &snapshot.mappings {
        collision_groups
            .entry((mapping.mode, mapping.lhs.clone()))
            .or_default()
            .push(mapping);
    }

    let mut collisions = Vec::new();
    for ((mode, lhs), mappings) in collision_groups {
        let global = mappings
            .iter()
            .find(|mapping| mapping.scope.as_str() == "global");
        let buffer = mappings
            .iter()
            .find(|mapping| mapping.scope.as_str() == "buffer");
        if let (Some(global), Some(buffer)) = (global, buffer) {
            collision_ids
                .entry(global.mapping_id.clone())
                .or_default()
                .push(buffer.mapping_id.clone());
            collision_ids
                .entry(buffer.mapping_id.clone())
                .or_default()
                .push(global.mapping_id.clone());
            collisions.push(MappingCollision {
                kind: "potential_buffer_shadowing".to_owned(),
                mode: mode.as_str().to_owned(),
                lhs,
                global_mapping_id: global.mapping_id.clone(),
                buffer_mapping_id: buffer.mapping_id.clone(),
            });
        }
    }

    let mut mappings = Vec::with_capacity(snapshot.mappings.len() + counts.len());
    for mapping in &snapshot.mappings {
        let count = counts.get(&mapping.mapping_id).copied().unwrap_or(0);
        mappings.push(MappingAttributionEntry {
            mapping_id: mapping.mapping_id.clone(),
            status: if count > 0 {
                MappingAttributionStatus::Observed
            } else {
                MappingAttributionStatus::UnobservedInSample
            },
            count,
            mode: Some(mapping.mode.as_str().to_owned()),
            scope: Some(mapping.scope.as_str().to_owned()),
            lhs: Some(mapping.lhs.clone()),
            collision_mapping_ids: collision_ids
                .remove(&mapping.mapping_id)
                .unwrap_or_default(),
        });
    }
    for (mapping_id, count) in counts {
        if !snapshot.by_id.contains_key(mapping_id) {
            mappings.push(MappingAttributionEntry {
                mapping_id: mapping_id.clone(),
                status: MappingAttributionStatus::ObservedNotInSnapshot,
                count: *count,
                mode: None,
                scope: None,
                lhs: None,
                collision_mapping_ids: Vec::new(),
            });
        }
    }
    mappings.sort_by(|left, right| {
        attribution_status_rank(left.status)
            .cmp(&attribution_status_rank(right.status))
            .then_with(|| match left.status {
                MappingAttributionStatus::Observed
                | MappingAttributionStatus::ObservedNotInSnapshot => right
                    .count
                    .cmp(&left.count)
                    .then_with(|| left.mapping_id.cmp(&right.mapping_id)),
                MappingAttributionStatus::UnobservedInSample => {
                    let left_index = snapshot.by_id[&left.mapping_id];
                    let right_index = snapshot.by_id[&right.mapping_id];
                    mapping_order(
                        &snapshot.mappings[left_index],
                        &snapshot.mappings[right_index],
                    )
                }
            })
    });

    MappingAttribution {
        snapshot_version: snapshot.snapshot_version,
        mappings,
        collisions,
    }
}

fn attribution_status_rank(status: MappingAttributionStatus) -> u8 {
    match status {
        MappingAttributionStatus::Observed => 0,
        MappingAttributionStatus::ObservedNotInSnapshot => 1,
        MappingAttributionStatus::UnobservedInSample => 2,
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
    render_mapping_attribution(&mut output, summary);
    render_repeated_keys(&mut output, summary);
    output.push('\n');
    ergonomics::render_markdown(&mut output, &summary.ergonomics);
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

fn render_mapping_attribution(output: &mut String, summary: &AnalysisSummary) {
    let Some(attribution) = &summary.mapping_attribution else {
        return;
    };
    writeln!(output, "## Snapshot mapping attribution\n").unwrap();
    if attribution.mappings.is_empty() {
        writeln!(output, "_No observed or snapshotted mappings._\n").unwrap();
    } else {
        writeln!(output, "| Status | Mapping ID | Binding | Count |").unwrap();
        writeln!(output, "| --- | --- | --- | ---: |").unwrap();
        for mapping in &attribution.mappings {
            let binding = match (&mapping.mode, &mapping.scope, &mapping.lhs) {
                (Some(mode), Some(scope), Some(lhs)) => {
                    format!("{mode} / {scope} / {}", html_code(&lhs.concat()))
                }
                _ => "_not in snapshot_".to_owned(),
            };
            writeln!(
                output,
                "| {} | {} | {} | {} |",
                attribution_status_name(mapping.status),
                html_code(&mapping.mapping_id),
                binding,
                mapping.count
            )
            .unwrap();
        }
        output.push('\n');
    }

    writeln!(output, "### Potential buffer shadowing\n").unwrap();
    if attribution.collisions.is_empty() {
        writeln!(
            output,
            "_No global/buffer mapping collisions in the snapshot._\n"
        )
        .unwrap();
        return;
    }
    writeln!(output, "| Mode | LHS | Global mapping | Buffer mapping |").unwrap();
    writeln!(output, "| --- | --- | --- | --- |").unwrap();
    for collision in &attribution.collisions {
        writeln!(
            output,
            "| {} | {} | {} | {} |",
            collision.mode,
            html_code(&collision.lhs.concat()),
            html_code(&collision.global_mapping_id),
            html_code(&collision.buffer_mapping_id)
        )
        .unwrap();
    }
    output.push('\n');
}

fn attribution_status_name(status: MappingAttributionStatus) -> &'static str {
    match status {
        MappingAttributionStatus::Observed => "observed",
        MappingAttributionStatus::ObservedNotInSnapshot => "observed_not_in_snapshot",
        MappingAttributionStatus::UnobservedInSample => "unobserved_in_sample",
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

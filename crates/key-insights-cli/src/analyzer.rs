use std::{cmp::Reverse, collections::BTreeMap, fmt::Write, io::BufRead};

use serde::Serialize;

use crate::{
    Event, SCHEMA_VERSION, SequenceMode, ValidationError, validator::for_each_validated_event,
};

pub const MAX_RANKED_ITEMS: usize = 100;

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

pub fn analyze_jsonl<R: BufRead>(reader: R) -> Result<AnalysisSummary, ValidationError> {
    let mut accumulator = Accumulator::default();
    let validation = for_each_validated_event(reader, |event| accumulator.observe(event))?;
    Ok(accumulator.finish(validation.sessions, validation.events))
}

impl Accumulator {
    fn observe(&mut self, event: &Event) {
        match event {
            Event::SessionEnd { elapsed_ms, .. } => {
                self.total_session_duration_ms =
                    self.total_session_duration_ms.saturating_add(*elapsed_ms);
            }
            Event::KeySequence { mode, keys, .. } => {
                self.key_sequences = self.key_sequences.saturating_add(1);
                self.sequence_keys = self.sequence_keys.saturating_add(keys.len() as u64);
                let mode = sequence_mode_name(mode).to_owned();
                let stats = self.modes.entry(mode).or_default();
                stats.sequences = stats.sequences.saturating_add(1);
                stats.keys = stats.keys.saturating_add(keys.len() as u64);
                for key in keys {
                    increment(&mut self.keys, key, 1);
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
                increment(&mut self.mappings, mapping_id, 1);
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
                let repeated = self.repeated_keys.entry(keys[index].clone()).or_default();
                repeated.runs = repeated.runs.saturating_add(1);
                repeated.presses = repeated.presses.saturating_add(presses);
            }
            index = end;
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

fn increment(counts: &mut BTreeMap<String, u64>, key: &str, amount: u64) {
    let count = counts.entry(key.to_owned()).or_default();
    *count = count.saturating_add(amount);
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
            _ => escaped.push(character),
        }
    }
    format!("<code>{escaped}</code>")
}

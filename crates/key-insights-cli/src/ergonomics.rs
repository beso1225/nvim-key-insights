use std::{collections::BTreeMap, fmt::Write};

use serde::{Deserialize, Deserializer, Serialize};

use crate::{KeymapSnapshot, Mode};

pub const ERGONOMICS_CONTRACT_VERSION: u32 = 1;
pub const MAX_ERGONOMIC_CANDIDATES: usize = 100;
pub const MIN_CANDIDATE_SESSIONS: u64 = 3;
pub const MIN_CANDIDATE_SEQUENCE_KEYS: u64 = 100;
pub const MIN_CANDIDATE_OBSERVATIONS: u64 = 3;
pub const HISTOGRAM_VERSION: u32 = 1;
pub const OPERATION_TOKEN_SET_VERSION: u32 = 1;
pub const COUNTABLE_TOKEN_SET_VERSION: u32 = 1;
pub const DIRECTIONAL_MOTION_TOKEN_SET_VERSION: u32 = 1;
pub const CANDIDATE_KIND_VERSION: u32 = 1;

pub(crate) const SESSION_DURATION_BUCKETS: [&str; 5] =
    ["0-1s", "1-10s", "10-60s", "1-5m", "over-5m"];
pub(crate) const SEQUENCE_LENGTH_BUCKETS: [&str; 7] =
    ["1", "2", "3-4", "5-8", "9-16", "17-32", "33-plus"];
pub(crate) const LATENCY_BUCKETS: [&str; 5] =
    ["0-50ms", "50-100ms", "100-250ms", "250-500ms", "over-500ms"];

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ErgonomicSummary {
    pub contract_version: u32,
    pub candidate_limit: usize,
    pub thresholds: ErgonomicThresholds,
    pub distributions: ErgonomicDistributions,
    pub operations: OperationEvidence,
    pub count_prefixes: CountPrefixEvidence,
    pub mode_transitions: Vec<ModeTransitionCount>,
    pub repeated_motions: RepeatedMotionSummary,
    pub mapping_coverage: MappingCoverageEvidence,
    pub candidates: Vec<ErgonomicCandidate>,
}

impl Default for ErgonomicSummary {
    fn default() -> Self {
        Self {
            contract_version: ERGONOMICS_CONTRACT_VERSION,
            candidate_limit: MAX_ERGONOMIC_CANDIDATES,
            thresholds: ErgonomicThresholds::default(),
            distributions: ErgonomicDistributions::default(),
            operations: OperationEvidence::default(),
            count_prefixes: CountPrefixEvidence::default(),
            mode_transitions: Vec::new(),
            repeated_motions: RepeatedMotionSummary::default(),
            mapping_coverage: MappingCoverageEvidence::default(),
            candidates: Vec::new(),
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ErgonomicDistributions {
    pub histogram_version: u32,
    pub session_duration_ms: Vec<HistogramBucket>,
    pub sequence_length_keys: Vec<HistogramBucket>,
    pub average_inter_key_latency_ms: Vec<HistogramBucket>,
}

impl Default for ErgonomicDistributions {
    fn default() -> Self {
        Self::from_counts([0; 5], [0; 7], [0; 5])
    }
}

impl ErgonomicDistributions {
    fn from_counts(
        session_duration: [u64; 5],
        sequence_length: [u64; 7],
        latency: [u64; 5],
    ) -> Self {
        Self {
            histogram_version: HISTOGRAM_VERSION,
            session_duration_ms: buckets(SESSION_DURATION_BUCKETS, session_duration),
            sequence_length_keys: buckets(SEQUENCE_LENGTH_BUCKETS, sequence_length),
            average_inter_key_latency_ms: buckets(LATENCY_BUCKETS, latency),
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct HistogramBucket {
    pub bucket: &'static str,
    pub count: u64,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct OperationEvidence {
    pub token_set_version: u32,
    pub undo: u64,
    pub redo: u64,
    pub repeat: u64,
    pub search_start: u64,
    pub search_navigation: u64,
}

impl Default for OperationEvidence {
    fn default() -> Self {
        Self {
            token_set_version: OPERATION_TOKEN_SET_VERSION,
            undo: 0,
            redo: 0,
            repeat: 0,
            search_start: 0,
            search_navigation: 0,
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct ModeTransitionCount {
    pub from: &'static str,
    pub to: &'static str,
    pub count: u64,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct CountPrefixEvidence {
    pub token_set_version: u32,
    pub occurrences: u64,
    pub digit_presses: u64,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct RepeatedMotionEvidence {
    pub motion: &'static str,
    pub runs: u64,
    pub presses: u64,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct RepeatedMotionSummary {
    pub token_set_version: u32,
    pub items: Vec<RepeatedMotionEvidence>,
}

impl Default for RepeatedMotionSummary {
    fn default() -> Self {
        Self {
            token_set_version: DIRECTIONAL_MOTION_TOKEN_SET_VERSION,
            items: Vec::new(),
        }
    }
}

#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct MappingCoverageEvidence {
    pub snapshot_version: Option<u32>,
    pub total_snapshot_mappings: u64,
    pub observed_mappings: u64,
    pub unobserved_mappings: u64,
}

impl Default for CountPrefixEvidence {
    fn default() -> Self {
        Self {
            token_set_version: COUNTABLE_TOKEN_SET_VERSION,
            occurrences: 0,
            digit_presses: 0,
        }
    }
}

#[derive(Default)]
pub(crate) struct ErgonomicAccumulator {
    session_duration: [u64; 5],
    sequence_length: [u64; 7],
    latency: [u64; 5],
    operations: OperationEvidence,
    count_prefixes: CountPrefixEvidence,
    mode_transitions: BTreeMap<(&'static str, &'static str), u64>,
    repeated_motions: BTreeMap<&'static str, (u64, u64)>,
    overflowed: bool,
}

impl ErgonomicAccumulator {
    pub(crate) fn observe_session_duration(&mut self, elapsed_ms: u64) {
        checked_accumulate(
            &mut self.session_duration[session_duration_bucket(elapsed_ms)],
            1,
            &mut self.overflowed,
        );
    }

    pub(crate) fn observe_sequence(&mut self, key_count: usize, duration_ms: u64) {
        checked_accumulate(
            &mut self.sequence_length[sequence_length_bucket(key_count)],
            1,
            &mut self.overflowed,
        );
        if key_count >= 2 {
            let average_ms = duration_ms / (key_count as u64 - 1);
            checked_accumulate(
                &mut self.latency[latency_bucket(average_ms)],
                1,
                &mut self.overflowed,
            );
        }
    }

    pub(crate) fn observe_operations(&mut self, keys: &[String]) {
        for key in keys {
            let count = match key.as_str() {
                "u" => &mut self.operations.undo,
                "<C-R>" => &mut self.operations.redo,
                "." => &mut self.operations.repeat,
                "/" | "?" => &mut self.operations.search_start,
                "n" | "N" | "*" | "#" => &mut self.operations.search_navigation,
                _ => continue,
            };
            checked_accumulate(count, 1, &mut self.overflowed);
        }
    }

    pub(crate) fn observe_count_prefixes(&mut self, keys: &[String]) {
        let _ = self.observe_count_prefixes_counted(keys);
    }

    fn observe_count_prefixes_counted(&mut self, keys: &[String]) -> usize {
        let mut inspected_tokens = 0;
        let mut index = 0;
        while index < keys.len() {
            inspected_tokens += 1;
            if is_nonzero_digit(&keys[index]) {
                let mut operation = index + 1;
                while operation < keys.len() {
                    inspected_tokens += 1;
                    if !is_digit(&keys[operation]) {
                        break;
                    }
                    operation += 1;
                }
                if operation < keys.len() {
                    inspected_tokens += 1;
                }
                if operation < keys.len() && is_countable_operation(&keys[operation]) {
                    checked_accumulate(
                        &mut self.count_prefixes.occurrences,
                        1,
                        &mut self.overflowed,
                    );
                    checked_accumulate(
                        &mut self.count_prefixes.digit_presses,
                        (operation - index) as u64,
                        &mut self.overflowed,
                    );
                    index = operation + 1;
                } else {
                    index = operation;
                }
                continue;
            }
            index += 1;
        }
        inspected_tokens
    }

    pub(crate) fn observe_repeated_motions(&mut self, keys: &[String]) {
        let mut index = 0;
        while index < keys.len() {
            let mut end = index + 1;
            while end < keys.len() && keys[end] == keys[index] {
                end += 1;
            }
            let presses = (end - index) as u64;
            if presses >= 3
                && let Some(motion) = directional_motion(&keys[index])
            {
                let stats = self.repeated_motions.entry(motion).or_default();
                checked_accumulate(&mut stats.0, 1, &mut self.overflowed);
                checked_accumulate(&mut stats.1, presses, &mut self.overflowed);
            }
            index = end;
        }
    }

    pub(crate) fn observe_mode_transition(&mut self, from: &Mode, to: &Mode) {
        let count = self
            .mode_transitions
            .entry((mode_name(from), mode_name(to)))
            .or_default();
        checked_accumulate(count, 1, &mut self.overflowed);
    }

    pub(crate) fn has_overflowed(&self) -> bool {
        self.overflowed
    }

    pub(crate) fn finish(
        self,
        sessions: u64,
        sequence_keys: u64,
        snapshot: Option<&KeymapSnapshot>,
        mapping_counts: &BTreeMap<String, u64>,
    ) -> ErgonomicSummary {
        let mut repeated_motions: Vec<_> = self
            .repeated_motions
            .into_iter()
            .map(|(motion, (runs, presses))| RepeatedMotionEvidence {
                motion,
                runs,
                presses,
            })
            .collect();
        repeated_motions.sort_by(|left, right| {
            right
                .runs
                .cmp(&left.runs)
                .then_with(|| right.presses.cmp(&left.presses))
                .then_with(|| left.motion.cmp(right.motion))
        });
        repeated_motions.truncate(MAX_ERGONOMIC_CANDIDATES);

        let mut candidates =
            if sessions >= MIN_CANDIDATE_SESSIONS && sequence_keys >= MIN_CANDIDATE_SEQUENCE_KEYS {
                repeated_motions
                    .iter()
                    .filter(|motion| motion.runs >= MIN_CANDIDATE_OBSERVATIONS)
                    .map(|motion| ErgonomicCandidate {
                        candidate_id: format!("repeated-motion-{}", motion.motion),
                        kind: "repeated_motion".to_owned(),
                        kind_version: CANDIDATE_KIND_VERSION,
                        observations: motion.runs,
                        measurements: BTreeMap::from([
                            ("presses".to_owned(), motion.presses),
                            ("runs".to_owned(), motion.runs),
                        ]),
                        guard: CandidateGuard {
                            observed_sessions: sessions,
                            observed_sequence_keys: sequence_keys,
                            required_sessions: MIN_CANDIDATE_SESSIONS,
                            required_sequence_keys: MIN_CANDIDATE_SEQUENCE_KEYS,
                            required_observations: MIN_CANDIDATE_OBSERVATIONS,
                        },
                    })
                    .take(MAX_ERGONOMIC_CANDIDATES)
                    .collect()
            } else {
                Vec::new()
            };

        let mapping_coverage = if let Some(snapshot) = snapshot {
            let observed_mappings = snapshot
                .mappings
                .iter()
                .filter(|mapping| {
                    mapping_counts
                        .get(&mapping.mapping_id)
                        .is_some_and(|count| *count > 0)
                })
                .count() as u64;
            let total_snapshot_mappings = snapshot.mappings.len() as u64;
            let unobserved_mappings = total_snapshot_mappings - observed_mappings;
            if sessions >= MIN_CANDIDATE_SESSIONS
                && sequence_keys >= MIN_CANDIDATE_SEQUENCE_KEYS
                && sessions >= MIN_CANDIDATE_OBSERVATIONS
            {
                candidates.extend(
                    snapshot
                        .mappings
                        .iter()
                        .filter(|mapping| !mapping_counts.contains_key(&mapping.mapping_id))
                        .map(|mapping| ErgonomicCandidate {
                            candidate_id: format!("mapping-unobserved-v1:{}", mapping.mapping_id),
                            kind: "current_mapping_unobserved_in_sample".to_owned(),
                            kind_version: CANDIDATE_KIND_VERSION,
                            observations: sessions,
                            measurements: BTreeMap::from([
                                ("observed_uses".to_owned(), 0),
                                ("sampled_sessions".to_owned(), sessions),
                            ]),
                            guard: CandidateGuard {
                                observed_sessions: sessions,
                                observed_sequence_keys: sequence_keys,
                                required_sessions: MIN_CANDIDATE_SESSIONS,
                                required_sequence_keys: MIN_CANDIDATE_SEQUENCE_KEYS,
                                required_observations: MIN_CANDIDATE_OBSERVATIONS,
                            },
                        }),
                );
            }
            MappingCoverageEvidence {
                snapshot_version: Some(snapshot.snapshot_version),
                total_snapshot_mappings,
                observed_mappings,
                unobserved_mappings,
            }
        } else {
            MappingCoverageEvidence::default()
        };
        candidates.sort_by(|left, right| {
            left.kind
                .cmp(&right.kind)
                .then_with(|| right.observations.cmp(&left.observations))
                .then_with(|| left.candidate_id.cmp(&right.candidate_id))
        });
        candidates.truncate(MAX_ERGONOMIC_CANDIDATES);

        ErgonomicSummary {
            distributions: ErgonomicDistributions::from_counts(
                self.session_duration,
                self.sequence_length,
                self.latency,
            ),
            operations: self.operations,
            count_prefixes: self.count_prefixes,
            mode_transitions: self
                .mode_transitions
                .into_iter()
                .map(|((from, to), count)| ModeTransitionCount { from, to, count })
                .collect(),
            repeated_motions: RepeatedMotionSummary {
                token_set_version: DIRECTIONAL_MOTION_TOKEN_SET_VERSION,
                items: repeated_motions,
            },
            mapping_coverage,
            candidates,
            ..ErgonomicSummary::default()
        }
    }
}

fn checked_accumulate(target: &mut u64, amount: u64, overflowed: &mut bool) {
    match target.checked_add(amount) {
        Some(total) => *target = total,
        None => *overflowed = true,
    }
}

fn directional_motion(token: &str) -> Option<&'static str> {
    match token {
        "h" => Some("h"),
        "j" => Some("j"),
        "k" => Some("k"),
        "l" => Some("l"),
        "w" => Some("w"),
        "b" => Some("b"),
        "e" => Some("e"),
        _ => None,
    }
}

fn is_digit(token: &str) -> bool {
    matches!(
        token,
        "0" | "1" | "2" | "3" | "4" | "5" | "6" | "7" | "8" | "9"
    )
}

fn is_nonzero_digit(token: &str) -> bool {
    matches!(token, "1" | "2" | "3" | "4" | "5" | "6" | "7" | "8" | "9")
}

fn is_countable_operation(token: &str) -> bool {
    matches!(
        token,
        "h" | "j"
            | "k"
            | "l"
            | "w"
            | "W"
            | "b"
            | "B"
            | "e"
            | "E"
            | "0"
            | "$"
            | "^"
            | "_"
            | "+"
            | "-"
            | "G"
            | "|"
            | "%"
            | "{"
            | "}"
            | "("
            | ")"
            | "n"
            | "N"
            | "*"
            | "#"
            | "d"
            | "c"
            | "y"
            | "x"
            | "X"
            | "p"
            | "P"
            | "u"
            | "."
    )
}

fn mode_name(mode: &Mode) -> &'static str {
    match mode {
        Mode::Normal => "normal",
        Mode::Visual => "visual",
        Mode::OperatorPending => "operator_pending",
        Mode::Insert => "insert",
        Mode::Command => "command",
        Mode::Search => "search",
        Mode::Other => "other",
    }
}

fn buckets<const N: usize>(labels: [&'static str; N], counts: [u64; N]) -> Vec<HistogramBucket> {
    labels
        .into_iter()
        .zip(counts)
        .map(|(bucket, count)| HistogramBucket { bucket, count })
        .collect()
}

fn session_duration_bucket(elapsed_ms: u64) -> usize {
    match elapsed_ms {
        0..1_000 => 0,
        1_000..10_000 => 1,
        10_000..60_000 => 2,
        60_000..300_000 => 3,
        _ => 4,
    }
}

fn sequence_length_bucket(key_count: usize) -> usize {
    match key_count {
        0 => unreachable!("validated key sequences are non-empty"),
        1 => 0,
        2 => 1,
        3..=4 => 2,
        5..=8 => 3,
        9..=16 => 4,
        17..=32 => 5,
        33.. => 6,
    }
}

fn latency_bucket(average_ms: u64) -> usize {
    match average_ms {
        0..50 => 0,
        50..100 => 1,
        100..250 => 2,
        250..500 => 3,
        _ => 4,
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ErgonomicThresholds {
    pub minimum_candidate_sessions: u64,
    pub minimum_candidate_sequence_keys: u64,
    pub minimum_candidate_observations: u64,
}

impl Default for ErgonomicThresholds {
    fn default() -> Self {
        Self {
            minimum_candidate_sessions: MIN_CANDIDATE_SESSIONS,
            minimum_candidate_sequence_keys: MIN_CANDIDATE_SEQUENCE_KEYS,
            minimum_candidate_observations: MIN_CANDIDATE_OBSERVATIONS,
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ErgonomicCandidate {
    pub candidate_id: String,
    pub kind: String,
    pub kind_version: u32,
    pub observations: u64,
    pub measurements: BTreeMap<String, u64>,
    pub guard: CandidateGuard,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct CandidateGuard {
    pub observed_sessions: u64,
    pub observed_sequence_keys: u64,
    pub required_sessions: u64,
    pub required_sequence_keys: u64,
    pub required_observations: u64,
}

impl<'de> Deserialize<'de> for HistogramBucket {
    fn deserialize<D>(deserializer: D) -> Result<Self, D::Error>
    where
        D: Deserializer<'de>,
    {
        #[derive(Deserialize)]
        #[serde(deny_unknown_fields)]
        struct Wire {
            bucket: String,
            count: u64,
        }
        let wire = Wire::deserialize(deserializer)?;
        Ok(Self {
            bucket: bucket_from_value(wire.bucket)?,
            count: wire.count,
        })
    }
}

impl<'de> Deserialize<'de> for ModeTransitionCount {
    fn deserialize<D>(deserializer: D) -> Result<Self, D::Error>
    where
        D: Deserializer<'de>,
    {
        #[derive(Deserialize)]
        #[serde(deny_unknown_fields)]
        struct Wire {
            from: String,
            to: String,
            count: u64,
        }
        let wire = Wire::deserialize(deserializer)?;
        Ok(Self {
            from: mode_from_value(wire.from)?,
            to: mode_from_value(wire.to)?,
            count: wire.count,
        })
    }
}

impl<'de> Deserialize<'de> for RepeatedMotionEvidence {
    fn deserialize<D>(deserializer: D) -> Result<Self, D::Error>
    where
        D: Deserializer<'de>,
    {
        #[derive(Deserialize)]
        #[serde(deny_unknown_fields)]
        struct Wire {
            motion: String,
            runs: u64,
            presses: u64,
        }
        let wire = Wire::deserialize(deserializer)?;
        Ok(Self {
            motion: motion_from_value(wire.motion)?,
            runs: wire.runs,
            presses: wire.presses,
        })
    }
}

fn bucket_from_value<E>(value: String) -> Result<&'static str, E>
where
    E: serde::de::Error,
{
    SESSION_DURATION_BUCKETS
        .into_iter()
        .chain(SEQUENCE_LENGTH_BUCKETS)
        .chain(LATENCY_BUCKETS)
        .find(|candidate| *candidate == value)
        .ok_or_else(|| E::custom("unknown histogram bucket"))
}

fn mode_from_value<E>(value: String) -> Result<&'static str, E>
where
    E: serde::de::Error,
{
    match value.as_str() {
        "normal" => Ok("normal"),
        "visual" => Ok("visual"),
        "operator_pending" => Ok("operator_pending"),
        "insert" => Ok("insert"),
        "command" => Ok("command"),
        "search" => Ok("search"),
        "other" => Ok("other"),
        _ => Err(E::custom("unknown mode")),
    }
}

fn motion_from_value<E>(value: String) -> Result<&'static str, E>
where
    E: serde::de::Error,
{
    match value.as_str() {
        "h" => Ok("h"),
        "j" => Ok("j"),
        "k" => Ok("k"),
        "l" => Ok("l"),
        "w" => Ok("w"),
        "b" => Ok("b"),
        "e" => Ok("e"),
        _ => Err(E::custom("unknown repeated motion")),
    }
}

pub(crate) fn render_markdown(output: &mut String, summary: &ErgonomicSummary) {
    writeln!(output, "## Ergonomic evidence\n").unwrap();
    writeln!(output, "- Contract version: {}", summary.contract_version).unwrap();
    writeln!(output, "- Candidate limit: {}", summary.candidate_limit).unwrap();
    writeln!(
        output,
        "- Candidate guard: {} sessions, {} sequence keys, {} observations\n",
        summary.thresholds.minimum_candidate_sessions,
        summary.thresholds.minimum_candidate_sequence_keys,
        summary.thresholds.minimum_candidate_observations
    )
    .unwrap();
    writeln!(output, "### Distributions\n").unwrap();
    writeln!(output, "| Metric | Bucket | Count |").unwrap();
    writeln!(output, "| --- | --- | ---: |").unwrap();
    for (metric, buckets) in [
        (
            "Session duration",
            &summary.distributions.session_duration_ms,
        ),
        (
            "Sequence length",
            &summary.distributions.sequence_length_keys,
        ),
        (
            "Average inter-key latency",
            &summary.distributions.average_inter_key_latency_ms,
        ),
    ] {
        for bucket in buckets {
            writeln!(
                output,
                "| {metric} | {} | {} |",
                bucket.bucket, bucket.count
            )
            .unwrap();
        }
    }
    output.push('\n');
    writeln!(output, "### Operations\n").unwrap();
    writeln!(output, "| Operation | Count |").unwrap();
    writeln!(output, "| --- | ---: |").unwrap();
    for (operation, count) in [
        ("Undo", summary.operations.undo),
        ("Redo", summary.operations.redo),
        ("Repeat", summary.operations.repeat),
        ("Search start", summary.operations.search_start),
        ("Search navigation", summary.operations.search_navigation),
    ] {
        writeln!(output, "| {operation} | {count} |").unwrap();
    }
    writeln!(output, "\n### Count prefixes\n").unwrap();
    writeln!(
        output,
        "- Occurrences: {}",
        summary.count_prefixes.occurrences
    )
    .unwrap();
    writeln!(
        output,
        "- Digit presses: {}",
        summary.count_prefixes.digit_presses
    )
    .unwrap();
    writeln!(output, "\n### Mode transitions\n").unwrap();
    if summary.mode_transitions.is_empty() {
        writeln!(output, "_No mode transitions observed._\n").unwrap();
    } else {
        writeln!(output, "| From | To | Count |").unwrap();
        writeln!(output, "| --- | --- | ---: |").unwrap();
        for transition in &summary.mode_transitions {
            writeln!(
                output,
                "| {} | {} | {} |",
                transition.from, transition.to, transition.count
            )
            .unwrap();
        }
        output.push('\n');
    }
    writeln!(output, "### Repeated motions\n").unwrap();
    if summary.repeated_motions.items.is_empty() {
        writeln!(output, "_No repeated motion runs observed._\n").unwrap();
    } else {
        writeln!(output, "| Motion | Runs | Presses |").unwrap();
        writeln!(output, "| --- | ---: | ---: |").unwrap();
        for motion in &summary.repeated_motions.items {
            writeln!(
                output,
                "| {} | {} | {} |",
                motion.motion, motion.runs, motion.presses
            )
            .unwrap();
        }
        output.push('\n');
    }
    writeln!(output, "### Mapping coverage\n").unwrap();
    if let Some(snapshot_version) = summary.mapping_coverage.snapshot_version {
        writeln!(output, "- Snapshot version: {snapshot_version}").unwrap();
        writeln!(
            output,
            "- Observed mappings: {}/{}",
            summary.mapping_coverage.observed_mappings,
            summary.mapping_coverage.total_snapshot_mappings
        )
        .unwrap();
        writeln!(
            output,
            "- Current mappings unobserved in sample: {}\n",
            summary.mapping_coverage.unobserved_mappings
        )
        .unwrap();
    } else {
        writeln!(output, "_No keymap snapshot was provided._\n").unwrap();
    }
    if summary.candidates.is_empty() {
        writeln!(output, "_No ergonomic candidates met the sample guard._").unwrap();
    } else {
        writeln!(output, "### Candidates\n").unwrap();
        writeln!(
            output,
            "| Candidate | Kind | Observations | Measurements | Guard |"
        )
        .unwrap();
        writeln!(output, "| --- | --- | ---: | --- | --- |").unwrap();
        for candidate in &summary.candidates {
            let measurements = candidate
                .measurements
                .iter()
                .map(|(name, value)| format!("{name}={value}"))
                .collect::<Vec<_>>()
                .join(", ");
            writeln!(
                output,
                "| {} | {} v{} | {} | {} | {}/{} sessions; {}/{} keys; {}/{} observations |",
                candidate.candidate_id,
                candidate.kind,
                candidate.kind_version,
                candidate.observations,
                measurements,
                candidate.guard.observed_sessions,
                candidate.guard.required_sessions,
                candidate.guard.observed_sequence_keys,
                candidate.guard.required_sequence_keys,
                candidate.observations,
                candidate.guard.required_observations
            )
            .unwrap();
        }
    }
}

#[cfg(test)]
mod tests {
    use super::ErgonomicAccumulator;

    #[test]
    fn records_counter_overflow_instead_of_saturating_silently() {
        let mut accumulator = ErgonomicAccumulator::default();
        accumulator.count_prefixes.occurrences = u64::MAX;
        accumulator.observe_count_prefixes(&["2".to_owned(), "j".to_owned()]);

        assert!(accumulator.has_overflowed());
        assert_eq!(accumulator.count_prefixes.occurrences, u64::MAX);
    }

    #[test]
    fn rejects_a_long_non_operation_digit_run_in_one_pass() {
        let mut keys = vec!["1".to_owned(); 16_000];
        keys.push("q".to_owned());
        let mut accumulator = ErgonomicAccumulator::default();

        let inspected_tokens = accumulator.observe_count_prefixes_counted(&keys);

        assert_eq!(accumulator.count_prefixes.occurrences, 0);
        assert_eq!(accumulator.count_prefixes.digit_presses, 0);
        assert!(inspected_tokens <= keys.len() * 3);
    }
}

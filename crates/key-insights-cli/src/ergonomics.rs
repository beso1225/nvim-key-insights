use std::fmt::Write;

use serde::Serialize;

pub const ERGONOMICS_CONTRACT_VERSION: u32 = 1;
pub const MAX_ERGONOMIC_CANDIDATES: usize = 100;
pub const MIN_CANDIDATE_SESSIONS: u64 = 3;
pub const MIN_CANDIDATE_SEQUENCE_KEYS: u64 = 100;
pub const MIN_CANDIDATE_OBSERVATIONS: u64 = 3;
pub const HISTOGRAM_VERSION: u32 = 1;

const SESSION_DURATION_BUCKETS: [&str; 5] = ["0-1s", "1-10s", "10-60s", "1-5m", "over-5m"];
const SEQUENCE_LENGTH_BUCKETS: [&str; 7] = ["1", "2", "3-4", "5-8", "9-16", "17-32", "33-plus"];
const LATENCY_BUCKETS: [&str; 5] = ["0-50ms", "50-100ms", "100-250ms", "250-500ms", "over-500ms"];

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct ErgonomicSummary {
    pub contract_version: u32,
    pub candidate_limit: usize,
    pub thresholds: ErgonomicThresholds,
    pub distributions: ErgonomicDistributions,
    pub candidates: Vec<ErgonomicCandidate>,
}

impl Default for ErgonomicSummary {
    fn default() -> Self {
        Self {
            contract_version: ERGONOMICS_CONTRACT_VERSION,
            candidate_limit: MAX_ERGONOMIC_CANDIDATES,
            thresholds: ErgonomicThresholds::default(),
            distributions: ErgonomicDistributions::default(),
            candidates: Vec::new(),
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
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

#[derive(Default)]
pub(crate) struct ErgonomicAccumulator {
    session_duration: [u64; 5],
    sequence_length: [u64; 7],
    latency: [u64; 5],
}

impl ErgonomicAccumulator {
    pub(crate) fn observe_session_duration(&mut self, elapsed_ms: u64) {
        let count = &mut self.session_duration[session_duration_bucket(elapsed_ms)];
        *count = count.saturating_add(1);
    }

    pub(crate) fn observe_sequence(&mut self, key_count: usize, duration_ms: u64) {
        let count = &mut self.sequence_length[sequence_length_bucket(key_count)];
        *count = count.saturating_add(1);
        if key_count >= 2 {
            let average_ms = duration_ms / (key_count as u64 - 1);
            let count = &mut self.latency[latency_bucket(average_ms)];
            *count = count.saturating_add(1);
        }
    }

    pub(crate) fn finish(self) -> ErgonomicSummary {
        ErgonomicSummary {
            distributions: ErgonomicDistributions::from_counts(
                self.session_duration,
                self.sequence_length,
                self.latency,
            ),
            ..ErgonomicSummary::default()
        }
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

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
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

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct ErgonomicCandidate {
    pub candidate_id: String,
    pub kind: String,
    pub observations: u64,
    pub guard: CandidateGuard,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct CandidateGuard {
    pub observed_sessions: u64,
    pub observed_sequence_keys: u64,
    pub required_sessions: u64,
    pub required_sequence_keys: u64,
    pub required_observations: u64,
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
    if summary.candidates.is_empty() {
        writeln!(output, "_No ergonomic candidates met the sample guard._").unwrap();
    }
}

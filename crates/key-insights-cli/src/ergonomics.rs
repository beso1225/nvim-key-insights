use std::fmt::Write;

use serde::Serialize;

pub const ERGONOMICS_CONTRACT_VERSION: u32 = 1;
pub const MAX_ERGONOMIC_CANDIDATES: usize = 100;
pub const MIN_CANDIDATE_SESSIONS: u64 = 3;
pub const MIN_CANDIDATE_SEQUENCE_KEYS: u64 = 100;
pub const MIN_CANDIDATE_OBSERVATIONS: u64 = 3;

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct ErgonomicSummary {
    pub contract_version: u32,
    pub candidate_limit: usize,
    pub thresholds: ErgonomicThresholds,
    pub candidates: Vec<ErgonomicCandidate>,
}

impl Default for ErgonomicSummary {
    fn default() -> Self {
        Self {
            contract_version: ERGONOMICS_CONTRACT_VERSION,
            candidate_limit: MAX_ERGONOMIC_CANDIDATES,
            thresholds: ErgonomicThresholds::default(),
            candidates: Vec::new(),
        }
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
    if summary.candidates.is_empty() {
        writeln!(output, "_No ergonomic candidates met the sample guard._").unwrap();
    }
}

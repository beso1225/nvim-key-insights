use std::{
    collections::{BTreeMap, BTreeSet},
    fmt::Write as _,
    io::Read,
};

use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};

pub const SNAPSHOT_VERSION: u32 = 1;
pub const MAX_SNAPSHOT_BYTES: usize = 1024 * 1024;
pub const MAX_SNAPSHOT_MAPPINGS: usize = 4096;
const MAX_LHS_BYTES: usize = 4096;
const MAX_LHS_TOKENS: usize = 64;
const MAX_TOKEN_BYTES: usize = 256;

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SnapshotError(&'static str);

impl std::fmt::Display for SnapshotError {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        formatter.write_str(self.0)
    }
}

impl std::error::Error for SnapshotError {}

#[derive(Debug, Clone, Deserialize)]
#[serde(deny_unknown_fields)]
struct SnapshotDocument {
    snapshot_version: u32,
    mappings: Vec<SnapshotMapping>,
}

#[derive(Debug, Clone, PartialEq, Eq, Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
pub struct SnapshotMapping {
    pub mapping_id: String,
    pub mode: SnapshotMode,
    pub scope: SnapshotScope,
    pub lhs: Vec<String>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Deserialize, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum SnapshotMode {
    Normal,
    OperatorPending,
    Visual,
}

impl SnapshotMode {
    pub(crate) fn as_str(self) -> &'static str {
        match self {
            Self::Normal => "normal",
            Self::OperatorPending => "operator_pending",
            Self::Visual => "visual",
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Deserialize, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum SnapshotScope {
    Buffer,
    Global,
}

impl SnapshotScope {
    pub(crate) fn as_str(self) -> &'static str {
        match self {
            Self::Buffer => "buffer",
            Self::Global => "global",
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct KeymapSnapshot {
    pub snapshot_version: u32,
    pub mappings: Vec<SnapshotMapping>,
    pub(crate) by_id: BTreeMap<String, usize>,
}

pub fn parse_keymap_snapshot<R: Read>(reader: R) -> Result<KeymapSnapshot, SnapshotError> {
    let mut bytes = Vec::new();
    reader
        .take((MAX_SNAPSHOT_BYTES + 1) as u64)
        .read_to_end(&mut bytes)
        .map_err(|_| SnapshotError("failed to read keymap snapshot"))?;
    if bytes.len() > MAX_SNAPSHOT_BYTES {
        return Err(SnapshotError("keymap snapshot exceeds the byte limit"));
    }
    let document: SnapshotDocument = serde_json::from_slice(&bytes)
        .map_err(|_| SnapshotError("keymap snapshot is not strict JSON"))?;
    if document.snapshot_version != SNAPSHOT_VERSION {
        return Err(SnapshotError("unsupported keymap snapshot version"));
    }
    if document.mappings.len() > MAX_SNAPSHOT_MAPPINGS {
        return Err(SnapshotError("keymap snapshot exceeds the mapping limit"));
    }

    let mut by_id = BTreeMap::new();
    let mut tuples = BTreeSet::new();
    let mut previous: Option<&SnapshotMapping> = None;
    for (index, mapping) in document.mappings.iter().enumerate() {
        validate_mapping(mapping)?;
        if previous.is_some_and(|prior| mapping_order(prior, mapping).is_ge()) {
            return Err(SnapshotError("keymap snapshot mappings are not canonical"));
        }
        previous = Some(mapping);
        if by_id.insert(mapping.mapping_id.clone(), index).is_some() {
            return Err(SnapshotError(
                "keymap snapshot contains a duplicate mapping ID",
            ));
        }
        if !tuples.insert((mapping.mode, mapping.scope, mapping.lhs.clone())) {
            return Err(SnapshotError(
                "keymap snapshot contains a duplicate mapping tuple",
            ));
        }
    }

    Ok(KeymapSnapshot {
        snapshot_version: document.snapshot_version,
        mappings: document.mappings,
        by_id,
    })
}

fn validate_mapping(mapping: &SnapshotMapping) -> Result<(), SnapshotError> {
    if mapping.lhs.is_empty() || mapping.lhs.len() > MAX_LHS_TOKENS {
        return Err(SnapshotError(
            "keymap snapshot has an invalid LHS token count",
        ));
    }
    let mut canonical = String::new();
    for token in &mapping.lhs {
        if token.is_empty()
            || token.len() > MAX_TOKEN_BYTES
            || token
                .chars()
                .any(|character| character <= '\u{1f}' || character == '\u{7f}')
        {
            return Err(SnapshotError("keymap snapshot has an invalid LHS token"));
        }
        canonical.push_str(token);
        if canonical.len() > MAX_LHS_BYTES {
            return Err(SnapshotError("keymap snapshot LHS exceeds the byte limit"));
        }
    }
    if tokenize_canonical(&canonical) != mapping.lhs {
        return Err(SnapshotError(
            "keymap snapshot LHS is not canonically tokenized",
        ));
    }
    let expected = mapping_id(mapping.mode, mapping.scope, &mapping.lhs);
    if mapping.mapping_id != expected {
        return Err(SnapshotError(
            "keymap snapshot mapping ID does not match its tuple",
        ));
    }
    Ok(())
}

fn tokenize_canonical(canonical: &str) -> Vec<String> {
    let mut tokens = Vec::new();
    let mut index = 0;
    while index < canonical.len() {
        let remainder = &canonical[index..];
        let mut end = index
            + remainder
                .chars()
                .next()
                .expect("non-empty remainder")
                .len_utf8();
        if remainder.starts_with('<')
            && let Some(relative_closing) = remainder.find('>')
        {
            let candidate_end = index + relative_closing + 1;
            if candidate_end - index <= MAX_TOKEN_BYTES {
                end = candidate_end;
            }
        }
        tokens.push(canonical[index..end].to_owned());
        index = end;
    }
    tokens
}

fn mapping_id(mode: SnapshotMode, scope: SnapshotScope, lhs: &[String]) -> String {
    let mut preimage = String::new();
    append_length_prefixed(&mut preimage, "mapping-v1");
    append_length_prefixed(&mut preimage, mode.as_str());
    append_length_prefixed(&mut preimage, scope.as_str());
    append_length_prefixed(&mut preimage, &lhs.len().to_string());
    for token in lhs {
        append_length_prefixed(&mut preimage, token);
    }
    format!("mapping-v1:{:x}", Sha256::digest(preimage.as_bytes()))
}

fn append_length_prefixed(output: &mut String, value: &str) {
    write!(output, "{}:{value}", value.len()).expect("writing to a string cannot fail");
}

pub(crate) fn mapping_order(left: &SnapshotMapping, right: &SnapshotMapping) -> std::cmp::Ordering {
    left.mode
        .cmp(&right.mode)
        .then_with(|| left.lhs.cmp(&right.lhs))
        .then_with(|| left.scope.cmp(&right.scope))
        .then_with(|| left.mapping_id.cmp(&right.mapping_id))
}

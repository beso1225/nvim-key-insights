use std::{
    collections::HashSet,
    fmt,
    io::{self, BufRead, Read},
};

use crate::{Event, SCHEMA_VERSION};

pub const MAX_EVENT_LINE_BYTES: usize = 64 * 1024;
pub const MAX_SESSION_ID_BYTES: usize = 128;
pub const MAX_SESSIONS_PER_LOG: usize = 4096;
pub const MAX_KEY_TOKEN_BYTES: usize = 256;
pub const MAX_MAPPING_ID_BYTES: usize = 256;

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ValidationSummary {
    pub sessions: u64,
    pub events: u64,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ValidationError {
    pub line: usize,
    pub kind: ValidationErrorKind,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ValidationErrorKind {
    Io,
    LineTooLong,
    MalformedEvent,
    UnsupportedSchema { found: u32 },
    EmptySessionId,
    SessionIdTooLong,
    EmptyKeySequence,
    KeyTokenTooLong,
    MappingIdTooLong,
    InvalidSessionStartElapsed,
    ExpectedSessionStart,
    SessionAlreadyActive,
    SessionMismatch,
    ElapsedTimeWentBackward,
    ReusedSessionId,
    TooManySessions,
    UnclosedSession,
}

impl fmt::Display for ValidationError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(
            formatter,
            "JSONL validation failed at line {}: {}",
            self.line, self.kind
        )
    }
}

impl std::error::Error for ValidationError {}

impl fmt::Display for ValidationErrorKind {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Io => formatter.write_str("I/O error"),
            Self::LineTooLong => formatter.write_str("event line exceeds the size limit"),
            Self::MalformedEvent => formatter.write_str("malformed event"),
            Self::UnsupportedSchema { found } => {
                write!(formatter, "unsupported schema version {found}")
            }
            Self::EmptySessionId => formatter.write_str("session ID is empty"),
            Self::SessionIdTooLong => formatter.write_str("session ID exceeds the size limit"),
            Self::EmptyKeySequence => formatter.write_str("key sequence is empty"),
            Self::KeyTokenTooLong => formatter.write_str("key token exceeds the size limit"),
            Self::MappingIdTooLong => formatter.write_str("mapping ID exceeds the size limit"),
            Self::InvalidSessionStartElapsed => {
                formatter.write_str("session_start elapsed_ms must be zero")
            }
            Self::ExpectedSessionStart => formatter.write_str("expected session_start"),
            Self::SessionAlreadyActive => formatter.write_str("a session is already active"),
            Self::SessionMismatch => formatter.write_str("session ID does not match"),
            Self::ElapsedTimeWentBackward => formatter.write_str("elapsed time moved backwards"),
            Self::ReusedSessionId => formatter.write_str("session ID was reused"),
            Self::TooManySessions => formatter.write_str("session count exceeds the limit"),
            Self::UnclosedSession => formatter.write_str("session was not closed"),
        }
    }
}

struct ActiveSession {
    id: String,
    last_elapsed_ms: u64,
    start_line: usize,
}

pub fn validate_jsonl<R: BufRead>(mut reader: R) -> Result<ValidationSummary, ValidationError> {
    for_each_validated_event(&mut reader, |_| {})
}

pub(crate) fn for_each_validated_event<R, F>(
    mut reader: R,
    mut on_event: F,
) -> Result<ValidationSummary, ValidationError>
where
    R: BufRead,
    F: FnMut(&Event),
{
    let mut summary = ValidationSummary {
        sessions: 0,
        events: 0,
    };
    let mut active: Option<ActiveSession> = None;
    let mut seen_session_ids = HashSet::new();
    let mut buffer = Vec::new();
    let mut line_number = 0;

    loop {
        buffer.clear();
        let mut limited_reader = (&mut reader).take((MAX_EVENT_LINE_BYTES + 1) as u64);
        let bytes_read =
            limited_reader
                .read_until(b'\n', &mut buffer)
                .map_err(|_error: io::Error| ValidationError {
                    line: line_number + 1,
                    kind: ValidationErrorKind::Io,
                })?;
        if bytes_read == 0 {
            break;
        }

        line_number += 1;
        if buffer.len() > MAX_EVENT_LINE_BYTES {
            return Err(error(line_number, ValidationErrorKind::LineTooLong));
        }

        trim_line_ending(&mut buffer);
        if buffer.is_empty() {
            return Err(error(line_number, ValidationErrorKind::MalformedEvent));
        }

        let event: Event = serde_json::from_slice(&buffer)
            .map_err(|_| error(line_number, ValidationErrorKind::MalformedEvent))?;
        validate_event(
            &event,
            line_number,
            &mut active,
            &mut seen_session_ids,
            &mut summary,
        )?;
        on_event(&event);
    }

    if let Some(session) = active {
        return Err(error(
            session.start_line,
            ValidationErrorKind::UnclosedSession,
        ));
    }

    Ok(summary)
}

fn validate_event(
    event: &Event,
    line: usize,
    active: &mut Option<ActiveSession>,
    seen_session_ids: &mut HashSet<String>,
    summary: &mut ValidationSummary,
) -> Result<(), ValidationError> {
    if event.schema_version() != SCHEMA_VERSION {
        return Err(error(
            line,
            ValidationErrorKind::UnsupportedSchema {
                found: event.schema_version(),
            },
        ));
    }
    if event.session_id().is_empty() {
        return Err(error(line, ValidationErrorKind::EmptySessionId));
    }
    if event.session_id().len() > MAX_SESSION_ID_BYTES {
        return Err(error(line, ValidationErrorKind::SessionIdTooLong));
    }
    validate_payload(event, line)?;

    match event {
        Event::SessionStart {
            session_id,
            elapsed_ms,
            ..
        } => {
            if active.is_some() {
                return Err(error(line, ValidationErrorKind::SessionAlreadyActive));
            }
            if *elapsed_ms != 0 {
                return Err(error(line, ValidationErrorKind::InvalidSessionStartElapsed));
            }
            if seen_session_ids.contains(session_id) {
                return Err(error(line, ValidationErrorKind::ReusedSessionId));
            }
            if seen_session_ids.len() >= MAX_SESSIONS_PER_LOG {
                return Err(error(line, ValidationErrorKind::TooManySessions));
            }
            seen_session_ids.insert(session_id.clone());
            *active = Some(ActiveSession {
                id: session_id.clone(),
                last_elapsed_ms: *elapsed_ms,
                start_line: line,
            });
            summary.sessions += 1;
        }
        other => {
            let session = active
                .as_mut()
                .ok_or_else(|| error(line, ValidationErrorKind::ExpectedSessionStart))?;
            if other.session_id() != session.id {
                return Err(error(line, ValidationErrorKind::SessionMismatch));
            }
            if other.elapsed_ms() < session.last_elapsed_ms {
                return Err(error(line, ValidationErrorKind::ElapsedTimeWentBackward));
            }
            session.last_elapsed_ms = other.elapsed_ms();
            if matches!(other, Event::SessionEnd { .. }) {
                *active = None;
            }
        }
    }

    summary.events += 1;
    Ok(())
}

fn validate_payload(event: &Event, line: usize) -> Result<(), ValidationError> {
    let keys = match event {
        Event::KeySequence { keys, .. } => Some(keys),
        Event::MappingUse { typed_keys, .. } => Some(typed_keys),
        _ => None,
    };

    if keys.is_some_and(|values| values.is_empty() || values.iter().any(String::is_empty)) {
        return Err(error(line, ValidationErrorKind::EmptyKeySequence));
    }
    if keys.is_some_and(|values| values.iter().any(|value| value.len() > MAX_KEY_TOKEN_BYTES)) {
        return Err(error(line, ValidationErrorKind::KeyTokenTooLong));
    }
    if matches!(
        event,
        Event::MappingUse { mapping_id, .. } if mapping_id.len() > MAX_MAPPING_ID_BYTES
    ) {
        return Err(error(line, ValidationErrorKind::MappingIdTooLong));
    }

    Ok(())
}

fn trim_line_ending(buffer: &mut Vec<u8>) {
    if buffer.last() == Some(&b'\n') {
        buffer.pop();
    }
    if buffer.last() == Some(&b'\r') {
        buffer.pop();
    }
}

fn error(line: usize, kind: ValidationErrorKind) -> ValidationError {
    ValidationError { line, kind }
}

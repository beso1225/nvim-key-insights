use std::{
    ffi::{OsStr, OsString},
    fs,
    path::Path,
};

use key_insights::{MAX_SESSION_ID_BYTES, MAX_SESSIONS_PER_LOG};

use super::{ResolvedDirectory, ResolvedInputPath, input_identity, owned_input_identity};

const SESSION_FILE_PREFIX: &str = "nvim-key-insights-";
const SESSION_FILE_SUFFIX: &str = ".jsonl";
const MAX_SESSION_DIRECTORY_ENTRIES: usize = 8192;

pub(super) fn discover_session_inputs(path: &Path) -> Result<Vec<ResolvedInputPath>, String> {
    discover_session_inputs_with_limits_and_hooks(
        path,
        MAX_SESSION_DIRECTORY_ENTRIES,
        MAX_SESSIONS_PER_LOG,
        || {},
        |_| {},
    )
}

fn discover_session_inputs_with_limits_and_hooks<AfterScan, BeforeOpen>(
    path: &Path,
    maximum_entries: usize,
    maximum_inputs: usize,
    after_scan: AfterScan,
    mut before_open: BeforeOpen,
) -> Result<Vec<ResolvedInputPath>, String>
where
    AfterScan: FnOnce(),
    BeforeOpen: FnMut(&Path),
{
    let directory = ResolvedDirectory::open(path).map_err(|error| {
        format!(
            "failed to open session directory {}: {error}",
            path.display()
        )
    })?;
    let resolved = fs::canonicalize(path).map_err(|error| {
        format!(
            "failed to resolve session directory {}: {error}",
            path.display()
        )
    })?;
    directory.verify_current().map_err(|error| {
        format!(
            "session directory changed during resolution {}: {error}",
            path.display()
        )
    })?;
    let mut names = directory.child_names(maximum_entries).map_err(|error| {
        format!(
            "failed to scan session directory {}: {error}",
            path.display()
        )
    })?;
    names.retain(|name| is_finalized_session_name(name));
    names.sort_by(|left, right| ascii_name(left).cmp(ascii_name(right)));
    after_scan();
    directory.verify_current().map_err(|error| {
        format!(
            "session directory changed during discovery {}: {error}",
            path.display()
        )
    })?;

    let mut identities = std::collections::HashSet::new();
    let mut inputs = Vec::new();
    for name in names {
        let input_path = resolved.join(&name);
        let Some(before) = directory.child_metadata(&name).map_err(|error| {
            format!(
                "failed to inspect session {}: {error}",
                input_path.display()
            )
        })?
        else {
            return Err(format!(
                "discovered session changed before it could be opened: {}",
                input_path.display()
            ));
        };
        if !before.is_private_file_owned_by_current_user() {
            continue;
        }
        before_open(&input_path);
        let Some(file) = directory
            .open_read_file(&name)
            .map_err(|error| format!("failed to open session {}: {error}", input_path.display()))?
        else {
            return Err(format!(
                "discovered session changed before it could be opened: {}",
                input_path.display()
            ));
        };
        let after = directory.child_metadata(&name).map_err(|error| {
            format!("failed to verify session {}: {error}", input_path.display())
        })?;
        let unchanged = after.is_some_and(|after| before.same_regular_file(after))
            && directory
                .open_file_matches_child(&file, &name)
                .map_err(|error| {
                    format!("failed to verify session {}: {error}", input_path.display())
                })?;
        if !unchanged {
            return Err(format!(
                "discovered session changed while it was opened: {}",
                input_path.display()
            ));
        }
        if inputs.len() >= maximum_inputs {
            return Err(format!(
                "finalized session count exceeds the limit of {maximum_inputs}"
            ));
        }
        let metadata = file.metadata().map_err(|error| {
            format!(
                "failed to inspect session {}: {error}",
                input_path.display()
            )
        })?;
        let identity = input_identity(&input_path, &metadata);
        if !identities.insert(owned_input_identity(&identity)) {
            return Err(format!("duplicate input: {}", input_path.display()));
        }
        inputs.push(ResolvedInputPath {
            path: input_path,
            file,
            identity,
        });
    }
    directory.verify_current().map_err(|error| {
        format!(
            "session directory changed during discovery {}: {error}",
            path.display()
        )
    })?;
    if inputs.is_empty() {
        return Err(format!("no finalized sessions found in {}", path.display()));
    }
    Ok(inputs)
}

fn is_finalized_session_name(name: &OsStr) -> bool {
    let Some(name) = name.to_str() else {
        return false;
    };
    let Some(session_id) = name
        .strip_prefix(SESSION_FILE_PREFIX)
        .and_then(|name| name.strip_suffix(SESSION_FILE_SUFFIX))
    else {
        return false;
    };
    !session_id.is_empty()
        && session_id.len() <= MAX_SESSION_ID_BYTES
        && session_id
            .bytes()
            .all(|byte| byte.is_ascii_alphanumeric() || byte == b'_' || byte == b'-')
}

fn ascii_name(name: &OsString) -> &str {
    name.to_str()
        .expect("collector namespace names are validated ASCII")
}

#[cfg(all(test, unix))]
mod tests {
    use std::{
        fs,
        os::unix::fs::{MetadataExt, PermissionsExt, symlink},
        path::PathBuf,
        time::{SystemTime, UNIX_EPOCH},
    };

    use super::*;

    #[test]
    fn bounds_directory_scan_before_filtering_names() {
        let directory = temporary_directory("scan-bound");
        fs::create_dir(&directory).expect("create directory");
        fs::write(directory.join("unrelated-a"), "a").expect("write first unrelated entry");
        fs::write(directory.join("unrelated-b"), "b").expect("write second unrelated entry");

        let error = discover_session_inputs_with_limits_and_hooks(
            &directory,
            1,
            MAX_SESSIONS_PER_LOG,
            || {},
            |_| {},
        )
        .expect_err("scan must be bounded");

        assert!(
            error.contains("directory entry count exceeds the limit of 1"),
            "{error}"
        );
        fs::remove_dir_all(directory).expect("remove directory");
    }

    #[test]
    fn bounds_accepted_finalized_session_count() {
        let directory = temporary_directory("accepted-bound");
        fs::create_dir(&directory).expect("create directory");
        write_private(directory.join("nvim-key-insights-a.jsonl"), "a");
        write_private(directory.join("nvim-key-insights-b.jsonl"), "b");

        let error = discover_session_inputs_with_limits_and_hooks(&directory, 10, 1, || {}, |_| {})
            .expect_err("accepted inputs must be bounded");

        assert!(
            error.contains("finalized session count exceeds the limit of 1"),
            "{error}"
        );
        fs::remove_dir_all(directory).expect("remove directory");
    }

    #[test]
    fn rejects_a_replaced_directory_after_scanning_its_handle() {
        let root = temporary_directory("directory-replacement");
        let directory = root.join("sessions");
        let moved = root.join("moved");
        let replacement = root.join("replacement");
        fs::create_dir_all(&directory).expect("create session directory");
        fs::create_dir(&replacement).expect("create replacement directory");
        write_private(directory.join("nvim-key-insights-a.jsonl"), "not JSON\n");

        let error = discover_session_inputs_with_limits_and_hooks(
            &directory,
            10,
            10,
            || {
                fs::rename(&directory, &moved).expect("move session directory");
                symlink(&replacement, &directory).expect("replace session directory");
            },
            |_| {},
        )
        .expect_err("directory replacement must fail closed");

        assert!(
            error.contains("session directory changed during discovery"),
            "{error}"
        );
        fs::remove_dir_all(root).expect("remove root");
    }

    #[test]
    fn rejects_a_discovered_file_replaced_before_open() {
        let directory = temporary_directory("file-replacement");
        fs::create_dir(&directory).expect("create directory");
        let session = directory.join("nvim-key-insights-a.jsonl");
        let replacement = directory.join("replacement");
        write_private(&session, "first\n");
        write_private(&replacement, "replacement\n");
        let original_metadata = fs::metadata(&session).expect("inspect original session");
        let replacement_metadata = fs::metadata(&replacement).expect("inspect replacement");
        assert_ne!(
            (original_metadata.dev(), original_metadata.ino()),
            (replacement_metadata.dev(), replacement_metadata.ino()),
            "the fixture must use distinct filesystem identities"
        );

        let error = discover_session_inputs_with_limits_and_hooks(
            &directory,
            10,
            10,
            || {},
            |path| {
                fs::rename(&replacement, path).expect("replace discovered session");
            },
        )
        .expect_err("file replacement must fail closed");

        assert!(
            error.contains("discovered session changed while it was opened"),
            "{error}"
        );
        fs::remove_dir_all(directory).expect("remove directory");
    }

    fn write_private(path: impl AsRef<Path>, contents: &str) {
        fs::write(path.as_ref(), contents).expect("write private file");
        fs::set_permissions(path, fs::Permissions::from_mode(0o600))
            .expect("set private permissions");
    }

    fn temporary_directory(label: &str) -> PathBuf {
        let unique = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .expect("clock after epoch")
            .as_nanos();
        std::env::temp_dir().join(format!(
            "key-insights-discovery-{label}-{}-{unique}",
            std::process::id()
        ))
    }
}

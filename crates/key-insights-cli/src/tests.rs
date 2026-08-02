use std::{
    fs,
    os::unix::fs::{PermissionsExt, symlink},
    path::PathBuf,
    time::{SystemTime, UNIX_EPOCH},
};

use super::{
    OutputBackup, OutputLocks, PairPublication, StagedOutput, link_without_replacement,
    open_private_lock_file, output_lock_path, publish_pair, publish_pair_with_hook,
    publish_pair_with_hooks, resolve_paths,
};

#[test]
fn output_paths_cannot_alias_recovery_sidecars() {
    let unique = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .expect("clock after epoch")
        .as_nanos();
    let directory = std::env::temp_dir().join(format!(
        "key-insights-recovery-path-collision-{}-{unique}",
        std::process::id()
    ));
    fs::create_dir(&directory).expect("create test directory");
    let input = directory.join("input.jsonl");
    let summary = fs::canonicalize(&directory)
        .expect("canonical test directory")
        .join("summary.json");
    let report = super::output_recovery_index_path(&summary).expect("summary recovery index");
    fs::write(&input, "raw input\n").expect("write input");

    let error = match resolve_paths(&input, &summary, &report) {
        Ok(_) => panic!("outputs must not alias recovery artifacts"),
        Err(error) => error,
    };

    assert!(error.contains("recovery artifact"), "{error}");
    fs::remove_dir_all(directory).expect("remove test directory");
}

#[test]
fn captured_backup_detects_destination_replacement() {
    let unique = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .expect("clock after epoch")
        .as_nanos();
    let directory = std::env::temp_dir().join(format!(
        "key-insights-destination-replacement-{}-{unique}",
        std::process::id()
    ));
    fs::create_dir(&directory).expect("create test directory");
    let summary = directory.join("summary.json");
    let report = directory.join("report.md");
    fs::write(&summary, "old summary\n").expect("write old summary");
    fs::write(&report, "old report\n").expect("write old report");
    let summary_output = StagedOutput::create(&summary, b"new summary\n").expect("stage summary");
    let report_output = StagedOutput::create(&report, b"new report\n").expect("stage report");
    let publication = PairPublication::begin_anchored(&summary_output, &report_output)
        .expect("begin publication");

    fs::remove_file(&summary).expect("remove captured summary");
    fs::write(&summary, "intervening summary\n").expect("replace captured summary");

    let error = publication
        .summary_backup
        .verify_destination_unchanged()
        .expect_err("replacement must be detected");

    assert!(error.contains("changed after backup capture"), "{error}");
    publication.commit().expect("clean aborted publication");
    assert_eq!(
        fs::read_to_string(&summary).expect("read intervening summary"),
        "intervening summary\n"
    );
    fs::remove_dir_all(directory).expect("remove test directory");
}

#[test]
fn publication_preserves_a_destination_replaced_after_backup_capture() {
    let unique = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .expect("clock after epoch")
        .as_nanos();
    let directory = std::env::temp_dir().join(format!(
        "key-insights-pre-publication-replacement-{}-{unique}",
        std::process::id()
    ));
    fs::create_dir(&directory).expect("create test directory");
    let summary = directory.join("summary.json");
    let report = directory.join("report.md");
    fs::write(&summary, "old summary\n").expect("write old summary");
    fs::write(&report, "old report\n").expect("write old report");
    let summary_output = StagedOutput::create(&summary, b"new summary\n").expect("stage summary");
    let report_output = StagedOutput::create(&report, b"new report\n").expect("stage report");

    let error = publish_pair_with_hooks(
        summary_output,
        report_output,
        || {
            fs::remove_file(&summary).expect("remove captured summary");
            fs::write(&summary, "intervening summary\n").expect("replace captured summary");
        },
        || {},
    )
    .expect_err("intervening destination must abort publication");

    assert!(error.contains("changed after backup capture"), "{error}");
    assert_eq!(
        fs::read_to_string(&summary).expect("read intervening summary"),
        "intervening summary\n"
    );
    assert_eq!(
        fs::read_to_string(&report).expect("read original report"),
        "old report\n"
    );
    fs::remove_dir_all(directory).expect("remove test directory");
}

#[test]
fn publication_preserves_a_destination_created_after_absence_was_captured() {
    let unique = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .expect("clock after epoch")
        .as_nanos();
    let directory = std::env::temp_dir().join(format!(
        "key-insights-pre-publication-creation-{}-{unique}",
        std::process::id()
    ));
    fs::create_dir(&directory).expect("create test directory");
    let summary = directory.join("summary.json");
    let report = directory.join("report.md");
    fs::write(&report, "old report\n").expect("write old report");
    let summary_output = StagedOutput::create(&summary, b"new summary\n").expect("stage summary");
    let report_output = StagedOutput::create(&report, b"new report\n").expect("stage report");

    let error = publish_pair_with_hooks(
        summary_output,
        report_output,
        || fs::write(&summary, "intervening summary\n").expect("create intervening summary"),
        || {},
    )
    .expect_err("new destination must abort publication");

    assert!(error.contains("changed after backup capture"), "{error}");
    assert_eq!(
        fs::read_to_string(&summary).expect("read intervening summary"),
        "intervening summary\n"
    );
    assert_eq!(
        fs::read_to_string(&report).expect("read original report"),
        "old report\n"
    );
    fs::remove_dir_all(directory).expect("remove test directory");
}

#[test]
fn publication_preserves_report_replaced_after_summary_publication() {
    let unique = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .expect("clock after epoch")
        .as_nanos();
    let directory = std::env::temp_dir().join(format!(
        "key-insights-report-replacement-{}-{unique}",
        std::process::id()
    ));
    fs::create_dir(&directory).expect("create test directory");
    let summary = directory.join("summary.json");
    let report = directory.join("report.md");
    fs::write(&summary, "old summary\n").expect("write old summary");
    fs::write(&report, "old report\n").expect("write old report");
    let summary_output = StagedOutput::create(&summary, b"new summary\n").expect("stage summary");
    let report_output = StagedOutput::create(&report, b"new report\n").expect("stage report");

    let error = publish_pair_with_hooks(
        summary_output,
        report_output,
        || {},
        || {
            fs::remove_file(&report).expect("remove captured report");
            fs::write(&report, "intervening report\n").expect("replace captured report");
        },
    )
    .expect_err("intervening report must abort publication");

    assert!(error.contains("changed after backup capture"), "{error}");
    assert_eq!(
        fs::read_to_string(&summary).expect("read published summary"),
        "new summary\n"
    );
    assert_eq!(
        fs::read_to_string(&report).expect("read intervening report"),
        "intervening report\n"
    );
    fs::remove_dir_all(directory).expect("remove test directory");
}

#[test]
fn startup_scavenging_removes_only_owned_stale_staged_outputs() {
    let unique = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .expect("clock after epoch")
        .as_nanos();
    let directory = std::env::temp_dir().join(format!(
        "key-insights-stage-scavenging-{}-{unique}",
        std::process::id()
    ));
    fs::create_dir(&directory).expect("create test directory");
    let input = directory.join("input.jsonl");
    let summary = directory.join("summary.json");
    let report = directory.join("report.md");
    fs::write(&input, "raw input\n").expect("write input");
    let paths = resolve_paths(&input, &summary, &report).expect("resolve safe paths");
    let dead_pid = if std::process::id() == 1 { 2 } else { 1 };
    assert!(super::staged_output_process_is_alive(std::process::id()));

    let stale_name =
        super::staged_output_name(summary.file_name().expect("summary name"), dead_pid, 1);
    drop(
        paths
            .summary
            .directory
            .open_private_new_file(&stale_name)
            .expect("create stale staged output"),
    );
    let renamed_output_stale_name =
        super::staged_output_name(std::ffi::OsStr::new("old-summary.json"), dead_pid, 1);
    drop(
        paths
            .summary
            .directory
            .open_private_new_file(&renamed_output_stale_name)
            .expect("create stale stage for a prior output name"),
    );

    let live = StagedOutput::create(&paths.report, b"live report\n").expect("create live stage");
    let live_name = live
        .temporary_name
        .as_ref()
        .expect("live stage name")
        .clone();
    assert!(live_name.to_string_lossy().starts_with(".key-insights.v1."));
    assert!(live_name.len() <= 255);

    let wrong_permissions_name =
        super::staged_output_name(summary.file_name().expect("summary name"), dead_pid, 2);
    let wrong_permissions_path = directory.join(&wrong_permissions_name);
    fs::write(&wrong_permissions_path, "unrelated\n").expect("write unrelated file");
    fs::set_permissions(&wrong_permissions_path, fs::Permissions::from_mode(0o644))
        .expect("set unrelated permissions");
    let linked_name =
        super::staged_output_name(summary.file_name().expect("summary name"), dead_pid, 4);
    let linked_path = directory.join(&linked_name);
    drop(
        paths
            .summary
            .directory
            .open_private_new_file(&linked_name)
            .expect("create linked unrelated file"),
    );
    let linked_peer = directory.join("unrelated-hard-link");
    fs::hard_link(&linked_path, &linked_peer).expect("link unrelated file");
    let malformed_name = format!(
        ".key-insights.v1.{}.stage-not-a-process",
        super::bounded_file_label(summary.file_name().expect("summary name"))
    );
    fs::write(directory.join(&malformed_name), "unrelated\n").expect("write malformed file");
    let symlink_name =
        super::staged_output_name(summary.file_name().expect("summary name"), dead_pid, 3);
    symlink(&input, directory.join(&symlink_name)).expect("create unrelated symlink");

    super::recover_outputs_anchored_with_scavenger(
        &paths.summary,
        &paths.report,
        super::current_unix_time_seconds().expect("current time"),
        |_| false,
    )
    .expect("preserve fresh staged outputs");

    assert!(directory.join(&stale_name).exists());
    assert!(directory.join(&renamed_output_stale_name).exists());

    super::recover_outputs_anchored_with_scavenger(
        &paths.summary,
        &paths.report,
        u64::MAX,
        |pid| pid == std::process::id(),
    )
    .expect("recover and scavenge outputs");

    assert!(!directory.join(stale_name).exists());
    assert!(!directory.join(renamed_output_stale_name).exists());
    assert!(directory.join(live_name).exists());
    assert!(wrong_permissions_path.exists());
    assert!(linked_path.exists());
    assert!(linked_peer.exists());
    assert!(directory.join(malformed_name).exists());
    assert!(
        fs::symlink_metadata(directory.join(symlink_name))
            .expect("inspect unrelated symlink")
            .file_type()
            .is_symlink()
    );
    assert_eq!(
        fs::read_to_string(&input).expect("read input"),
        "raw input\n"
    );

    drop(live);
    fs::remove_dir_all(directory).expect("remove test directory");
}

#[test]
fn startup_scavenging_bounds_each_cleanup_pass() {
    let unique = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .expect("clock after epoch")
        .as_nanos();
    let directory = std::env::temp_dir().join(format!(
        "key-insights-stage-cleanup-bound-{}-{unique}",
        std::process::id()
    ));
    fs::create_dir(&directory).expect("create test directory");
    let input = directory.join("input.jsonl");
    let summary = directory.join("summary.json");
    let report = directory.join("report.md");
    fs::write(&input, "raw input\n").expect("write input");
    let paths = resolve_paths(&input, &summary, &report).expect("resolve safe paths");
    let dead_pid = if std::process::id() == 1 { 2 } else { 1 };
    let names: Vec<_> = (0..=super::MAX_STAGE_REMOVALS)
        .map(|identifier| {
            super::staged_output_name(
                summary.file_name().expect("summary name"),
                dead_pid,
                identifier as u64,
            )
        })
        .collect();
    for name in &names {
        drop(
            paths
                .summary
                .directory
                .open_private_new_file(name)
                .expect("create stale staged output"),
        );
    }

    super::recover_outputs_anchored_with_scavenger(&paths.summary, &paths.report, u64::MAX, |_| {
        false
    })
    .expect("run bounded cleanup pass");

    assert_eq!(
        names
            .iter()
            .filter(|name| directory.join(name).exists())
            .count(),
        1
    );

    super::recover_outputs_anchored_with_scavenger(&paths.summary, &paths.report, u64::MAX, |_| {
        false
    })
    .expect("run eventual cleanup pass");
    assert!(names.iter().all(|name| !directory.join(name).exists()));

    fs::remove_dir_all(directory).expect("remove test directory");
}

#[test]
fn staged_output_rename_does_not_follow_a_swapped_symlink() {
    let unique = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .expect("clock after epoch")
        .as_nanos();
    let directory = std::env::temp_dir().join(format!(
        "key-insights-publication-{}-{unique}",
        std::process::id()
    ));
    fs::create_dir(&directory).expect("create test directory");
    let input = directory.join("input.jsonl");
    let summary = directory.join("summary.json");
    let report = directory.join("report.md");
    fs::write(&input, "raw input\n").expect("write input");
    let paths = resolve_paths(&input, &summary, &report).expect("resolve safe paths");

    symlink(&input, &summary).expect("swap output to input symlink");
    StagedOutput::create(&paths.summary, b"summary\n")
        .expect("stage output")
        .publish()
        .expect("publish output");

    assert_eq!(
        fs::read_to_string(&input).expect("input survives"),
        "raw input\n"
    );
    assert_eq!(
        fs::read_to_string(&summary).expect("read output"),
        "summary\n"
    );
    assert!(
        !fs::symlink_metadata(&summary)
            .expect("output metadata")
            .file_type()
            .is_symlink()
    );
    fs::remove_dir_all(directory).expect("remove test directory");
}

#[test]
fn staging_rejects_a_swapped_output_ancestor() {
    let unique = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .expect("clock after epoch")
        .as_nanos();
    let directory = std::env::temp_dir().join(format!(
        "key-insights-ancestor-swap-{}-{unique}",
        std::process::id()
    ));
    let output_directory = directory.join("outputs");
    let moved_directory = directory.join("moved-outputs");
    let attacker_directory = directory.join("attacker");
    fs::create_dir_all(&output_directory).expect("create output directory");
    fs::create_dir(&attacker_directory).expect("create attacker directory");
    let input = directory.join("input.jsonl");
    let summary = output_directory.join("summary.json");
    let report = output_directory.join("report.md");
    fs::write(&input, "raw input\n").expect("write input");
    let paths = resolve_paths(&input, &summary, &report).expect("resolve safe paths");

    fs::rename(&output_directory, &moved_directory).expect("move resolved output directory");
    symlink(&attacker_directory, &output_directory).expect("replace ancestor with symlink");

    let error = match StagedOutput::create(&paths.summary, b"summary\n") {
        Ok(_) => panic!("swapped ancestor must reject staging"),
        Err(error) => error,
    };
    assert!(
        error.contains("output directory changed"),
        "unexpected staging error: {error}"
    );
    assert!(
        !attacker_directory.join("summary.json").exists(),
        "staging must not follow the replacement ancestor"
    );
    assert!(
        !moved_directory.join("summary.json").exists(),
        "rejected staging must not create output in the moved directory"
    );
    fs::remove_dir_all(directory).expect("remove test directory");
}

#[test]
fn startup_recovery_rejects_a_swapped_output_ancestor() {
    let unique = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .expect("clock after epoch")
        .as_nanos();
    let directory = std::env::temp_dir().join(format!(
        "key-insights-recovery-ancestor-swap-{}-{unique}",
        std::process::id()
    ));
    let output_directory = directory.join("outputs");
    let moved_directory = directory.join("moved-outputs");
    let attacker_directory = directory.join("attacker");
    fs::create_dir_all(&output_directory).expect("create output directory");
    fs::create_dir(&attacker_directory).expect("create attacker directory");
    let input = directory.join("input.jsonl");
    let summary = output_directory.join("summary.json");
    let report = output_directory.join("report.md");
    fs::write(&input, "raw input\n").expect("write input");
    let paths = resolve_paths(&input, &summary, &report).expect("resolve safe paths");

    fs::rename(&output_directory, &moved_directory).expect("move resolved output directory");
    symlink(&attacker_directory, &output_directory).expect("replace ancestor with symlink");

    let error = super::recover_outputs_anchored(&paths.summary, &paths.report)
        .expect_err("swapped ancestor must reject startup recovery");
    assert!(
        error.contains("output directory changed"),
        "unexpected recovery error: {error}"
    );
    assert_eq!(
        fs::read_dir(&attacker_directory)
            .expect("read attacker directory")
            .count(),
        0,
        "startup recovery must not follow the replacement ancestor"
    );
    fs::remove_dir_all(directory).expect("remove test directory");
}

#[test]
fn startup_recovery_does_not_follow_an_ancestor_swapped_after_index_read() {
    use std::panic::{AssertUnwindSafe, catch_unwind};

    let unique = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .expect("clock after epoch")
        .as_nanos();
    let directory = std::env::temp_dir().join(format!(
        "key-insights-mid-recovery-ancestor-swap-{}-{unique}",
        std::process::id()
    ));
    let output_directory = directory.join("outputs");
    let moved_directory = directory.join("moved-outputs");
    let attacker_directory = directory.join("attacker");
    fs::create_dir_all(&output_directory).expect("create output directory");
    fs::create_dir(&attacker_directory).expect("create attacker directory");
    let input = directory.join("input.jsonl");
    let summary = output_directory.join("summary.json");
    let report = output_directory.join("report.md");
    fs::write(&input, "raw input\n").expect("write input");
    fs::write(&summary, "old summary\n").expect("write old summary");
    fs::write(&report, "old report\n").expect("write old report");
    let paths = resolve_paths(&input, &summary, &report).expect("resolve recovery paths");
    let interrupted = catch_unwind(AssertUnwindSafe(|| {
        publish_pair_with_hook(
            StagedOutput::create(&paths.summary, b"new summary\n").expect("stage summary"),
            StagedOutput::create(&paths.report, b"new report\n").expect("stage report"),
            || panic!("interrupt publication after summary"),
        )
    }));
    assert!(interrupted.is_err(), "publication must be interrupted");
    let outputs = [&paths.summary, &paths.report];

    let error = super::recover_destination_anchored_with_hook(&paths.summary, outputs, || {
        fs::rename(&output_directory, &moved_directory).expect("move resolved output directory");
        symlink(&attacker_directory, &output_directory).expect("replace ancestor with symlink");
    })
    .expect_err("ancestor swap must fail closed after anchored recovery");

    assert!(
        error.contains("output directory changed"),
        "unexpected recovery error: {error}"
    );
    assert_eq!(
        fs::read_to_string(moved_directory.join("summary.json")).expect("read restored summary"),
        "old summary\n"
    );
    assert_eq!(
        fs::read_to_string(moved_directory.join("report.md")).expect("read old report"),
        "old report\n"
    );
    assert_eq!(
        fs::read_dir(&attacker_directory)
            .expect("read attacker directory")
            .count(),
        0,
        "startup recovery must not follow the replacement ancestor"
    );
    fs::remove_dir_all(directory).expect("remove test directory");
}

#[test]
fn publication_rejects_an_ancestor_swapped_after_staging() {
    let unique = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .expect("clock after epoch")
        .as_nanos();
    let directory = std::env::temp_dir().join(format!(
        "key-insights-publication-ancestor-swap-{}-{unique}",
        std::process::id()
    ));
    let output_directory = directory.join("outputs");
    let moved_directory = directory.join("moved-outputs");
    let attacker_directory = directory.join("attacker");
    fs::create_dir_all(&output_directory).expect("create output directory");
    fs::create_dir(&attacker_directory).expect("create attacker directory");
    let input = directory.join("input.jsonl");
    let summary = output_directory.join("summary.json");
    let report = output_directory.join("report.md");
    fs::write(&input, "raw input\n").expect("write input");
    let paths = resolve_paths(&input, &summary, &report).expect("resolve safe paths");
    let summary_output = StagedOutput::create(&paths.summary, b"summary\n").expect("stage summary");
    let report_output = StagedOutput::create(&paths.report, b"report\n").expect("stage report");

    fs::rename(&output_directory, &moved_directory).expect("move resolved output directory");
    symlink(&attacker_directory, &output_directory).expect("replace ancestor with symlink");

    let error = publish_pair(summary_output, report_output)
        .expect_err("swapped ancestor must reject publication");
    assert!(
        error.contains("output directory changed"),
        "unexpected publication error: {error}"
    );
    assert_eq!(
        fs::read_dir(&attacker_directory)
            .expect("read attacker directory")
            .count(),
        0,
        "publication must not create attacker-controlled artifacts"
    );
    assert!(
        !moved_directory.join("summary.json").exists()
            && !moved_directory.join("report.md").exists(),
        "rejected publication must not create public outputs in the moved directory"
    );
    fs::remove_dir_all(directory).expect("remove test directory");
}

#[test]
fn lock_acquisition_does_not_follow_an_ancestor_swap() {
    let unique = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .expect("clock after epoch")
        .as_nanos();
    let directory = std::env::temp_dir().join(format!(
        "key-insights-lock-ancestor-swap-{}-{unique}",
        std::process::id()
    ));
    let output_directory = directory.join("outputs");
    let moved_directory = directory.join("moved-outputs");
    let attacker_directory = directory.join("attacker");
    fs::create_dir_all(&output_directory).expect("create output directory");
    fs::create_dir(&attacker_directory).expect("create attacker directory");
    let summary = output_directory.join("summary.json");
    let report = output_directory.join("report.md");
    let summary_output = StagedOutput::create(&summary, b"summary\n").expect("stage summary");
    let report_output = StagedOutput::create(&report, b"report\n").expect("stage report");

    let error =
        match OutputLocks::acquire_anchored_with_hook(&summary_output, &report_output, || {
            fs::rename(&output_directory, &moved_directory)
                .expect("move resolved output directory");
            symlink(&attacker_directory, &output_directory).expect("replace ancestor with symlink");
        }) {
            Ok(_) => panic!("ancestor swap must reject lock acquisition"),
            Err(error) => error,
        };

    assert!(
        error.contains("output directory changed"),
        "unexpected lock error: {error}"
    );
    assert_eq!(
        fs::read_dir(&attacker_directory)
            .expect("read attacker directory")
            .count(),
        0,
        "lock acquisition must not follow the replacement ancestor"
    );
    drop(summary_output);
    drop(report_output);
    fs::remove_dir_all(directory).expect("remove test directory");
}

#[test]
fn alias_spellings_prepare_locks_in_the_same_filesystem_order() {
    let unique = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .expect("clock after epoch")
        .as_nanos();
    let directory = std::env::temp_dir().join(format!(
        "key-insights-lock-alias-order-{}-{unique}",
        std::process::id()
    ));
    fs::create_dir(&directory).expect("create test directory");
    let upper_a = directory.join("A");
    let lower_a = directory.join("a");
    let lower_b = directory.join("b");
    let upper_b = directory.join("B");
    let aliases = super::output_paths_may_collide(&upper_a, &lower_a).expect("probe A aliases")
        && super::output_paths_may_collide(&lower_b, &upper_b).expect("probe B aliases");
    if !aliases {
        fs::remove_dir_all(directory).expect("remove case-sensitive test directory");
        return;
    }

    let first_a = super::resolve_output_path(&upper_a).expect("resolve A");
    let first_b = super::resolve_output_path(&lower_b).expect("resolve b");
    let second_a = super::resolve_output_path(&lower_a).expect("resolve a");
    let second_b = super::resolve_output_path(&upper_b).expect("resolve B");
    let first_order: Vec<_> = super::prepare_anchored_locks(&first_a, &first_b)
        .expect("prepare first lock set")
        .into_iter()
        .map(|prepared| prepared.identity)
        .collect();
    let second_order: Vec<_> = super::prepare_anchored_locks(&second_a, &second_b)
        .expect("prepare aliased lock set")
        .into_iter()
        .map(|prepared| prepared.identity)
        .collect();

    assert_eq!(
        first_order, second_order,
        "alias spellings must not reverse physical lock order"
    );
    fs::remove_dir_all(directory).expect("remove test directory");
}

#[test]
fn prepared_locks_follow_filesystem_identity_order() {
    let unique = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .expect("clock after epoch")
        .as_nanos();
    let directory = std::env::temp_dir().join(format!(
        "key-insights-lock-identity-order-{}-{unique}",
        std::process::id()
    ));
    fs::create_dir(&directory).expect("create test directory");
    let first = super::resolve_output_path(&directory.join("first")).expect("resolve first output");
    let second =
        super::resolve_output_path(&directory.join("second")).expect("resolve second output");

    let identities: Vec<_> = super::prepare_anchored_locks(&first, &second)
        .expect("prepare lock set")
        .into_iter()
        .map(|prepared| prepared.identity)
        .collect();

    assert!(
        identities.windows(2).all(|pair| pair[0] < pair[1]),
        "prepared locks must be strictly ordered and deduplicated by identity"
    );
    fs::remove_dir_all(directory).expect("remove test directory");
}

#[test]
fn publication_rolls_back_when_an_ancestor_is_swapped_after_the_first_output() {
    let unique = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .expect("clock after epoch")
        .as_nanos();
    let directory = std::env::temp_dir().join(format!(
        "key-insights-mid-publication-ancestor-swap-{}-{unique}",
        std::process::id()
    ));
    let output_directory = directory.join("outputs");
    let moved_directory = directory.join("moved-outputs");
    let attacker_directory = directory.join("attacker");
    fs::create_dir_all(&output_directory).expect("create output directory");
    fs::create_dir(&attacker_directory).expect("create attacker directory");
    let summary = output_directory.join("summary.json");
    let report = output_directory.join("report.md");
    fs::write(&summary, "old summary\n").expect("write old summary");
    fs::write(&report, "old report\n").expect("write old report");
    let summary_output = StagedOutput::create(&summary, b"new summary\n").expect("stage summary");
    let report_output = StagedOutput::create(&report, b"new report\n").expect("stage report");

    let error = publish_pair_with_hook(summary_output, report_output, || {
        fs::rename(&output_directory, &moved_directory).expect("move resolved output directory");
        symlink(&attacker_directory, &output_directory).expect("replace ancestor with symlink");
    })
    .expect_err("swapped ancestor must fail publication and roll back");

    assert!(
        error.contains("output directory changed"),
        "unexpected publication error: {error}"
    );
    assert_eq!(
        fs::read_to_string(moved_directory.join("summary.json")).expect("read restored summary"),
        "old summary\n"
    );
    assert_eq!(
        fs::read_to_string(moved_directory.join("report.md")).expect("read restored report"),
        "old report\n"
    );
    assert_eq!(
        fs::read_dir(&attacker_directory)
            .expect("read attacker directory")
            .count(),
        0,
        "rollback must not follow the replacement ancestor"
    );
    fs::remove_dir_all(directory).expect("remove test directory");
}

#[test]
fn failed_second_publication_restores_the_previous_pair() {
    let unique = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .expect("clock after epoch")
        .as_nanos();
    let directory = std::env::temp_dir().join(format!(
        "key-insights-rollback-{}-{unique}",
        std::process::id()
    ));
    fs::create_dir(&directory).expect("create test directory");
    let directory = fs::canonicalize(directory).expect("canonicalize test directory");
    let summary = directory.join("summary.json");
    let report = directory.join("report.md");
    fs::write(&summary, "old summary\n").expect("write old summary");
    fs::write(&report, "old report\n").expect("write old report");
    let summary_output = StagedOutput::create(&summary, b"new summary\n").expect("stage summary");
    let report_output = StagedOutput::create(&report, b"new report\n").expect("stage report");
    fs::remove_file(report_output.temporary_path()).expect("force second publication failure");

    publish_pair(summary_output, report_output).expect_err("publication must fail");

    assert_eq!(
        fs::read_to_string(&summary).expect("read summary"),
        "old summary\n"
    );
    assert_eq!(
        fs::read_to_string(&report).expect("read report"),
        "old report\n"
    );
    fs::remove_dir_all(directory).expect("remove test directory");
}

#[test]
fn next_publication_recovers_a_pair_interrupted_after_the_summary() {
    use std::panic::{AssertUnwindSafe, catch_unwind};

    let unique = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .expect("clock after epoch")
        .as_nanos();
    let directory = std::env::temp_dir().join(format!(
        "key-insights-interrupted-pair-{}-{unique}",
        std::process::id()
    ));
    fs::create_dir(&directory).expect("create test directory");
    let directory = fs::canonicalize(directory).expect("canonicalize test directory");
    let summary = directory.join("summary.json");
    let report = directory.join("report.md");
    fs::write(&summary, "old summary\n").expect("write old summary");
    fs::write(&report, "old report\n").expect("write old report");
    let interrupted_summary =
        StagedOutput::create(&summary, b"interrupted summary\n").expect("stage summary");
    let interrupted_report =
        StagedOutput::create(&report, b"interrupted report\n").expect("stage report");

    let interrupted = catch_unwind(AssertUnwindSafe(|| {
        publish_pair_with_hook(interrupted_summary, interrupted_report, || {
            panic!("simulate process termination after summary publication");
        })
    }));
    assert!(interrupted.is_err(), "publication must be interrupted");
    assert_eq!(
        fs::read_to_string(&summary).expect("read interrupted summary"),
        "interrupted summary\n"
    );
    assert_eq!(
        fs::read_to_string(&report).expect("read old report"),
        "old report\n"
    );

    let empty_input = directory.join("empty.jsonl");
    fs::write(&empty_input, "").expect("write invalid retry input");
    let retry_error = super::run(vec![
        "analyze".into(),
        empty_input.into_os_string(),
        "--summary".into(),
        summary.clone().into_os_string(),
        "--report".into(),
        report.clone().into_os_string(),
    ])
    .expect_err("analysis must fail after startup recovery");
    assert!(
        retry_error.contains("session"),
        "recovery must complete before validation: {retry_error}"
    );

    assert_eq!(
        fs::read_to_string(&summary).expect("read recovered summary"),
        "old summary\n"
    );
    assert_eq!(
        fs::read_to_string(&report).expect("read recovered report"),
        "old report\n"
    );
    fs::remove_dir_all(directory).expect("remove test directory");
}

#[test]
fn interrupted_publication_recovers_outputs_that_were_previously_absent() {
    use std::panic::{AssertUnwindSafe, catch_unwind};

    let unique = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .expect("clock after epoch")
        .as_nanos();
    let directory = std::env::temp_dir().join(format!(
        "key-insights-interrupted-absent-pair-{}-{unique}",
        std::process::id()
    ));
    fs::create_dir(&directory).expect("create test directory");
    let summary = directory.join("summary.json");
    let report = directory.join("report.md");

    let interrupted_summary =
        StagedOutput::create(&summary, b"interrupted summary\n").expect("stage summary");
    let interrupted_report =
        StagedOutput::create(&report, b"interrupted report\n").expect("stage report");
    let interrupted = catch_unwind(AssertUnwindSafe(|| {
        publish_pair_with_hook(interrupted_summary, interrupted_report, || {
            panic!("simulate process termination after summary publication");
        })
    }));
    assert!(interrupted.is_err(), "publication must be interrupted");

    let retry_summary =
        StagedOutput::create(&summary, b"retry summary\n").expect("stage retry summary");
    let retry_report =
        StagedOutput::create(&report, b"retry report\n").expect("stage retry report");
    fs::remove_file(retry_summary.temporary_path()).expect("force retry publication failure");
    publish_pair(retry_summary, retry_report).expect_err("retry must fail after recovery");

    assert!(!summary.exists(), "previously absent summary stays absent");
    assert!(!report.exists(), "previously absent report stays absent");
    fs::remove_dir_all(directory).expect("remove test directory");
}

#[test]
fn committed_recovery_keeps_the_new_pair() {
    let unique = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .expect("clock after epoch")
        .as_nanos();
    let directory = std::env::temp_dir().join(format!(
        "key-insights-committed-pair-{}-{unique}",
        std::process::id()
    ));
    fs::create_dir(&directory).expect("create test directory");
    let summary = directory.join("summary.json");
    let report = directory.join("report.md");
    fs::write(&summary, "old summary\n").expect("write old summary");
    fs::write(&report, "old report\n").expect("write old report");
    let summary_output = StagedOutput::create(&summary, b"new summary\n").expect("stage summary");
    let report_output = StagedOutput::create(&report, b"new report\n").expect("stage report");
    let _locks = OutputLocks::acquire(&summary, &report).expect("acquire locks");
    let publication = super::PairPublication::begin(&summary, &report).expect("begin pair");
    summary_output.publish().expect("publish summary");
    report_output.publish().expect("publish report");
    super::sync_output_directories(&summary, &report).expect("sync pair");
    link_without_replacement(&publication.paths.active, &publication.paths.committed)
        .expect("install committed marker");
    super::sync_parent_directory(&publication.paths.committed).expect("sync marker");
    drop(publication);
    drop(_locks);

    let retry_summary =
        StagedOutput::create(&summary, b"retry summary\n").expect("stage retry summary");
    let retry_report =
        StagedOutput::create(&report, b"retry report\n").expect("stage retry report");
    fs::remove_file(retry_summary.temporary_path()).expect("force retry publication failure");
    publish_pair(retry_summary, retry_report).expect_err("retry must fail after recovery");

    assert_eq!(
        fs::read_to_string(&summary).expect("read committed summary"),
        "new summary\n"
    );
    assert_eq!(
        fs::read_to_string(&report).expect("read committed report"),
        "new report\n"
    );
    fs::remove_dir_all(directory).expect("remove test directory");
}

#[test]
fn rollback_recovery_is_idempotent_after_one_output_was_restored() {
    let unique = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .expect("clock after epoch")
        .as_nanos();
    let directory = std::env::temp_dir().join(format!(
        "key-insights-partial-recovery-{}-{unique}",
        std::process::id()
    ));
    fs::create_dir(&directory).expect("create test directory");
    let summary = directory.join("summary.json");
    let report = directory.join("report.md");
    fs::write(&summary, "old summary\n").expect("write old summary");
    fs::write(&report, "old report\n").expect("write old report");
    let summary_output = StagedOutput::create(&summary, b"new summary\n").expect("stage summary");
    let _locks = OutputLocks::acquire(&summary, &report).expect("acquire locks");
    let mut publication = super::PairPublication::begin(&summary, &report).expect("begin pair");
    summary_output.publish().expect("publish summary");
    publication
        .summary_backup
        .restore()
        .expect("partially restore summary");
    drop(publication);
    drop(_locks);

    super::recover_outputs(&summary, &report).expect("repeat interrupted rollback");

    assert_eq!(
        fs::read_to_string(&summary).expect("read restored summary"),
        "old summary\n"
    );
    assert_eq!(
        fs::read_to_string(&report).expect("read restored report"),
        "old report\n"
    );
    fs::remove_dir_all(directory).expect("remove test directory");
}

#[test]
fn reusing_one_destination_recovers_its_previous_transaction_first() {
    use std::panic::{AssertUnwindSafe, catch_unwind};

    let unique = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .expect("clock after epoch")
        .as_nanos();
    let directory = std::env::temp_dir().join(format!(
        "key-insights-reused-destination-{}-{unique}",
        std::process::id()
    ));
    fs::create_dir(&directory).expect("create test directory");
    let summary = directory.join("summary.json");
    let report_a = directory.join("report-a.md");
    let report_b = directory.join("report-b.md");
    fs::write(&summary, "old summary\n").expect("write old summary");
    fs::write(&report_a, "old report A\n").expect("write old report A");

    let interrupted_summary =
        StagedOutput::create(&summary, b"interrupted summary A\n").expect("stage summary A");
    let interrupted_report =
        StagedOutput::create(&report_a, b"interrupted report A\n").expect("stage report A");
    let interrupted = catch_unwind(AssertUnwindSafe(|| {
        publish_pair_with_hook(interrupted_summary, interrupted_report, || {
            panic!("simulate process termination after summary A");
        })
    }));
    assert!(interrupted.is_err(), "publication A must be interrupted");

    publish_pair(
        StagedOutput::create(&summary, b"summary B\n").expect("stage summary B"),
        StagedOutput::create(&report_b, b"report B\n").expect("stage report B"),
    )
    .expect("publish pair B after recovering summary A");
    super::recover_outputs(&summary, &report_a).expect("recover stale pair A state");

    assert_eq!(
        fs::read_to_string(&summary).expect("read summary B"),
        "summary B\n",
        "stale pair A must not revert the newer successful summary"
    );
    assert_eq!(
        fs::read_to_string(&report_b).expect("read report B"),
        "report B\n"
    );
    fs::remove_dir_all(directory).expect("remove test directory");
}

#[test]
fn recovery_restarts_after_the_last_destination_cleanup() {
    use std::panic::{AssertUnwindSafe, catch_unwind};

    let unique = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .expect("clock after epoch")
        .as_nanos();
    let directory = std::env::temp_dir().join(format!(
        "key-insights-recovery-cleanup-crash-{}-{unique}",
        std::process::id()
    ));
    fs::create_dir(&directory).expect("create test directory");
    let summary = directory.join("summary.json");
    let report = directory.join("report.md");
    fs::write(&summary, "old summary\n").expect("write old summary");
    fs::write(&report, "old report\n").expect("write old report");

    let interrupted = catch_unwind(AssertUnwindSafe(|| {
        publish_pair_with_hook(
            StagedOutput::create(&summary, b"new summary\n").expect("stage summary"),
            StagedOutput::create(&report, b"new report\n").expect("stage report"),
            || panic!("interrupt publication after summary"),
        )
    }));
    assert!(interrupted.is_err(), "publication must be interrupted");
    super::recover_destination(&summary).expect("recover first destination");

    let cleanup_interrupted = catch_unwind(AssertUnwindSafe(|| {
        super::recover_destination_with_hook(&report, || {
            panic!("interrupt after final destination cleanup");
        })
    }));
    assert!(
        cleanup_interrupted.is_err(),
        "recovery cleanup must be interrupted"
    );

    super::recover_outputs(&summary, &report).expect("restart recovery cleanup");
    assert_eq!(
        fs::read_to_string(&summary).expect("read recovered summary"),
        "old summary\n"
    );
    assert_eq!(
        fs::read_to_string(&report).expect("read recovered report"),
        "old report\n"
    );
    fs::remove_dir_all(directory).expect("remove test directory");
}

#[test]
fn recovery_restarts_after_the_first_destination_cleanup() {
    use std::panic::{AssertUnwindSafe, catch_unwind};

    let unique = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .expect("clock after epoch")
        .as_nanos();
    let directory = std::env::temp_dir().join(format!(
        "key-insights-first-recovery-cleanup-crash-{}-{unique}",
        std::process::id()
    ));
    fs::create_dir(&directory).expect("create test directory");
    let summary = directory.join("summary.json");
    let report = directory.join("report.md");
    fs::write(&summary, "old summary\n").expect("write old summary");
    fs::write(&report, "old report\n").expect("write old report");

    let interrupted = catch_unwind(AssertUnwindSafe(|| {
        publish_pair_with_hook(
            StagedOutput::create(&summary, b"new summary\n").expect("stage summary"),
            StagedOutput::create(&report, b"new report\n").expect("stage report"),
            || panic!("interrupt publication after summary"),
        )
    }));
    assert!(interrupted.is_err(), "publication must be interrupted");

    let cleanup_interrupted = catch_unwind(AssertUnwindSafe(|| {
        super::recover_destination_with_hook(&summary, || {
            panic!("interrupt after first destination cleanup");
        })
    }));
    assert!(
        cleanup_interrupted.is_err(),
        "recovery cleanup must be interrupted"
    );

    super::recover_outputs(&summary, &report).expect("restart recovery cleanup");
    assert_eq!(
        fs::read_to_string(&summary).expect("read recovered summary"),
        "old summary\n"
    );
    assert_eq!(
        fs::read_to_string(&report).expect("read recovered report"),
        "old report\n"
    );
    fs::remove_dir_all(directory).expect("remove test directory");
}

#[test]
fn partial_sidecar_write_is_never_published_and_retry_succeeds() {
    use std::io::Write;

    let unique = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .expect("clock after epoch")
        .as_nanos();
    let directory = std::env::temp_dir().join(format!(
        "key-insights-sidecar-write-failure-{}-{unique}",
        std::process::id()
    ));
    fs::create_dir(&directory).expect("create test directory");
    let sidecar = directory.join("recovery-index");

    super::publish_private_sidecar_with(&sidecar, b"complete", |file, _contents| {
        file.write_all(b"partial")?;
        Err(std::io::Error::new(
            std::io::ErrorKind::StorageFull,
            "injected sidecar write failure",
        ))
    })
    .expect_err("partial sidecar write must fail");
    assert!(
        !sidecar.exists(),
        "partial final sidecar must not be visible"
    );

    super::publish_private_sidecar_with(&sidecar, b"complete", |file, contents| {
        file.write_all(contents)?;
        file.sync_all()
    })
    .expect("retry sidecar publication");
    assert_eq!(fs::read(&sidecar).expect("read sidecar"), b"complete");
    fs::remove_dir_all(directory).expect("remove test directory");
}

#[test]
fn sidecar_publication_never_replaces_an_existing_entry() {
    use std::io::Write;

    let unique = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .expect("clock after epoch")
        .as_nanos();
    let directory = std::env::temp_dir().join(format!(
        "key-insights-occupied-sidecar-{}-{unique}",
        std::process::id()
    ));
    fs::create_dir(&directory).expect("create test directory");
    let sidecar = directory.join("recovery-marker");
    fs::write(&sidecar, "unrelated\n").expect("write occupied sidecar");

    super::publish_private_sidecar_with(&sidecar, b"ours", |file, contents| {
        file.write_all(contents)?;
        file.sync_all()
    })
    .expect_err("occupied sidecar must reject publication");

    assert_eq!(
        fs::read_to_string(&sidecar).expect("read occupied sidecar"),
        "unrelated\n"
    );
    let temporary_files = fs::read_dir(&directory)
        .expect("read test directory")
        .filter_map(Result::ok)
        .filter(|entry| entry.file_name().to_string_lossy().contains(".tmp-"))
        .count();
    assert_eq!(
        temporary_files, 0,
        "failed publication cleans its temporary file"
    );
    fs::remove_dir_all(directory).expect("remove test directory");
}

#[test]
fn backup_link_never_replaces_an_existing_entry() {
    let unique = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .expect("clock after epoch")
        .as_nanos();
    let directory = std::env::temp_dir().join(format!(
        "key-insights-backup-reservation-{}-{unique}",
        std::process::id()
    ));
    fs::create_dir(&directory).expect("create test directory");
    let source = directory.join("summary.json");
    let occupied_backup = directory.join("occupied-backup");
    fs::write(&source, "previous summary\n").expect("write prior output");
    fs::write(&occupied_backup, "unrelated\n").expect("write unrelated file");

    let error = link_without_replacement(&source, &occupied_backup)
        .expect_err("occupied backup path must not be replaced");

    assert_eq!(error.kind(), std::io::ErrorKind::AlreadyExists);
    assert_eq!(
        fs::read_to_string(&source).expect("source survives"),
        "previous summary\n"
    );
    assert_eq!(
        fs::read_to_string(&occupied_backup).expect("unrelated backup survives"),
        "unrelated\n"
    );
    fs::remove_dir_all(directory).expect("remove test directory");
}

#[test]
fn backup_capture_keeps_the_previous_output_at_its_public_path() {
    let unique = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .expect("clock after epoch")
        .as_nanos();
    let directory = std::env::temp_dir().join(format!(
        "key-insights-linked-backup-{}-{unique}",
        std::process::id()
    ));
    fs::create_dir(&directory).expect("create test directory");
    let destination = directory.join("summary.json");
    fs::write(&destination, "previous summary\n").expect("write prior output");

    let mut backup = OutputBackup::capture(&destination).expect("capture output");

    assert_eq!(
        fs::read_to_string(&destination).expect("public output remains available"),
        "previous summary\n"
    );
    backup.discard().expect("discard backup");
    assert_eq!(
        fs::read_to_string(&destination).expect("public output survives cleanup"),
        "previous summary\n"
    );
    fs::remove_dir_all(directory).expect("remove test directory");
}

#[test]
fn backup_capture_refuses_a_swapped_output_symlink() {
    let unique = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .expect("clock after epoch")
        .as_nanos();
    let directory = std::env::temp_dir().join(format!(
        "key-insights-backup-symlink-{}-{unique}",
        std::process::id()
    ));
    fs::create_dir(&directory).expect("create test directory");
    let destination = directory.join("summary.json");
    let target = directory.join("target");
    fs::write(&target, "unrelated\n").expect("write symlink target");
    symlink(&target, &destination).expect("swap destination to symlink");

    let error = match OutputBackup::capture(&destination) {
        Ok(_) => panic!("symlink must be rejected"),
        Err(error) => error,
    };

    assert!(error.contains("non-regular file"));
    assert!(
        fs::symlink_metadata(&destination)
            .expect("symlink survives")
            .file_type()
            .is_symlink()
    );
    assert_eq!(
        fs::read_to_string(&target).expect("read target"),
        "unrelated\n"
    );
    fs::remove_dir_all(directory).expect("remove test directory");
}

#[test]
fn anchored_backup_rechecks_the_source_after_linking() {
    let unique = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .expect("clock after epoch")
        .as_nanos();
    let directory = std::env::temp_dir().join(format!(
        "key-insights-anchored-backup-race-{}-{unique}",
        std::process::id()
    ));
    fs::create_dir(&directory).expect("create test directory");
    let destination = directory.join("summary.json");
    let target = directory.join("target");
    let backup_path = directory.join("reserved-backup");
    fs::write(&destination, "previous summary\n").expect("write previous output");
    fs::write(&target, "unrelated\n").expect("write symlink target");
    let staged = StagedOutput::create(&destination, b"new summary\n").expect("stage output");

    let error = match OutputBackup::capture_anchored_with_hook(&staged, backup_path.clone(), || {
        fs::remove_file(&destination).expect("remove checked destination");
        symlink(&target, &destination).expect("swap destination to symlink");
    }) {
        Ok(_) => panic!("post-link source replacement must be rejected"),
        Err(error) => error,
    };

    assert!(error.contains("changed while capturing"), "{error}");
    assert!(
        fs::symlink_metadata(&destination)
            .expect("replacement symlink survives")
            .file_type()
            .is_symlink()
    );
    assert!(!backup_path.exists(), "rejected owned backup is removed");
    assert_eq!(
        fs::read_to_string(&target).expect("read symlink target"),
        "unrelated\n"
    );
    drop(staged);
    fs::remove_dir_all(directory).expect("remove test directory");
}

#[test]
fn paired_publication_is_serialized_across_threads() {
    use std::{sync::mpsc, thread, time::Duration};

    let unique = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .expect("clock after epoch")
        .as_nanos();
    let directory = std::env::temp_dir().join(format!(
        "key-insights-pair-lock-{}-{unique}",
        std::process::id()
    ));
    fs::create_dir(&directory).expect("create test directory");
    let summary = directory.join("summary.json");
    let report = directory.join("report.md");
    let a_summary = StagedOutput::create(&summary, b"A summary\n").expect("stage A summary");
    let a_report = StagedOutput::create(&report, b"A report\n").expect("stage A report");
    let b_summary = StagedOutput::create(&summary, b"B summary\n").expect("stage B summary");
    let b_report = StagedOutput::create(&report, b"B report\n").expect("stage B report");
    let (a_reached_tx, a_reached_rx) = mpsc::channel();
    let (release_a_tx, release_a_rx) = mpsc::channel();
    let (b_done_tx, b_done_rx) = mpsc::channel();

    let a = thread::spawn(move || {
        publish_pair_with_hook(a_summary, a_report, || {
            a_reached_tx.send(()).expect("signal A summary");
            release_a_rx.recv().expect("release A report");
        })
    });
    a_reached_rx.recv().expect("A published its summary");
    let competing_lock =
        open_private_lock_file(&output_lock_path(&summary).expect("derive competing summary lock"))
            .expect("open competing summary lock");
    assert!(
        matches!(
            competing_lock
                .try_lock()
                .expect_err("A still holds the summary lock"),
            std::fs::TryLockError::WouldBlock
        ),
        "the competing lock must be blocked by A"
    );
    let b = thread::spawn(move || {
        let result = publish_pair(b_summary, b_report);
        b_done_tx.send(result).expect("signal B completion");
    });

    let early_b_result = b_done_rx.recv_timeout(Duration::from_millis(100)).ok();
    let b_was_blocked = early_b_result.is_none();
    release_a_tx.send(()).expect("release A");
    a.join().expect("join A").expect("publish A pair");
    let b_result = match early_b_result {
        Some(result) => result,
        None => b_done_rx
            .recv_timeout(Duration::from_secs(2))
            .expect("B completes after A"),
    };
    b_result.expect("publish B pair");
    b.join().expect("join B");

    assert!(
        b_was_blocked,
        "B must wait until A has published both artifacts"
    );
    assert_eq!(
        fs::read_to_string(&summary).expect("read summary"),
        "B summary\n"
    );
    assert_eq!(
        fs::read_to_string(&report).expect("read report"),
        "B report\n"
    );
    fs::remove_dir_all(directory).expect("remove test directory");
}

#[test]
fn publication_lock_rejects_a_symlink_without_touching_its_target() {
    let unique = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .expect("clock after epoch")
        .as_nanos();
    let directory = std::env::temp_dir().join(format!(
        "key-insights-lock-symlink-{}-{unique}",
        std::process::id()
    ));
    fs::create_dir(&directory).expect("create test directory");
    let summary = directory.join("summary.json");
    let report = directory.join("report.md");
    let target = directory.join("unrelated");
    fs::write(&target, "unrelated\n").expect("write lock target");
    let summary_lock = output_lock_path(&summary).expect("derive lock path");
    symlink(&target, &summary_lock).expect("create lock symlink");

    let error = match OutputLocks::acquire(&summary, &report) {
        Ok(_) => panic!("symlink lock must be rejected"),
        Err(error) => error,
    };

    assert!(error.contains("lock path is not a regular file"));
    assert_eq!(
        fs::read_to_string(&target).expect("read lock target"),
        "unrelated\n"
    );
    fs::remove_dir_all(directory).expect("remove test directory");
}

#[test]
fn publication_lock_verification_rejects_a_fifo() {
    use std::{ffi::CString, os::unix::ffi::OsStrExt};

    let unique = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .expect("clock after epoch")
        .as_nanos();
    let directory = std::env::temp_dir().join(format!(
        "key-insights-lock-fifo-{}-{unique}",
        std::process::id()
    ));
    fs::create_dir(&directory).expect("create test directory");
    let directory = fs::canonicalize(directory).expect("canonicalize test directory");
    let lock_name = std::ffi::OsStr::new("lock");
    let lock_path = directory.join(lock_name);
    let lock_path_c = CString::new(lock_path.as_os_str().as_bytes()).expect("fifo path");
    assert_eq!(unsafe { libc::mkfifo(lock_path_c.as_ptr(), 0o600) }, 0);
    let lock_file = std::fs::OpenOptions::new()
        .read(true)
        .write(true)
        .open(&lock_path)
        .expect("open fifo");
    let resolved = super::ResolvedDirectory::open(&directory).expect("open directory");

    assert!(
        !resolved
            .open_file_matches_child(&lock_file, lock_name)
            .expect("verify fifo"),
        "non-regular lock must fail post-open verification"
    );
    drop(lock_file);
    fs::remove_dir_all(directory).expect("remove test directory");
}

#[test]
fn publication_lock_names_remain_within_common_filesystem_limits() {
    let destination = PathBuf::from("/tmp").join(format!("{}.json", "x".repeat(240)));

    let lock = output_lock_path(&destination).expect("derive lock path");

    assert!(
        lock.file_name()
            .expect("lock file name")
            .to_string_lossy()
            .len()
            <= 128
    );
}

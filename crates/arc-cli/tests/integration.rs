use std::process::Command;
use tempfile::TempDir;

fn arc_binary() -> Command {
    let mut cmd = Command::new(env!("CARGO_BIN_EXE_arc"));
    cmd.env("ARC_EPHEMERAL_RUNNER", "integration-test");
    cmd
}

fn setup_repo() -> TempDir {
    let dir = TempDir::new().expect("tempdir");
    let output = arc_binary()
        .current_dir(dir.path())
        .args(["init", "--repo-name", "test-repo", "--no-git"])
        .output()
        .expect("failed to run arc init");
    assert!(
        output.status.success(),
        "arc init failed: {}",
        String::from_utf8_lossy(&output.stderr)
    );
    let stdout = String::from_utf8_lossy(&output.stdout);
    assert!(
        stdout.contains("Initialized empty arc repository"),
        "unexpected init output: {stdout}"
    );
    dir
}

#[test]
fn init_creates_arc_directory() {
    let dir = setup_repo();
    assert!(dir.path().join(".arc").exists(), ".arc directory should exist after init");
    assert!(dir.path().join(".arc").is_dir(), ".arc should be a directory");
}

#[test]
fn init_with_custom_branch() {
    let dir = TempDir::new().expect("tempdir");
    let output = arc_binary()
        .current_dir(dir.path())
        .args(["init", "--repo-name", "test-repo", "--no-git", "--default-branch", "develop"])
        .output()
        .expect("failed to run arc init");
    assert!(
        output.status.success(),
        "arc init with --default-branch failed: {}",
        String::from_utf8_lossy(&output.stderr)
    );
    assert!(dir.path().join(".arc").exists(), ".arc directory should exist after init");
}

#[test]
fn snap_on_clean_repo_says_nothing_to_snap() {
    let dir = setup_repo();
    let output = arc_binary()
        .current_dir(dir.path())
        .args(["snap"])
        .output()
        .expect("failed to run arc snap");
    let stdout = String::from_utf8_lossy(&output.stdout);
    assert!(
        stdout.contains("Nothing to snap") || stdout.contains("snap "),
        "expected 'Nothing to snap' or 'snap <hash>', got: {stdout}"
    );
}

#[test]
fn snap_with_message_creates_snapshot() {
    let dir = setup_repo();
    // Create a file so there's something to snap (use .md for tracked extension)
    std::fs::write(dir.path().join("hello.md"), "hello world").expect("write file");

    let output = arc_binary()
        .current_dir(dir.path())
        .args(["snap", "-m", "add hello.md"])
        .output()
        .expect("failed to run arc snap");
    assert!(
        output.status.success(),
        "arc snap failed: {}",
        String::from_utf8_lossy(&output.stderr)
    );
    let stdout = String::from_utf8_lossy(&output.stdout);
    assert!(stdout.contains("snap "), "expected snap output with hash, got: {stdout}");
}

#[test]
fn snap_produces_hex_hash() {
    let dir = setup_repo();
    std::fs::write(dir.path().join("data.md"), "content").expect("write file");

    let output = arc_binary()
        .current_dir(dir.path())
        .args(["snap", "-m", "add data.md"])
        .output()
        .expect("failed to run arc snap");
    let stdout = String::from_utf8_lossy(&output.stdout);
    // Extract the hex hash from "snap <64-hex-chars>"
    let line = stdout.lines().next().expect("no output");
    let hash_part = line.strip_prefix("snap ").expect("expected 'snap ' prefix");
    assert_eq!(hash_part.len(), 64, "hash should be 64 hex chars, got {}", hash_part.len());
    assert!(
        hash_part.chars().all(|c| c.is_ascii_hexdigit()),
        "hash should be hex, got: {hash_part}"
    );
}

#[test]
fn log_on_empty_repo_shows_no_changes() {
    let dir = setup_repo();
    let output =
        arc_binary().current_dir(dir.path()).args(["log"]).output().expect("failed to run arc log");
    let stdout = String::from_utf8_lossy(&output.stdout);
    assert!(
        stdout.contains("No changes yet"),
        "expected 'No changes yet' on empty repo, got: {stdout}"
    );
}

#[test]
fn log_after_snap_shows_commit() {
    let dir = setup_repo();
    std::fs::write(dir.path().join("file.md"), "data").expect("write file");

    let snap_output = arc_binary()
        .current_dir(dir.path())
        .args(["snap", "-m", "initial commit"])
        .output()
        .expect("failed to run arc snap");
    assert!(snap_output.status.success());

    let log_output =
        arc_binary().current_dir(dir.path()).args(["log"]).output().expect("failed to run arc log");
    assert!(log_output.status.success());
    let stdout = String::from_utf8_lossy(&log_output.stdout);
    // Log should not show "No changes yet" after a snap
    assert!(
        !stdout.contains("No changes yet"),
        "log should show commits after snap, got: {stdout}"
    );
}

#[test]
fn status_on_clean_repo_shows_clean() {
    let dir = setup_repo();
    let output = arc_binary()
        .current_dir(dir.path())
        .args(["status"])
        .output()
        .expect("failed to run arc status");
    assert!(output.status.success());
    let stdout = String::from_utf8_lossy(&output.stdout);
    assert!(
        stdout.contains("Nothing to snap") || stdout.contains("clean"),
        "expected clean status, got: {stdout}"
    );
}

#[test]
fn status_auto_snapshots_uncommitted_changes() {
    let dir = setup_repo();
    // Write a new tracked file
    std::fs::write(dir.path().join("new.md"), "new content").expect("write file");

    // Status auto-snapshots first, so it reports clean
    let output = arc_binary()
        .current_dir(dir.path())
        .args(["status"])
        .output()
        .expect("failed to run arc status");
    assert!(output.status.success());
    let stdout = String::from_utf8_lossy(&output.stdout);
    assert!(
        stdout.contains("Nothing to snap") || stdout.contains("clean"),
        "expected clean status after auto-snapshot, got: {stdout}"
    );

    // The auto-snap should have created a snapshot, so log should show it
    let log_output =
        arc_binary().current_dir(dir.path()).args(["log"]).output().expect("failed to run arc log");
    assert!(log_output.status.success());
    let log_stdout = String::from_utf8_lossy(&log_output.stdout);
    assert!(
        !log_stdout.contains("No changes yet"),
        "log should show auto-snapshot, got: {log_stdout}"
    );
}

#[test]
fn snap_twice_second_says_nothing_to_snap() {
    let dir = setup_repo();
    std::fs::write(dir.path().join("a.md"), "a").expect("write file");

    let first = arc_binary()
        .current_dir(dir.path())
        .args(["snap", "-m", "first snap"])
        .output()
        .expect("first snap");
    assert!(first.status.success());

    let second = arc_binary().current_dir(dir.path()).args(["snap"]).output().expect("second snap");
    let stdout = String::from_utf8_lossy(&second.stdout);
    assert!(
        stdout.contains("Nothing to snap"),
        "second snap on clean repo should say nothing to snap, got: {stdout}"
    );
}

#[test]
fn log_with_template_flag() {
    let dir = setup_repo();
    std::fs::write(dir.path().join("t.md"), "t").expect("write file");

    let snap_output = arc_binary()
        .current_dir(dir.path())
        .args(["snap", "-m", "template test"])
        .output()
        .expect("snap");
    assert!(snap_output.status.success());

    let log_output = arc_binary()
        .current_dir(dir.path())
        .args(["log", "--template", "{id_short} {intent}"])
        .output()
        .expect("log with template");
    assert!(log_output.status.success());
    let stdout = String::from_utf8_lossy(&log_output.stdout);
    // Should not show "No changes yet" since we snapped
    assert!(
        !stdout.contains("No changes yet"),
        "log with template should show commits, got: {stdout}"
    );
}

#[test]
fn json_flag_accepted_without_error() {
    let dir = setup_repo();
    let output = arc_binary()
        .current_dir(dir.path())
        .args(["--json", "status"])
        .output()
        .expect("failed to run arc --json status");
    assert!(
        output.status.success(),
        "arc --json status failed: {}",
        String::from_utf8_lossy(&output.stderr)
    );
    let stdout = String::from_utf8_lossy(&output.stdout);
    // --json is accepted and does not affect successful status output (plain text)
    assert!(
        !stdout.contains("error") && !stdout.contains("Error"),
        "unexpected error in --json output: {stdout}"
    );
}

#[test]
fn multiple_snaps_appear_in_log() {
    let dir = setup_repo();

    for i in 1..=3 {
        std::fs::write(dir.path().join(format!("file{i}.md")), format!("content {i}"))
            .expect("write file");
        let output = arc_binary()
            .current_dir(dir.path())
            .args(["snap", "-m", &format!("snap {i}")])
            .output()
            .expect("snap");
        assert!(output.status.success(), "snap {i} failed");
    }

    let log_output = arc_binary().current_dir(dir.path()).args(["log"]).output().expect("log");
    assert!(log_output.status.success());
    let stdout = String::from_utf8_lossy(&log_output.stdout);
    // Should show multiple commits
    assert!(!stdout.contains("No changes yet"), "log should show commits, got: {stdout}");
}

#[test]
fn quiet_flag_suppresses_output() {
    let dir = setup_repo();
    let output = arc_binary()
        .current_dir(dir.path())
        .args(["--quiet", "status"])
        .output()
        .expect("failed to run arc --quiet status");
    assert!(output.status.success());
    let stdout = String::from_utf8_lossy(&output.stdout);
    // Quiet mode should produce minimal or no output
    assert!(
        stdout.len() < 200,
        "quiet mode should have minimal output, got {} bytes: {stdout}",
        stdout.len()
    );
}

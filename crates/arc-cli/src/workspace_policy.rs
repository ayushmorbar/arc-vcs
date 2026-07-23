use std::{fs, path::Path};

use anyhow::Context;
use arc_store_types::newtypes::{ChangeId, SnapshotId};
use tracing::instrument;

/// Summary emitted by workspace policy checks.
#[derive(Debug, Clone)]
pub struct WorkspacePolicyReport {
    /// Root policy files that were validated.
    pub policy_files: Vec<String>,
    /// Number of ignore patterns validated from `.gitignore`.
    pub validated_gitignore_patterns: usize,
    /// Typed frontier evidence attached to this report.
    pub frontier: Vec<ChangeId>,
    /// Typed synthesis snapshot evidence attached to this report.
    pub synthesis_snapshots: Vec<SnapshotId>,
}

const REQUIRED_POLICY_FILES: [&str; 4] =
    [".editorconfig", ".gitattributes", ".watchmanconfig", "rustfmt.toml"];

const REQUIRED_GITIGNORE_PATTERNS: [&str; 4] = ["/target", ".direnv", ".envrc", "/rendered-docs"];

/// Audit root workspace-policy files in read-only mode.
#[instrument(skip(frontier, synthesis_snapshots))]
pub fn audit_workspace_policy(
    repo_root: &Path,
    frontier: Vec<ChangeId>,
    synthesis_snapshots: Vec<SnapshotId>,
) -> anyhow::Result<WorkspacePolicyReport> {
    let policy_files = ensure_required_policy_files(repo_root)?;
    ensure_editorconfig(repo_root)?;
    ensure_gitattributes(repo_root)?;
    let validated_gitignore_patterns = ensure_gitignore(repo_root)?;
    ensure_rustfmt(repo_root)?;

    Ok(WorkspacePolicyReport {
        policy_files,
        validated_gitignore_patterns,
        frontier,
        synthesis_snapshots,
    })
}

#[instrument]
fn ensure_required_policy_files(repo_root: &Path) -> anyhow::Result<Vec<String>> {
    let mut found = Vec::new();
    for file in REQUIRED_POLICY_FILES {
        let path = repo_root.join(file);
        if !path.exists() {
            anyhow::bail!("missing workspace policy file: {}", path.display());
        }
        found.push(file.to_string());
    }
    Ok(found)
}

#[instrument]
fn ensure_editorconfig(repo_root: &Path) -> anyhow::Result<()> {
    let path = repo_root.join(".editorconfig");
    let raw =
        fs::read_to_string(&path).with_context(|| format!("failed to read {}", path.display()))?;

    let lines = effective_lines(&raw);

    if !lines.iter().any(|line| line == "[*.rs]") {
        anyhow::bail!("{}. must define [*.rs] section", path.display());
    }
    if !lines.iter().any(|line| line == "indent_size = 4") {
        anyhow::bail!("{}. must pin Rust indentation width", path.display());
    }
    Ok(())
}

#[instrument]
fn ensure_gitattributes(repo_root: &Path) -> anyhow::Result<()> {
    let path = repo_root.join(".gitattributes");
    let raw =
        fs::read_to_string(&path).with_context(|| format!("failed to read {}", path.display()))?;
    let lines = effective_lines(&raw);

    if !lines.iter().any(|line| line == "Cargo.lock linguist-generated=true merge=binary") {
        anyhow::bail!(
            "{} must mark Cargo.lock as generated with binary merge strategy",
            path.display()
        );
    }
    Ok(())
}

#[instrument]
fn ensure_gitignore(repo_root: &Path) -> anyhow::Result<usize> {
    let path = repo_root.join(".gitignore");
    let raw =
        fs::read_to_string(&path).with_context(|| format!("failed to read {}", path.display()))?;

    let mut count = 0usize;
    for pattern in REQUIRED_GITIGNORE_PATTERNS {
        if raw.lines().any(|line| line.trim() == pattern) {
            count += 1;
        } else {
            anyhow::bail!("{} missing required ignore pattern '{}'", path.display(), pattern);
        }
    }

    Ok(count)
}

#[instrument]
fn ensure_rustfmt(repo_root: &Path) -> anyhow::Result<()> {
    let path = repo_root.join("rustfmt.toml");
    let raw =
        fs::read_to_string(&path).with_context(|| format!("failed to read {}", path.display()))?;
    let parsed: toml::Value = toml::from_str(&raw)
        .with_context(|| format!("failed to parse {} as TOML", path.display()))?;

    if parsed.get("edition").and_then(toml::Value::as_str) != Some("2024") {
        anyhow::bail!("{} must pin edition = \"2024\"", path.display());
    }
    if parsed.get("max_width").and_then(toml::Value::as_integer) != Some(100) {
        anyhow::bail!("{} must pin max_width", path.display());
    }
    Ok(())
}

fn effective_lines(raw: &str) -> Vec<String> {
    raw.lines()
        .map(str::trim)
        .filter(|line| !line.is_empty() && !line.starts_with('#') && !line.starts_with(';'))
        .map(ToString::to_string)
        .collect()
}

#[cfg(test)]
mod tests {
    use std::fs;

    use arc_store_types::newtypes::{ChangeId, SnapshotId};

    use super::audit_workspace_policy;

    fn write_valid_workspace_policy(root: &std::path::Path) {
        fs::write(
            root.join(".editorconfig"),
            "root = true\n\n[*.rs]\nindent_style = space\nindent_size = 4\n",
        )
        .expect("write editorconfig");
        fs::write(root.join(".gitattributes"), "Cargo.lock linguist-generated=true merge=binary\n")
            .expect("write gitattributes");
        fs::write(root.join(".gitignore"), "/target\n/rendered-docs\n.direnv\n.envrc\n")
            .expect("write gitignore");
        fs::write(root.join(".watchmanconfig"), "{}\n").expect("write watchman");
        fs::write(root.join("rustfmt.toml"), "edition = \"2024\"\nmax_width = 100\n")
            .expect("write rustfmt");
    }

    #[test]
    fn workspace_policy_audit_accepts_valid_configuration() {
        let dir = tempfile::tempdir().expect("tempdir");
        write_valid_workspace_policy(dir.path());

        let report =
            audit_workspace_policy(dir.path(), Vec::new(), Vec::new()).expect("audit should pass");
        assert_eq!(report.policy_files.len(), 4);
        assert_eq!(report.validated_gitignore_patterns, 4);
    }

    #[test]
    fn workspace_policy_audit_rejects_missing_pattern() {
        let dir = tempfile::tempdir().expect("tempdir");
        write_valid_workspace_policy(dir.path());
        fs::write(rooted(&dir, ".gitignore"), "/target\n.envrc\n").expect("rewrite gitignore");

        let err = audit_workspace_policy(dir.path(), Vec::new(), Vec::new())
            .expect_err("audit should fail");
        assert!(
            err.to_string().contains("missing required ignore pattern"),
            "unexpected error: {err}"
        );
    }

    #[test]
    fn workspace_policy_audit_rejects_commented_editorconfig_directives() {
        let dir = tempfile::tempdir().expect("tempdir");
        write_valid_workspace_policy(dir.path());
        fs::write(rooted(&dir, ".editorconfig"), "root = true\n# [*.rs]\n# indent_size = 4\n")
            .expect("rewrite editorconfig");

        let err = audit_workspace_policy(dir.path(), Vec::new(), Vec::new())
            .expect_err("audit should fail");
        assert!(err.to_string().contains("must define [*.rs] section"), "unexpected error: {err}");
    }

    #[test]
    fn workspace_policy_audit_rejects_commented_gitattributes_directive() {
        let dir = tempfile::tempdir().expect("tempdir");
        write_valid_workspace_policy(dir.path());
        fs::write(
            rooted(&dir, ".gitattributes"),
            "# Cargo.lock linguist-generated=true merge=binary\n",
        )
        .expect("rewrite gitattributes");

        let err = audit_workspace_policy(dir.path(), Vec::new(), Vec::new())
            .expect_err("audit should fail");
        assert!(
            err.to_string().contains("must mark Cargo.lock as generated"),
            "unexpected error: {err}"
        );
    }

    #[test]
    fn workspace_policy_audit_current_workspace() {
        let crate_dir = std::path::Path::new(env!("CARGO_MANIFEST_DIR"));
        let repo_root = crate_dir.ancestors().nth(2).expect("arc workspace root should exist");

        let report = audit_workspace_policy(repo_root, Vec::new(), Vec::new())
            .expect("current workspace policy must be valid");
        assert_eq!(report.policy_files.len(), 4);
        assert!(report.validated_gitignore_patterns >= 4);
    }

    #[test]
    fn workspace_policy_audit_rejects_missing_policy_file() {
        let dir = tempfile::tempdir().expect("tempdir");
        write_valid_workspace_policy(dir.path());
        fs::remove_file(dir.path().join(".editorconfig")).expect("remove editorconfig");

        let err = audit_workspace_policy(dir.path(), Vec::new(), Vec::new())
            .expect_err("audit should fail");
        assert!(
            err.to_string().contains("missing workspace policy file"),
            "unexpected error: {err}"
        );
    }

    #[test]
    fn workspace_policy_audit_rejects_editorconfig_missing_indent_size() {
        let dir = tempfile::tempdir().expect("tempdir");
        write_valid_workspace_policy(dir.path());
        fs::write(rooted(&dir, ".editorconfig"), "root = true\n\n[*.rs]\nindent_style = space\n")
            .expect("rewrite editorconfig");

        let err = audit_workspace_policy(dir.path(), Vec::new(), Vec::new())
            .expect_err("audit should fail");
        assert!(
            err.to_string().contains("must pin Rust indentation width"),
            "unexpected error: {err}"
        );
    }

    #[test]
    fn workspace_policy_audit_rejects_rustfmt_missing_edition() {
        let dir = tempfile::tempdir().expect("tempdir");
        write_valid_workspace_policy(dir.path());
        fs::write(rooted(&dir, "rustfmt.toml"), "max_width = 100\n").expect("rewrite rustfmt");

        let err = audit_workspace_policy(dir.path(), Vec::new(), Vec::new())
            .expect_err("audit should fail");
        assert!(err.to_string().contains("must pin edition"), "unexpected error: {err}");
    }

    #[test]
    fn workspace_policy_audit_rejects_rustfmt_missing_max_width() {
        let dir = tempfile::tempdir().expect("tempdir");
        write_valid_workspace_policy(dir.path());
        fs::write(rooted(&dir, "rustfmt.toml"), "edition = \"2024\"\n").expect("rewrite rustfmt");

        let err = audit_workspace_policy(dir.path(), Vec::new(), Vec::new())
            .expect_err("audit should fail");
        assert!(err.to_string().contains("must pin max_width"), "unexpected error: {err}");
    }

    #[test]
    fn workspace_policy_audit_rejects_invalid_rustfmt_toml() {
        let dir = tempfile::tempdir().expect("tempdir");
        write_valid_workspace_policy(dir.path());
        fs::write(rooted(&dir, "rustfmt.toml"), "this is not valid [[[ toml\n")
            .expect("rewrite rustfmt");

        let err = audit_workspace_policy(dir.path(), Vec::new(), Vec::new())
            .expect_err("audit should fail");
        assert!(
            err.to_string().contains("failed to parse") || err.to_string().contains("TOML"),
            "unexpected error: {err}"
        );
    }

    #[test]
    fn workspace_policy_audit_passes_frontier_and_synthesis() {
        let dir = tempfile::tempdir().expect("tempdir");
        write_valid_workspace_policy(dir.path());
        let frontier = vec![
            ChangeId::from_hex("0123456789abcdef0123456789abcdef0123456789abcdef0123456789abcdef")
                .expect("valid change id"),
        ];
        let snapshots = vec![
            SnapshotId::from_hex(
                "abcdef0123456789abcdef0123456789abcdef0123456789abcdef0123456789",
            )
            .expect("valid snapshot id"),
        ];

        let report = audit_workspace_policy(dir.path(), frontier.clone(), snapshots.clone())
            .expect("audit should pass");
        assert_eq!(report.frontier, frontier);
        assert_eq!(report.synthesis_snapshots, snapshots);
    }

    fn rooted(dir: &tempfile::TempDir, file: &str) -> std::path::PathBuf {
        dir.path().join(file)
    }
}

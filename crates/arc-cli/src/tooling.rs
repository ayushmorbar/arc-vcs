use std::collections::BTreeMap;
use std::fs;
use std::path::Path;

use anyhow::Context;
use arc_store_types::newtypes::{ChangeId, SnapshotId};
use serde::Deserialize;
use tracing::instrument;

/// Summary emitted by tooling-audit checks.
#[derive(Debug, Clone)]
pub struct ToolingAuditReport {
    /// Number of accepted `from->to` spell normalization rules.
    pub codespell_rules: usize,
    /// Required mise tasks that were found.
    pub present_required_tasks: Vec<String>,
    /// Slow timeout period for the default nextest profile.
    pub default_slow_timeout_period: String,
    /// Optional termination budget in the CI profile.
    pub ci_terminate_after: Option<u32>,
    /// Frontier of hydrated DAG heads (typed IDs, never raw strings).
    pub frontier: Vec<ChangeId>,
    /// Available synthesis snapshots in `.arc/synthesis`.
    pub synthesis_snapshots: Vec<SnapshotId>,
}

#[derive(Debug, Deserialize)]
struct NextestFile {
    #[serde(default)]
    profile: BTreeMap<String, NextestProfile>,
}

#[derive(Debug, Deserialize)]
struct NextestProfile {
    #[serde(default, rename = "slow-timeout")]
    slow_timeout: Option<SlowTimeout>,
}

#[derive(Debug, Deserialize)]
struct SlowTimeout {
    period: String,
    #[serde(default, rename = "terminate-after")]
    terminate_after: Option<u32>,
}

/// Audit root `.config` developer-tooling policy files.
///
/// This is a read-only verification path: no mutation, no side effects.
#[instrument(skip(frontier, synthesis_snapshots))]
pub fn audit_workspace_tooling(
    repo_root: &Path,
    frontier: Vec<ChangeId>,
    synthesis_snapshots: Vec<SnapshotId>,
) -> anyhow::Result<ToolingAuditReport> {
    let config_dir = repo_root.join(".config");
    let nextest = read_nextest(&config_dir.join("nextest.toml"))?;
    let codespell_rules = read_codespell_rules(&config_dir.join("codespell-additional-dict"))?;
    let present_required_tasks = read_mise_required_tasks(&config_dir.join("mise.toml"))?;

    Ok(ToolingAuditReport {
        codespell_rules,
        present_required_tasks,
        default_slow_timeout_period: nextest.default_period,
        ci_terminate_after: nextest.ci_terminate_after,
        frontier,
        synthesis_snapshots,
    })
}

struct NextestSummary {
    default_period: String,
    ci_terminate_after: Option<u32>,
}

#[instrument]
fn read_nextest(path: &Path) -> anyhow::Result<NextestSummary> {
    let raw = fs::read_to_string(path)
        .with_context(|| format!("failed to read nextest config at {}", path.display()))?;
    let parsed: NextestFile = toml::from_str(&raw)
        .with_context(|| format!("failed to parse nextest config at {}", path.display()))?;

    let default_profile = parsed
        .profile
        .get("default")
        .and_then(|profile| profile.slow_timeout.as_ref())
        .ok_or_else(|| anyhow::anyhow!("nextest profile.default.slow-timeout is required"))?;

    let ci_terminate_after = parsed
        .profile
        .get("ci")
        .and_then(|profile| profile.slow_timeout.as_ref())
        .and_then(|timeout| timeout.terminate_after);

    Ok(NextestSummary {
        default_period: default_profile.period.clone(),
        ci_terminate_after,
    })
}

#[instrument]
fn read_codespell_rules(path: &Path) -> anyhow::Result<usize> {
    let raw = fs::read_to_string(path)
        .with_context(|| format!("failed to read codespell dictionary at {}", path.display()))?;
    let mut rules = 0usize;

    for (idx, line) in raw.lines().enumerate() {
        let trimmed = line.trim();
        if trimmed.is_empty() || trimmed.starts_with('#') {
            continue;
        }
        let (from, to) = trimmed.split_once("->").ok_or_else(|| {
            anyhow::anyhow!(
                "invalid codespell rule at line {} in {}: expected from->to",
                idx + 1,
                path.display()
            )
        })?;
        if from.trim().is_empty() || to.trim().is_empty() {
            anyhow::bail!(
                "invalid codespell rule at line {} in {}: empty side",
                idx + 1,
                path.display()
            );
        }
        rules += 1;
    }

    Ok(rules)
}

#[instrument]
fn read_mise_required_tasks(path: &Path) -> anyhow::Result<Vec<String>> {
    let raw = fs::read_to_string(path)
        .with_context(|| format!("failed to read mise config at {}", path.display()))?;
    let parsed: toml::Value = toml::from_str(&raw)
        .with_context(|| format!("failed to parse mise config at {}", path.display()))?;

    let tasks = parsed
        .get("tasks")
        .and_then(toml::Value::as_table)
        .ok_or_else(|| anyhow::anyhow!("mise.toml must define [tasks]") )?;

    let required = ["check:test", "check:clippy", "check:format"];
    let mut present = Vec::new();

    for task in required {
        if tasks.contains_key(task) {
            present.push(task.to_string());
        } else {
            anyhow::bail!("mise.toml missing required task: {task}");
        }
    }

    Ok(present)
}

#[cfg(test)]
mod tests {
    use std::fs;

    use arc_store_types::newtypes::{ChangeId, SnapshotId};

    use super::audit_workspace_tooling;

    #[test]
    fn audit_workspace_tooling_accepts_valid_config() {
        let dir = tempfile::tempdir().expect("tempdir");
        let config_dir = dir.path().join(".config");
        fs::create_dir_all(&config_dir).expect("create config dir");

        fs::write(
            config_dir.join("nextest.toml"),
            "[profile.default]\nslow-timeout = { period = \"10s\" }\n\n[profile.ci]\nslow-timeout = { period = \"10s\", terminate-after = 4 }\n",
        )
        .expect("write nextest");
        fs::write(config_dir.join("codespell-additional-dict"), "co-locate->colocate\n")
            .expect("write dict");
        fs::write(
            config_dir.join("mise.toml"),
            "[tasks.\"check:test\"]\nrun = \"cargo test --workspace\"\n\n[tasks.\"check:clippy\"]\nrun = \"cargo clippy --workspace --all-targets -- -D warnings\"\n\n[tasks.\"check:format\"]\nrun = \"cargo fmt --all -- --check\"\n",
        )
        .expect("write mise");

        let frontier = vec![
            ChangeId::from_hex(
                "0123456789abcdef0123456789abcdef0123456789abcdef0123456789abcdef",
            )
            .expect("valid change id"),
        ];
        let snapshots = vec![
            SnapshotId::from_hex(
                "abcdef0123456789abcdef0123456789abcdef0123456789abcdef0123456789",
            )
            .expect("valid snapshot id"),
        ];

        let report = audit_workspace_tooling(dir.path(), frontier.clone(), snapshots.clone())
            .expect("audit should pass");
        assert_eq!(report.codespell_rules, 1);
        assert_eq!(report.default_slow_timeout_period, "10s");
        assert_eq!(report.ci_terminate_after, Some(4));
        assert_eq!(report.frontier, frontier);
        assert_eq!(report.synthesis_snapshots, snapshots);
    }

    #[test]
    fn audit_workspace_tooling_rejects_bad_codespell_line() {
        let dir = tempfile::tempdir().expect("tempdir");
        let config_dir = dir.path().join(".config");
        fs::create_dir_all(&config_dir).expect("create config dir");

        fs::write(
            config_dir.join("nextest.toml"),
            "[profile.default]\nslow-timeout = { period = \"10s\" }\n",
        )
        .expect("write nextest");
        fs::write(config_dir.join("codespell-additional-dict"), "invalid-line\n")
            .expect("write dict");
        fs::write(
            config_dir.join("mise.toml"),
            "[tasks.\"check:test\"]\nrun = \"cargo test --workspace\"\n\n[tasks.\"check:clippy\"]\nrun = \"cargo clippy --workspace --all-targets -- -D warnings\"\n\n[tasks.\"check:format\"]\nrun = \"cargo fmt --all -- --check\"\n",
        )
        .expect("write mise");

        let err = audit_workspace_tooling(dir.path(), Vec::new(), Vec::new()).expect_err("audit should fail");
        assert!(
            err.to_string().contains("invalid codespell rule"),
            "unexpected error: {err}"
        );
    }

    #[test]
    fn tooling_audit_current_workspace() {
        let crate_dir = std::path::Path::new(env!("CARGO_MANIFEST_DIR"));
        let repo_root = crate_dir
            .ancestors()
            .nth(2)
            .expect("arc workspace root should exist");

        let report = audit_workspace_tooling(repo_root, Vec::new(), Vec::new())
            .expect("current workspace tooling policy must be valid");
        assert!(report.codespell_rules > 0);
        assert_eq!(report.present_required_tasks.len(), 3);
    }
}


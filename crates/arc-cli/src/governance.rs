use std::fs;
use std::path::Path;

use anyhow::Context;
use arc_store_types::newtypes::{ChangeId, SnapshotId};
use tracing::instrument;

/// Summary emitted by GitHub governance checks.
#[derive(Debug, Clone)]
pub struct GovernanceAuditReport {
    /// Workflow files that must exist under `.github/workflows/`.
    pub required_workflows: Vec<String>,
    /// Number of GitHub Action `uses:` references verified as pinned.
    pub pinned_action_references: usize,
    /// Dependabot ecosystems detected in `.github/dependabot.yml`.
    pub dependabot_ecosystems: Vec<String>,
    /// Typed frontier evidence attached to this report.
    pub frontier: Vec<ChangeId>,
    /// Typed synthesis snapshot evidence attached to this report.
    pub synthesis_snapshots: Vec<SnapshotId>,
}

const REQUIRED_WORKFLOWS: [&str; 3] = ["ci.yml", "docs.yml", "release.yml"];
const REQUIRED_DEPENDABOT_ECOSYSTEMS: [&str; 2] = ["cargo", "github-actions"];
const ALLOWED_TAGGED_ACTIONS: [&str; 3] = [
    "actions/upload-pages-artifact",
    "actions/deploy-pages",
    "Swatinem/rust-cache",
];

/// Audit repository GitHub governance files in read-only mode.
#[instrument(skip(frontier, synthesis_snapshots))]
pub fn audit_github_governance(
    repo_root: &Path,
    frontier: Vec<ChangeId>,
    synthesis_snapshots: Vec<SnapshotId>,
) -> anyhow::Result<GovernanceAuditReport> {
    let github_root = repo_root.join(".github");
    let workflows_dir = github_root.join("workflows");

    ensure_codeowners_exists(&github_root)?;
    let required_workflows = ensure_required_workflows(&workflows_dir)?;
    let pinned_action_references = ensure_pinned_actions(&workflows_dir)?;
    let dependabot_ecosystems = ensure_dependabot_ecosystems(&github_root)?;

    Ok(GovernanceAuditReport {
        required_workflows,
        pinned_action_references,
        dependabot_ecosystems,
        frontier,
        synthesis_snapshots,
    })
}

#[instrument]
fn ensure_codeowners_exists(github_root: &Path) -> anyhow::Result<()> {
    let codeowners = github_root.join("CODEOWNERS");
    if !codeowners.exists() {
        anyhow::bail!("missing governance file: {}", codeowners.display());
    }
    Ok(())
}

#[instrument]
fn ensure_required_workflows(workflows_dir: &Path) -> anyhow::Result<Vec<String>> {
    if !workflows_dir.exists() {
        anyhow::bail!("missing workflows directory: {}", workflows_dir.display());
    }

    let mut found = Vec::new();
    for required in REQUIRED_WORKFLOWS {
        let path = workflows_dir.join(required);
        if !path.exists() {
            anyhow::bail!("missing required workflow: {}", path.display());
        }
        found.push(required.to_string());
    }
    Ok(found)
}

#[instrument]
fn ensure_pinned_actions(workflows_dir: &Path) -> anyhow::Result<usize> {
    let mut pinned_count = 0usize;

    for entry in fs::read_dir(workflows_dir).with_context(|| {
        format!(
            "failed to read workflows directory {}",
            workflows_dir.display()
        )
    })? {
        let entry = entry.with_context(|| {
            format!(
                "failed to read entry in workflows directory {}",
                workflows_dir.display()
            )
        })?;
        let path = entry.path();
        if !is_yaml_file(&path) {
            continue;
        }

        let raw = fs::read_to_string(&path)
            .with_context(|| format!("failed to read workflow file {}", path.display()))?;

        for (index, line) in raw.lines().enumerate() {
            let Some(action_ref) = parse_uses_line(line) else {
                continue;
            };
            if action_ref.starts_with("./") || action_ref.starts_with("docker://") {
                continue;
            }

            if !is_pinned_action_ref(action_ref) {
                anyhow::bail!(
                    "workflow {} line {} uses disallowed action reference '{}'; expected @<40-hex-sha> (or approved v-tag for pages/cache actions)",
                    path.display(),
                    index + 1,
                    action_ref
                );
            }
            pinned_count += 1;
        }
    }

    Ok(pinned_count)
}

#[instrument]
fn ensure_dependabot_ecosystems(github_root: &Path) -> anyhow::Result<Vec<String>> {
    let dependabot = github_root.join("dependabot.yml");
    let raw = fs::read_to_string(&dependabot).with_context(|| {
        format!(
            "failed to read dependabot config at {}",
            dependabot.display()
        )
    })?;

    let mut ecosystems = Vec::new();
    for line in raw.lines() {
        let trimmed = line.trim();
        if !trimmed.starts_with("- package-ecosystem:")
            && !trimmed.starts_with("package-ecosystem:")
        {
            continue;
        }
        let Some((_, value)) = trimmed.split_once(':') else {
            continue;
        };
        let ecosystem = value.trim().trim_matches('"').trim_matches('\'');
        if !ecosystem.is_empty() {
            ecosystems.push(ecosystem.to_string());
        }
    }

    for required in REQUIRED_DEPENDABOT_ECOSYSTEMS {
        if !ecosystems.iter().any(|value| value == required) {
            anyhow::bail!("dependabot missing required ecosystem: {required}");
        }
    }

    Ok(ecosystems)
}

fn is_yaml_file(path: &Path) -> bool {
    matches!(
        path.extension().and_then(|ext| ext.to_str()),
        Some("yml" | "yaml")
    )
}

fn parse_uses_line(line: &str) -> Option<&str> {
    let trimmed = line.trim();
    if let Some(rest) = trimmed.strip_prefix("- uses:") {
        return Some(rest.trim());
    }
    if let Some(rest) = trimmed.strip_prefix("uses:") {
        return Some(rest.trim());
    }
    None
}

fn is_pinned_action_ref(value: &str) -> bool {
    let Some((action, ref_part)) = value.rsplit_once('@') else {
        return false;
    };
    if ref_part.len() == 40 && ref_part.as_bytes().iter().all(u8::is_ascii_hexdigit) {
        return true;
    }

    if ref_part.starts_with('v') && ALLOWED_TAGGED_ACTIONS.contains(&action) {
        return true;
    }

    false
}

#[cfg(test)]
mod tests {
    use std::fs;

    use arc_store_types::newtypes::{ChangeId, SnapshotId};

    use super::audit_github_governance;

    fn write_valid_governance_tree(root: &std::path::Path) {
        let github = root.join(".github");
        let workflows = github.join("workflows");
        fs::create_dir_all(&workflows).expect("create workflows");

        fs::write(github.join("CODEOWNERS"), "* @arc-vcs/maintainers\n").expect("write codeowners");
        fs::write(
            github.join("dependabot.yml"),
            "version: 2\nupdates:\n- package-ecosystem: cargo\n  directory: '/'\n  schedule:\n    interval: weekly\n- package-ecosystem: github-actions\n  directory: '/'\n  schedule:\n    interval: weekly\n",
        )
        .expect("write dependabot");

        let pinned = "de0fac2e4500dabe0009e67214ff5f5447ce83dd";
        for name in ["ci.yml", "docs.yml", "release.yml"] {
            fs::write(
                workflows.join(name),
                format!(
                    "name: test\njobs:\n  run:\n    runs-on: ubuntu-latest\n    steps:\n      - uses: actions/checkout@{pinned}\n"
                ),
            )
            .expect("write workflow");
        }
    }

    #[test]
    fn governance_audit_accepts_valid_configuration() {
        let dir = tempfile::tempdir().expect("tempdir");
        write_valid_governance_tree(dir.path());

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

        let report = audit_github_governance(dir.path(), frontier.clone(), snapshots.clone())
            .expect("audit should pass");
        assert_eq!(report.required_workflows.len(), 3);
        assert_eq!(report.pinned_action_references, 3);
        assert!(report.dependabot_ecosystems.iter().any(|s| s == "cargo"));
        assert!(
            report
                .dependabot_ecosystems
                .iter()
                .any(|s| s == "github-actions")
        );
        assert_eq!(report.frontier, frontier);
        assert_eq!(report.synthesis_snapshots, snapshots);
    }

    #[test]
    fn governance_audit_rejects_unpinned_action_reference() {
        let dir = tempfile::tempdir().expect("tempdir");
        write_valid_governance_tree(dir.path());

        let docs_workflow = dir
            .path()
            .join(".github")
            .join("workflows")
            .join("docs.yml");
        fs::write(
            docs_workflow,
            "name: docs\njobs:\n  deploy:\n    runs-on: ubuntu-latest\n    steps:\n      - uses: actions/checkout@v4\n",
        )
        .expect("rewrite docs workflow");

        let err = audit_github_governance(dir.path(), Vec::new(), Vec::new())
            .expect_err("audit should fail");
        assert!(
            err.to_string().contains("disallowed action reference"),
            "unexpected error: {err}"
        );
    }

    #[test]
    fn governance_audit_current_workspace() {
        let crate_dir = std::path::Path::new(env!("CARGO_MANIFEST_DIR"));
        let repo_root = crate_dir
            .ancestors()
            .nth(2)
            .expect("arc workspace root should exist");

        let report = audit_github_governance(repo_root, Vec::new(), Vec::new())
            .expect("current workspace governance policy must be valid");
        assert_eq!(report.required_workflows.len(), 3);
        assert!(report.pinned_action_references > 0);
    }
}


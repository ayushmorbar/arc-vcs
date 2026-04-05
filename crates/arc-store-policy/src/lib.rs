use std::borrow::Cow;
use std::collections::{HashMap, HashSet};
use std::fs;
use std::path::{Path, PathBuf};

use arc_policy::{PolicyAtom, PolicyDomain, PolicyLattice, PolicyValue, SourceTrace, TrustLevel};
use ignore::gitignore::{Gitignore, GitignoreBuilder};
use thiserror::Error;

mod text;

#[derive(Debug, Error)]
pub enum PolicyStoreError {
    #[error("failed to read '{path}': {source}")]
    Read {
        path: PathBuf,
        source: std::io::Error,
    },
    #[error("invalid include pattern '{pattern}' in '{path}': {details}")]
    InvalidGlob {
        path: PathBuf,
        pattern: String,
        details: String,
    },
    #[error("include cycle detected: {cycle}")]
    IncludeCycle { cycle: String },
    #[error("included file escapes workspace root: '{path}'")]
    OutsideWorkspace { path: PathBuf },
    #[error("too many include levels while reading '{path}'")]
    MaxDepth { path: PathBuf },
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum PathPolicyDecision {
    Ignored,
    Included,
    Unset,
}

#[derive(Debug, Clone)]
pub struct IgnoreTraceEntry {
    pub pattern: String,
    pub source: SourceTrace<'static>,
    pub outcome: arc_policy::TraceOutcome,
    pub line: usize,
    pub value: PolicyValue<bool>,
}

#[derive(Debug, Clone)]
pub struct IgnorePolicyTrace {
    pub query_path: String,
    pub decision: PathPolicyDecision,
    pub entries: Vec<IgnoreTraceEntry>,
}

#[derive(Debug, Clone, Copy, Default)]
pub struct MatchResult {
    ignore: bool,
}

impl MatchResult {
    pub fn is_ignore(&self) -> bool {
        self.ignore
    }
}

#[derive(Debug, Clone)]
struct IgnoreRule {
    pattern: String,
    matcher: Gitignore,
    line: usize,
}

#[derive(Debug, Clone, Default)]
pub struct ArcIgnoreMatcher {
    lattice: PolicyLattice<'static, bool>,
    rules: HashMap<String, IgnoreRule>,
}

impl ArcIgnoreMatcher {
    pub fn empty() -> Self {
        Self::default()
    }

    pub fn load(workspace_root: &Path) -> Result<Self, PolicyStoreError> {
        let workspace_root = workspace_root
            .canonicalize()
            .unwrap_or_else(|_| workspace_root.to_path_buf());
        let mut lattice: PolicyLattice<'static, bool> = PolicyLattice::new();
        let mut rules: HashMap<String, IgnoreRule> = HashMap::new();

        let root_arcignore = workspace_root.join(".arcignore");
        if !root_arcignore.exists() {
            return Ok(Self { lattice, rules });
        }

        let mut loader = IgnoreLoader {
            workspace_root: &workspace_root,
            max_depth: 32,
            visited: HashSet::new(),
            stack: Vec::new(),
            lattice: &mut lattice,
            rules: &mut rules,
        };
        loader.load_recursive(&root_arcignore, 0)?;

        Ok(Self { lattice, rules })
    }

    pub fn matched_path_or_any_parents(&self, path: &str, is_dir: bool) -> MatchResult {
        let decision = self.explain_path_kind(path, is_dir).decision;
        MatchResult {
            ignore: matches!(decision, PathPolicyDecision::Ignored),
        }
    }

    pub fn explain_path(&self, path: &str) -> IgnorePolicyTrace {
        self.explain_path_kind(path, false)
    }

    fn explain_path_kind(&self, path: &str, is_dir: bool) -> IgnorePolicyTrace {
        let normalized = text::normalize_slashes(path).into_owned();
        let trace = self.lattice.resolve_with(&normalized, |atom, query| {
            self.rules.get(atom.key.as_ref()).is_some_and(|rule| {
                rule.matcher
                    .matched_path_or_any_parents(query, is_dir)
                    .is_ignore()
            })
        });

        let decision = match trace.winner.as_ref().map(|w| &w.value) {
            Some(PolicyValue::Present(true)) => PathPolicyDecision::Ignored,
            Some(PolicyValue::Present(false)) => PathPolicyDecision::Included,
            Some(PolicyValue::Cleared) => PathPolicyDecision::Unset,
            _ => PathPolicyDecision::Unset,
        };

        let mut entries = Vec::with_capacity(trace.evaluated.len());
        for entry in trace.evaluated {
            let (line, pattern) = self
                .rules
                .get(entry.atom.key.as_ref())
                .map(|rule| (rule.line, rule.pattern.clone()))
                .unwrap_or((0, entry.atom.key.to_string()));
            entries.push(IgnoreTraceEntry {
                pattern,
                source: entry.atom.source,
                outcome: entry.outcome,
                line,
                value: entry.atom.value,
            });
        }

        IgnorePolicyTrace {
            query_path: normalized,
            decision,
            entries,
        }
    }
}

#[derive(Debug, Clone)]
pub struct ConfigTraceEntry {
    pub key: String,
    pub value: PolicyValue<String>,
    pub source: SourceTrace<'static>,
    pub outcome: arc_policy::TraceOutcome,
}

#[derive(Debug, Clone)]
pub struct ConfigPolicyTrace {
    pub key: String,
    pub winner: Option<PolicyValue<String>>,
    pub entries: Vec<ConfigTraceEntry>,
}

pub fn explain_config_key(
    workspace_root: &Path,
    key: &str,
) -> Result<ConfigPolicyTrace, PolicyStoreError> {
    let workspace_root = workspace_root
        .canonicalize()
        .unwrap_or_else(|_| workspace_root.to_path_buf());
    let lattice = load_config_lattice(&workspace_root)?;
    let trace = lattice.resolve(key);
    let winner = trace.winner.map(|w| match w.value {
        PolicyValue::Present(v) => PolicyValue::Present(v.into_owned()),
        PolicyValue::Cleared => PolicyValue::Cleared,
        PolicyValue::Unset => PolicyValue::Unset,
    });

    let entries = trace
        .evaluated
        .into_iter()
        .map(|entry| ConfigTraceEntry {
            key: entry.atom.key.into_owned(),
            value: match entry.atom.value {
                PolicyValue::Present(v) => PolicyValue::Present(v.into_owned()),
                PolicyValue::Cleared => PolicyValue::Cleared,
                PolicyValue::Unset => PolicyValue::Unset,
            },
            source: entry.atom.source,
            outcome: entry.outcome,
        })
        .collect();

    Ok(ConfigPolicyTrace {
        key: key.to_string(),
        winner,
        entries,
    })
}

fn load_config_lattice(
    workspace_root: &Path,
) -> Result<PolicyLattice<'static, Cow<'static, str>>, PolicyStoreError> {
    let mut lattice = PolicyLattice::new();
    let mut loader = ConfigLoader {
        workspace_root,
        max_depth: 32,
        visited: HashSet::new(),
        stack: Vec::new(),
        lattice: &mut lattice,
    };

    for (path, trust) in config_candidates(workspace_root) {
        if path.exists() {
            loader.load_recursive(&path, trust, 0)?;
        }
    }

    Ok(lattice)
}

struct ConfigLoader<'a> {
    workspace_root: &'a Path,
    max_depth: u16,
    visited: HashSet<PathBuf>,
    stack: Vec<PathBuf>,
    lattice: &'a mut PolicyLattice<'static, Cow<'static, str>>,
}

impl<'a> ConfigLoader<'a> {
    fn load_recursive(
        &mut self,
        file_path: &Path,
        trust: TrustLevel,
        depth: u16,
    ) -> Result<(), PolicyStoreError> {
        let canonical = file_path
            .canonicalize()
            .map_err(|source| PolicyStoreError::Read {
                path: file_path.to_path_buf(),
                source,
            })?;

        if depth > self.max_depth {
            return Err(PolicyStoreError::MaxDepth {
                path: canonical.clone(),
            });
        }

        if let Some(idx) = self.stack.iter().position(|p| p == &canonical) {
            let mut cycle: Vec<String> = self.stack[idx..]
                .iter()
                .map(|p| p.display().to_string())
                .collect();
            cycle.push(canonical.display().to_string());
            return Err(PolicyStoreError::IncludeCycle {
                cycle: cycle.join(" -> "),
            });
        }

        if !self.visited.insert(canonical.clone()) {
            return Ok(());
        }

        self.stack.push(canonical.clone());

        let bytes = fs::read(&canonical).map_err(|source| PolicyStoreError::Read {
            path: canonical.clone(),
            source,
        })?;
        for include in parse_config_includes(&bytes, &canonical, self.workspace_root) {
            let include = include?;
            if !include.path.starts_with(self.workspace_root) {
                return Err(PolicyStoreError::OutsideWorkspace { path: include.path });
            }
            self.load_recursive(&include.path, trust, depth + 1)?;
        }

        let parsed: toml::Value =
            toml::from_slice(&bytes).unwrap_or(toml::Value::Table(Default::default()));
        flatten_config_value(&parsed, "", &canonical, depth, trust, self.lattice);

        self.stack.pop();
        Ok(())
    }
}

fn config_candidates(workspace_root: &Path) -> Vec<(PathBuf, TrustLevel)> {
    let mut out = Vec::new();

    if let Some(dirs) = directories::ProjectDirs::from("", "arc-vcs", "arc") {
        out.push((dirs.config_dir().join("config.toml"), TrustLevel::User));
    }

    out.push((
        workspace_root.join(".arc").join("config.toml"),
        TrustLevel::Repo,
    ));

    out
}

fn flatten_config_value(
    value: &toml::Value,
    prefix: &str,
    source_path: &Path,
    depth: u16,
    trust: TrustLevel,
    lattice: &mut PolicyLattice<'static, Cow<'static, str>>,
) {
    match value {
        toml::Value::Table(table) => {
            for (k, v) in table {
                let next = if prefix.is_empty() {
                    k.to_string()
                } else {
                    format!("{prefix}.{k}")
                };
                flatten_config_value(v, &next, source_path, depth, trust, lattice);
            }
        }
        toml::Value::Array(arr) => {
            let as_text = arr
                .iter()
                .map(toml_scalar_to_string)
                .collect::<Vec<_>>()
                .join(",");
            lattice.push(PolicyAtom {
                domain: PolicyDomain::Config,
                key: Cow::Owned(prefix.to_string()),
                value: PolicyValue::Present(Cow::Owned(as_text)),
                source: SourceTrace {
                    origin: Cow::Owned(source_path.display().to_string()),
                    depth,
                    trust,
                },
            });
        }
        _ => {
            lattice.push(PolicyAtom {
                domain: PolicyDomain::Config,
                key: Cow::Owned(prefix.to_string()),
                value: PolicyValue::Present(Cow::Owned(toml_scalar_to_string(value))),
                source: SourceTrace {
                    origin: Cow::Owned(source_path.display().to_string()),
                    depth,
                    trust,
                },
            });
        }
    }
}

fn toml_scalar_to_string(value: &toml::Value) -> String {
    match value {
        toml::Value::String(s) => s.clone(),
        toml::Value::Integer(i) => i.to_string(),
        toml::Value::Float(f) => f.to_string(),
        toml::Value::Boolean(b) => b.to_string(),
        toml::Value::Datetime(dt) => dt.to_string(),
        toml::Value::Array(_) | toml::Value::Table(_) => value.to_string(),
    }
}

#[derive(Debug, Clone)]
struct IncludeDirective {
    path: PathBuf,
}

fn parse_config_includes(
    bytes: &[u8],
    source_path: &Path,
    workspace_root: &Path,
) -> Vec<Result<IncludeDirective, PolicyStoreError>> {
    let mut out = Vec::new();
    let mut buffers = text::Buffers::default();
    for raw_line in bytes.split(|b| *b == b'\n') {
        let mut with_src = buffers.use_foreign_src(raw_line);
        let (src, dest) = with_src.src_and_dest();
        dest.extend(src.iter().copied().filter(|b| *b != b'\r'));
        with_src.swap();
        let (src, _dest) = with_src.src_and_dest();
        let line_cow = String::from_utf8_lossy(src);
        let line = line_cow.trim();
        if line.is_empty() || line.starts_with('#') {
            continue;
        }

        if let Some(rest) = line.strip_prefix("include =") {
            let include = rest.trim().trim_matches('"');
            out.push(resolve_include(source_path, workspace_root, include));
            continue;
        }

        if let Some(rest) = line.strip_prefix("includeIf.gitdir:") {
            let mut parts = rest.splitn(2, '=');
            let prefix = parts.next().unwrap_or("").trim();
            let include = parts.next().unwrap_or("").trim().trim_matches('"');
            let root_lossy = workspace_root.to_string_lossy();
            let root_norm = text::normalize_slashes(root_lossy.as_ref());
            if !prefix.is_empty() && root_norm.starts_with(prefix) {
                out.push(resolve_include(source_path, workspace_root, include));
            }
            continue;
        }

        if let Some(rest) = line.strip_prefix("includeIf.exists =") {
            let include = rest.trim().trim_matches('"');
            let candidate = source_path.parent().unwrap_or(workspace_root).join(include);
            if candidate.exists() {
                out.push(resolve_include(source_path, workspace_root, include));
            }
        }
    }
    out
}

fn resolve_include(
    source_path: &Path,
    workspace_root: &Path,
    include: &str,
) -> Result<IncludeDirective, PolicyStoreError> {
    let include_path = source_path.parent().unwrap_or(workspace_root).join(include);
    let canonical = include_path
        .canonicalize()
        .map_err(|source| PolicyStoreError::Read {
            path: include_path.clone(),
            source,
        })?;
    if !canonical.starts_with(workspace_root) {
        return Err(PolicyStoreError::OutsideWorkspace { path: canonical });
    }
    Ok(IncludeDirective { path: canonical })
}

struct IgnoreLoader<'a> {
    workspace_root: &'a Path,
    max_depth: u16,
    visited: HashSet<PathBuf>,
    stack: Vec<PathBuf>,
    lattice: &'a mut PolicyLattice<'static, bool>,
    rules: &'a mut HashMap<String, IgnoreRule>,
}

impl<'a> IgnoreLoader<'a> {
    fn load_recursive(&mut self, file_path: &Path, depth: u16) -> Result<(), PolicyStoreError> {
        if depth > self.max_depth {
            return Err(PolicyStoreError::MaxDepth {
                path: file_path.to_path_buf(),
            });
        }

        let canonical = file_path
            .canonicalize()
            .map_err(|source| PolicyStoreError::Read {
                path: file_path.to_path_buf(),
                source,
            })?;

        if let Some(idx) = self.stack.iter().position(|p| p == &canonical) {
            let mut cycle: Vec<String> = self.stack[idx..]
                .iter()
                .map(|p| p.display().to_string())
                .collect();
            cycle.push(canonical.display().to_string());
            return Err(PolicyStoreError::IncludeCycle {
                cycle: cycle.join(" -> "),
            });
        }

        if !self.visited.insert(canonical.clone()) {
            return Ok(());
        }

        self.stack.push(canonical.clone());

        let bytes = fs::read(&canonical).map_err(|source| PolicyStoreError::Read {
            path: canonical.clone(),
            source,
        })?;

        let mut buffers = text::Buffers::default();

        for (line_idx, raw_line) in bytes.split(|b| *b == b'\n').enumerate() {
            let mut with_src = buffers.use_foreign_src(raw_line);
            let (src, dest) = with_src.src_and_dest();
            dest.extend(src.iter().copied().filter(|b| *b != b'\r'));
            with_src.swap();
            let (src, _dest) = with_src.src_and_dest();
            let line_cow = String::from_utf8_lossy(src);
            let line = line_cow.trim();

            if line.is_empty() {
                continue;
            }

            if let Some(rest) = line.strip_prefix("#include ") {
                let include_file =
                    resolve_ignore_include(&canonical, self.workspace_root, rest.trim())?;
                self.load_recursive(&include_file, depth + 1)?;
                continue;
            }

            if let Some(rest) = line.strip_prefix("#includeIf.exists ") {
                let candidate = canonical
                    .parent()
                    .unwrap_or(self.workspace_root)
                    .join(rest.trim());
                if candidate.exists() {
                    let include_file =
                        resolve_ignore_include(&canonical, self.workspace_root, rest.trim())?;
                    self.load_recursive(&include_file, depth + 1)?;
                }
                continue;
            }

            if line.starts_with('#') {
                continue;
            }

            let (value, pattern) = if let Some(stripped) = line.strip_prefix('!') {
                (PolicyValue::Present(false), stripped)
            } else {
                (PolicyValue::Present(true), line)
            };
            let pattern = pattern.trim();
            if pattern.is_empty() {
                continue;
            }

            let mut builder = GitignoreBuilder::new(self.workspace_root);
            builder
                .add_line(Some(canonical.clone()), pattern)
                .map_err(|source| PolicyStoreError::InvalidGlob {
                    path: canonical.clone(),
                    pattern: pattern.to_string(),
                    details: source.to_string(),
                })?;
            let matcher = builder.build().unwrap_or_else(|_| Gitignore::empty());

            let key = format!("{}:{}:{}", canonical.display(), line_idx + 1, pattern);
            self.rules.insert(
                key.clone(),
                IgnoreRule {
                    pattern: pattern.to_string(),
                    matcher,
                    line: line_idx + 1,
                },
            );
            self.lattice.push(PolicyAtom {
                domain: PolicyDomain::Ignore,
                key: Cow::Owned(key),
                value,
                source: SourceTrace {
                    origin: Cow::Owned(canonical.display().to_string()),
                    depth,
                    trust: TrustLevel::Repo,
                },
            });
        }

        self.stack.pop();
        Ok(())
    }
}

fn resolve_ignore_include(
    source_path: &Path,
    workspace_root: &Path,
    include: &str,
) -> Result<PathBuf, PolicyStoreError> {
    let include_path = source_path.parent().unwrap_or(workspace_root).join(include);
    let canonical = include_path
        .canonicalize()
        .map_err(|source| PolicyStoreError::Read {
            path: include_path.clone(),
            source,
        })?;
    if !canonical.starts_with(workspace_root) {
        return Err(PolicyStoreError::OutsideWorkspace { path: canonical });
    }
    Ok(canonical)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn detects_arcignore_include_cycle() {
        let tmp = tempfile::tempdir().expect("tempdir");
        let root = tmp.path();
        fs::write(root.join(".arcignore"), "#include a.ignore\n").expect("root ignore");
        fs::write(root.join("a.ignore"), "#include b.ignore\n").expect("a ignore");
        fs::write(root.join("b.ignore"), "#include a.ignore\n").expect("b ignore");

        let err = ArcIgnoreMatcher::load(root).expect_err("must fail with cycle");
        assert!(matches!(err, PolicyStoreError::IncludeCycle { .. }));
    }

    #[test]
    fn explain_path_returns_winner_and_overridden() {
        let tmp = tempfile::tempdir().expect("tempdir");
        let root = tmp.path();
        fs::write(root.join(".arcignore"), "*.rs\n!src/main.rs\n").expect("ignore");

        let matcher = ArcIgnoreMatcher::load(root).expect("matcher");
        let trace = matcher.explain_path("src/main.rs");
        assert!(matches!(trace.decision, PathPolicyDecision::Included));
        assert!(
            trace
                .entries
                .iter()
                .any(|e| e.outcome == arc_policy::TraceOutcome::Winning)
        );
        assert!(
            trace
                .entries
                .iter()
                .any(|e| e.outcome == arc_policy::TraceOutcome::Overridden)
        );
    }
}

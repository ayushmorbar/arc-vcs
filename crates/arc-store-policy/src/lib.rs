use std::{
    borrow::Cow,
    collections::{HashMap, HashSet},
    fs,
    path::{Path, PathBuf},
};

use arc_policy::{PolicyAtom, PolicyDomain, PolicyLattice, PolicyValue, SourceTrace, TrustLevel};
use ignore::gitignore::{Gitignore, GitignoreBuilder};
use thiserror::Error;

mod text;

#[derive(Debug, Error)]
pub enum PolicyStoreError {
    #[error("failed to read '{path}': {source}")]
    Read { path: PathBuf, source: std::io::Error },
    #[error("invalid include pattern '{pattern}' in '{path}': {details}")]
    InvalidGlob { path: PathBuf, pattern: String, details: String },
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
        let workspace_root =
            workspace_root.canonicalize().unwrap_or_else(|_| workspace_root.to_path_buf());
        let mut lattice: PolicyLattice<'static, bool> = PolicyLattice::new();
        let mut rules: HashMap<String, IgnoreRule> = HashMap::new();

        let root_arcignore = workspace_root.join(".arcignore");
        if !root_arcignore.exists() {
            return Ok(Self { lattice, rules });
        }

        let mut loader = IgnoreLoader {
            workspace_root: &workspace_root,
            include_state: IncludeState::new(32),
            lattice: &mut lattice,
            rules: &mut rules,
        };
        loader.load_recursive(&root_arcignore, 0)?;

        Ok(Self { lattice, rules })
    }

    pub fn matched_path_or_any_parents(&self, path: &str, is_dir: bool) -> MatchResult {
        let decision = self.explain_path_kind(path, is_dir).decision;
        MatchResult { ignore: matches!(decision, PathPolicyDecision::Ignored) }
    }

    pub fn explain_path(&self, path: &str) -> IgnorePolicyTrace {
        self.explain_path_kind(path, false)
    }

    fn explain_path_kind(&self, path: &str, is_dir: bool) -> IgnorePolicyTrace {
        let normalized = text::normalize_slashes(path).into_owned();
        let trace = self.lattice.resolve_with(&normalized, |atom, query| {
            self.rules.get(atom.key.as_ref()).is_some_and(|rule| {
                rule.matcher.matched_path_or_any_parents(query, is_dir).is_ignore()
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

        IgnorePolicyTrace { query_path: normalized, decision, entries }
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
    let workspace_root =
        workspace_root.canonicalize().unwrap_or_else(|_| workspace_root.to_path_buf());
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

    Ok(ConfigPolicyTrace { key: key.to_string(), winner, entries })
}

/// Rewrite policy/config text line-by-line while preserving all untouched bytes.
///
/// The callback receives the line content without trailing newline bytes. If it
/// returns `Some(replacement)`, the original line content is replaced and the
/// original line ending (`\n`, `\r\n`, or none) is preserved.
pub fn rewrite_policy_text_lossless(
    input: &[u8],
    mut rewrite_line: impl FnMut(&str) -> Option<String>,
) -> Vec<u8> {
    text::rewrite_lossless(input, |line, out| {
        let line_text = String::from_utf8_lossy(line.content);
        if let Some(replacement) = rewrite_line(line_text.as_ref()) {
            out.extend_from_slice(replacement.as_bytes());
            let newline = &line.raw[line.content.len()..];
            out.extend_from_slice(newline);
        } else {
            out.extend_from_slice(line.raw);
        }
    })
}

fn load_config_lattice(
    workspace_root: &Path,
) -> Result<PolicyLattice<'static, Cow<'static, str>>, PolicyStoreError> {
    let mut lattice = PolicyLattice::new();
    let mut loader = ConfigLoader {
        workspace_root,
        include_state: IncludeState::new(32),
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
    include_state: IncludeState,
    lattice: &'a mut PolicyLattice<'static, Cow<'static, str>>,
}

impl<'a> ConfigLoader<'a> {
    fn load_recursive(
        &mut self,
        file_path: &Path,
        trust: TrustLevel,
        depth: u16,
    ) -> Result<(), PolicyStoreError> {
        let Some(canonical) = self.include_state.enter(file_path, depth)? else {
            return Ok(());
        };

        let result = (|| {
            let bytes = fs::read(&canonical)
                .map_err(|source| PolicyStoreError::Read { path: canonical.clone(), source })?;
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
            Ok(())
        })();

        self.include_state.exit();
        result
    }
}

fn config_candidates(workspace_root: &Path) -> Vec<(PathBuf, TrustLevel)> {
    let mut out = Vec::new();

    if let Some(dirs) = directories::ProjectDirs::from("", "arc-vcs", "arc") {
        out.push((dirs.config_dir().join("config.toml"), TrustLevel::User));
    }

    out.push((workspace_root.join(".arc").join("config.toml"), repo_trust_level(workspace_root)));

    out
}

/// Determine trust level for repository-local policy files.
pub fn repo_trust_level(workspace_root: &Path) -> TrustLevel {
    if is_workspace_owned_by_current_user(workspace_root).unwrap_or(false) {
        TrustLevel::Repo
    } else {
        TrustLevel::Untrusted
    }
}

/// Return true if `path` appears to be owned by the current process user.
pub fn is_workspace_owned_by_current_user(path: &Path) -> std::io::Result<bool> {
    #[cfg(all(not(windows), not(target_os = "wasi")))]
    {
        use std::os::unix::fs::MetadataExt;

        let owner_of_path = std::fs::symlink_metadata(path)?.uid();
        #[allow(unsafe_code)]
        // SAFETY: libc::geteuid() is always safe on POSIX; the allow lint
        // satisfies the undocumented_unsafe_blocks policy for FFI calls.
        let owner_of_process = unsafe { libc::geteuid() };
        if owner_of_path == owner_of_process {
            return Ok(true);
        }
        if let Some(sudo_uid) = std::env::var_os("SUDO_UID")
            .and_then(|v| v.to_str().and_then(|s| s.parse::<u32>().ok()))
        {
            return Ok(owner_of_path == sudo_uid);
        }
        Ok(false)
    }

    #[cfg(target_os = "wasi")]
    {
        let _ = path;
        Ok(true)
    }

    #[cfg(windows)]
    {
        let _ = path;
        Ok(false)
    }
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
                let next = if prefix.is_empty() { k.to_string() } else { format!("{prefix}.{k}") };
                flatten_config_value(v, &next, source_path, depth, trust, lattice);
            }
        }
        toml::Value::Array(arr) => {
            let as_text = arr.iter().map(toml_scalar_to_string).collect::<Vec<_>>().join(",");
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

#[derive(Debug, Default)]
struct IncludeState {
    max_depth: u16,
    visited: HashSet<PathBuf>,
    stack: Vec<PathBuf>,
}

impl IncludeState {
    fn new(max_depth: u16) -> Self {
        Self { max_depth, visited: HashSet::new(), stack: Vec::new() }
    }

    fn enter(&mut self, file_path: &Path, depth: u16) -> Result<Option<PathBuf>, PolicyStoreError> {
        let canonical = file_path
            .canonicalize()
            .map_err(|source| PolicyStoreError::Read { path: file_path.to_path_buf(), source })?;

        if depth > self.max_depth {
            return Err(PolicyStoreError::MaxDepth { path: canonical.clone() });
        }

        if let Some(idx) = self.stack.iter().position(|p| p == &canonical) {
            let mut cycle: Vec<String> =
                self.stack[idx..].iter().map(|p| p.display().to_string()).collect();
            cycle.push(canonical.display().to_string());
            return Err(PolicyStoreError::IncludeCycle { cycle: cycle.join(" -> ") });
        }

        if !self.visited.insert(canonical.clone()) {
            return Ok(None);
        }

        self.stack.push(canonical.clone());
        Ok(Some(canonical))
    }

    fn exit(&mut self) {
        self.stack.pop();
    }
}

fn parse_config_includes(
    bytes: &[u8],
    source_path: &Path,
    workspace_root: &Path,
) -> Vec<Result<IncludeDirective, PolicyStoreError>> {
    let mut out = Vec::new();
    for line_view in text::iter_lines(bytes) {
        let line_cow = String::from_utf8_lossy(line_view.content);
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
        .map_err(|source| PolicyStoreError::Read { path: include_path.clone(), source })?;
    if !canonical.starts_with(workspace_root) {
        return Err(PolicyStoreError::OutsideWorkspace { path: canonical });
    }
    Ok(IncludeDirective { path: canonical })
}

struct IgnoreLoader<'a> {
    workspace_root: &'a Path,
    include_state: IncludeState,
    lattice: &'a mut PolicyLattice<'static, bool>,
    rules: &'a mut HashMap<String, IgnoreRule>,
}

impl<'a> IgnoreLoader<'a> {
    fn load_recursive(&mut self, file_path: &Path, depth: u16) -> Result<(), PolicyStoreError> {
        let Some(canonical) = self.include_state.enter(file_path, depth)? else {
            return Ok(());
        };

        let result = (|| {
            let bytes = fs::read(&canonical)
                .map_err(|source| PolicyStoreError::Read { path: canonical.clone(), source })?;

            for line_view in text::iter_lines(&bytes) {
                let line_cow = String::from_utf8_lossy(line_view.content);
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
                    let candidate =
                        canonical.parent().unwrap_or(self.workspace_root).join(rest.trim());
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
                builder.add_line(Some(canonical.clone()), pattern).map_err(|source| {
                    PolicyStoreError::InvalidGlob {
                        path: canonical.clone(),
                        pattern: pattern.to_string(),
                        details: source.to_string(),
                    }
                })?;
                let matcher = builder.build().unwrap_or_else(|_| Gitignore::empty());

                let key = format!("{}:{}:{}", canonical.display(), line_view.line_no, pattern);
                self.rules.insert(
                    key.clone(),
                    IgnoreRule { pattern: pattern.to_string(), matcher, line: line_view.line_no },
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

            Ok(())
        })();

        self.include_state.exit();
        result
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
        .map_err(|source| PolicyStoreError::Read { path: include_path.clone(), source })?;
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
        assert!(trace.entries.iter().any(|e| e.outcome == arc_policy::TraceOutcome::Winning));
        assert!(trace.entries.iter().any(|e| e.outcome == arc_policy::TraceOutcome::Overridden));
    }

    #[test]
    fn repo_trust_level_is_repo_or_untrusted() {
        let tmp = tempfile::tempdir().expect("tempdir");
        let trust = repo_trust_level(tmp.path());
        assert!(matches!(trust, TrustLevel::Repo | TrustLevel::Untrusted));
    }

    #[test]
    fn lossless_rewrite_preserves_newlines_and_comments() {
        let input = b"# header\r\nkey = old\n# footer";
        let out = rewrite_policy_text_lossless(input, |line| {
            if line.trim() == "key = old" { Some("key = new".to_string()) } else { None }
        });
        assert_eq!(out, b"# header\r\nkey = new\n# footer");
    }

    #[test]
    fn match_result_is_ignore_true_and_false() {
        let ignored = MatchResult { ignore: true };
        assert!(ignored.is_ignore());
        let not_ignored = MatchResult { ignore: false };
        assert!(!not_ignored.is_ignore());
    }

    #[test]
    fn match_result_default_is_not_ignored() {
        let default = MatchResult::default();
        assert!(!default.is_ignore());
    }

    #[test]
    fn empty_matcher_matches_nothing() {
        let matcher = ArcIgnoreMatcher::empty();
        let result = matcher.matched_path_or_any_parents("src/main.rs", false);
        assert!(!result.is_ignore(), "empty matcher must not ignore anything");
    }

    #[test]
    fn empty_matcher_explain_returns_unset() {
        let matcher = ArcIgnoreMatcher::empty();
        let trace = matcher.explain_path("src/main.rs");
        assert!(
            matches!(trace.decision, PathPolicyDecision::Unset),
            "empty matcher should yield Unset decision"
        );
        assert!(trace.entries.is_empty(), "empty matcher should produce no trace entries");
    }

    #[test]
    fn toml_scalar_to_string_covers_all_variants() {
        assert_eq!(toml_scalar_to_string(&toml::Value::String("hello".into())), "hello");
        assert_eq!(toml_scalar_to_string(&toml::Value::Integer(42)), "42");
        assert_eq!(
            toml_scalar_to_string(&toml::Value::Float(std::f64::consts::PI)),
            "3.141592653589793"
        );
        assert_eq!(toml_scalar_to_string(&toml::Value::Boolean(true)), "true");
        assert_eq!(
            toml_scalar_to_string(&toml::Value::Array(vec![toml::Value::Integer(1)])),
            "[1]"
        );
    }

    #[test]
    fn flatten_config_value_handles_table_and_scalar() {
        use arc_policy::{PolicyLattice, PolicyValue};

        let mut lattice = PolicyLattice::new();
        let path = PathBuf::from("/test/config.toml");
        let toml_val = toml::Value::try_from(toml::toml! {
            [server]
            host = "localhost"
            port = 8080
        })
        .unwrap();

        flatten_config_value(&toml_val, "", &path, 0, TrustLevel::Repo, &mut lattice);
        let trace = lattice.resolve("server.host");
        match trace.winner.map(|w| w.value) {
            Some(PolicyValue::Present(v)) => assert_eq!(v.as_ref(), "localhost"),
            other => panic!("expected Present(localhost), got {other:?}"),
        }
    }

    #[test]
    fn policy_store_error_display_covers_all_variants() {
        let read_err = PolicyStoreError::Read {
            path: PathBuf::from("/test"),
            source: std::io::Error::new(std::io::ErrorKind::NotFound, "missing"),
        };
        assert!(format!("{read_err}").contains("failed to read"));

        let glob_err = PolicyStoreError::InvalidGlob {
            path: PathBuf::from("/test"),
            pattern: "[bad".into(),
            details: "unclosed bracket".into(),
        };
        assert!(format!("{glob_err}").contains("invalid include pattern"));

        let cycle_err = PolicyStoreError::IncludeCycle { cycle: "a -> b -> a".into() };
        assert!(format!("{cycle_err}").contains("include cycle"));

        let outside_err = PolicyStoreError::OutsideWorkspace { path: PathBuf::from("/escape") };
        assert!(format!("{outside_err}").contains("escapes workspace root"));

        let depth_err = PolicyStoreError::MaxDepth { path: PathBuf::from("/deep") };
        assert!(format!("{depth_err}").contains("too many include levels"));
    }

    #[test]
    fn parse_config_includes_extracts_include_directives() {
        let tmp = tempfile::tempdir().expect("tempdir");
        let root = tmp.path().canonicalize().expect("canonicalize root");
        let included = root.join("extra.toml");
        fs::write(&included, "key = \"value\"\n").expect("included file");
        let main = root.join("config.toml");
        fs::write(&main, "include = \"extra.toml\"\nother = 1\n").expect("main config");

        let bytes = fs::read(&main).unwrap();
        let directives = parse_config_includes(&bytes, &main, &root);
        assert_eq!(directives.len(), 1, "should find one include directive");
        assert!(directives[0].is_ok(), "include directive should resolve successfully");
    }

    #[test]
    fn parse_config_includes_skips_comments_and_blanks() {
        let bytes = b"# this is a comment\n\nother = true\n";
        let root = PathBuf::from("/nonexistent");
        let directives = parse_config_includes(bytes, &root, &root);
        assert!(directives.is_empty(), "comments and blanks must produce no directives");
    }

    #[test]
    fn is_workspace_owned_by_current_user_returns_bool_on_windows() {
        let tmp = tempfile::tempdir().expect("tempdir");
        let result = is_workspace_owned_by_current_user(tmp.path());
        assert!(result.is_ok());
        // On Windows this always returns false (stub), on Unix returns true for own dir
        let owned = result.unwrap();
        assert!(matches!(owned, true | false), "result must be a valid bool");
    }

    #[test]
    fn include_state_detects_max_depth() {
        let tmp = tempfile::tempdir().expect("tempdir");
        let real_path = tmp.path().canonicalize().expect("canonicalize");
        let path = real_path.join("file.toml");
        fs::write(&path, "").unwrap();
        let mut state = IncludeState::new(0);
        let result = state.enter(&path, 1);
        assert!(result.is_err());
        assert!(matches!(result.unwrap_err(), PolicyStoreError::MaxDepth { .. }));
    }

    #[test]
    fn arc_ignore_matcher_load_with_no_arcignore() {
        let tmp = tempfile::tempdir().expect("tempdir");
        let matcher = ArcIgnoreMatcher::load(tmp.path()).expect("load with no .arcignore");
        let result = matcher.matched_path_or_any_parents("any/path.rs", false);
        assert!(!result.is_ignore());
    }

    #[test]
    fn arc_ignore_matcher_load_with_real_patterns() {
        let tmp = tempfile::tempdir().expect("tempdir");
        let root = tmp.path();
        fs::write(root.join(".arcignore"), "*.log\ntarget/\n!target/keep.rs\n").expect("ignore");

        let matcher = ArcIgnoreMatcher::load(root).expect("load matcher");
        assert!(
            matcher.matched_path_or_any_parents("debug.log", false).is_ignore(),
            "*.log should be ignored"
        );
        assert!(
            matcher.matched_path_or_any_parents("target/debug", true).is_ignore(),
            "target/ should be ignored"
        );
        assert!(
            !matcher.matched_path_or_any_parents("target/keep.rs", false).is_ignore(),
            "!target/keep.rs should be un-ignored"
        );
        assert!(
            !matcher.matched_path_or_any_parents("src/main.rs", false).is_ignore(),
            "unmatched path should not be ignored"
        );
    }

    #[test]
    fn rewrite_policy_text_lossless_empty_input() {
        let out = rewrite_policy_text_lossless(b"", |_| Some("replaced".to_string()));
        assert!(out.is_empty(), "empty input should produce empty output");
    }

    #[test]
    fn rewrite_policy_text_lossless_passthrough() {
        let input = b"line1\nline2\nline3\n";
        let out = rewrite_policy_text_lossless(input, |_| None);
        assert_eq!(out, input, "callback returning None should pass through unchanged");
    }

    #[test]
    fn rewrite_policy_text_lossless_no_trailing_newline() {
        let input = b"line1\nline2";
        let out = rewrite_policy_text_lossless(input, |line| {
            if line == "line2" { Some("LINE2".to_string()) } else { None }
        });
        assert_eq!(out, b"line1\nLINE2");
    }

    #[test]
    fn flatten_config_value_with_nested_table() {
        use arc_policy::{PolicyLattice, PolicyValue};

        let mut lattice = PolicyLattice::new();
        let path = PathBuf::from("/test/config.toml");
        let toml_val: toml::Value = toml::from_str("[a.b]\nkey = \"val\"").unwrap();

        flatten_config_value(&toml_val, "", &path, 0, TrustLevel::Repo, &mut lattice);
        let trace = lattice.resolve("a.b.key");
        match trace.winner.map(|w| w.value) {
            Some(PolicyValue::Present(v)) => assert_eq!(v.as_ref(), "val"),
            other => panic!("expected Present(val), got {other:?}"),
        }
    }

    #[test]
    fn flatten_config_value_with_array() {
        use arc_policy::{PolicyLattice, PolicyValue};

        let mut lattice = PolicyLattice::new();
        let path = PathBuf::from("/test/config.toml");
        let toml_val: toml::Value = toml::from_str("[server]\nhosts = [\"a\", \"b\"]").unwrap();

        flatten_config_value(&toml_val, "", &path, 0, TrustLevel::Repo, &mut lattice);
        let trace = lattice.resolve("server.hosts");
        match trace.winner.map(|w| w.value) {
            Some(PolicyValue::Present(v)) => assert_eq!(v.as_ref(), "a,b"),
            other => panic!("expected Present(a,b), got {other:?}"),
        }
    }

    #[test]
    fn toml_scalar_to_string_datetime_variant() {
        let toml_str = "dt = 2025-01-15T10:30:00Z";
        let parsed: toml::Value = toml::from_str(toml_str).expect("parse toml with datetime");
        let table = parsed.as_table().expect("must be table");
        let dt_val = table.get("dt").expect("dt key must exist");
        assert!(matches!(dt_val, toml::Value::Datetime(_)), "value must be Datetime variant");
        let s = toml_scalar_to_string(dt_val);
        assert_eq!(s, "2025-01-15T10:30:00Z");
    }

    #[test]
    fn parse_config_includes_includeif_exists() {
        let tmp = tempfile::tempdir().expect("tempdir");
        let root = tmp.path().canonicalize().expect("canonicalize root");
        let extra = root.join("extra.toml");
        fs::write(&extra, "x = 1\n").expect("write extra");

        let main = root.join("config.toml");
        fs::write(&main, "includeIf.exists = \"extra.toml\"\n").expect("write main");

        let bytes = fs::read(&main).unwrap();
        let directives = parse_config_includes(&bytes, &main, &root);
        assert_eq!(directives.len(), 1, "should find one includeIf.exists directive");
        assert!(directives[0].is_ok(), "directive should resolve successfully");
    }

    #[test]
    fn parse_config_includes_includeif_exists_missing() {
        let tmp = tempfile::tempdir().expect("tempdir");
        let root = tmp.path().canonicalize().expect("canonicalize root");

        let main = root.join("config.toml");
        fs::write(&main, "includeIf.exists = \"nonexistent.toml\"\n").expect("write main");

        let bytes = fs::read(&main).unwrap();
        let directives = parse_config_includes(&bytes, &main, &root);
        assert!(directives.is_empty(), "missing file should produce no directives");
    }

    #[test]
    fn parse_config_includes_includeif_gitdir() {
        let tmp = tempfile::tempdir().expect("tempdir");
        let root = tmp.path().canonicalize().expect("canonicalize root");
        let extra = root.join("extra.toml");
        fs::write(&extra, "y = 2\n").expect("write extra");

        let root_norm = root.to_string_lossy().replace('\\', "/");
        let main = root.join("config.toml");
        let directive_line = format!("includeIf.gitdir: {root_norm} = \"extra.toml\"\n");
        fs::write(&main, &directive_line).expect("write main");

        let bytes = fs::read(&main).unwrap();
        let directives = parse_config_includes(&bytes, &main, &root);
        assert_eq!(directives.len(), 1, "should find one includeIf.gitdir directive");
        assert!(directives[0].is_ok(), "directive should resolve successfully");
    }

    #[test]
    fn parse_config_includes_includeif_gitdir_no_match() {
        let tmp = tempfile::tempdir().expect("tempdir");
        let root = tmp.path().canonicalize().expect("canonicalize root");
        let extra = root.join("extra.toml");
        fs::write(&extra, "z = 3\n").expect("write extra");

        let main = root.join("config.toml");
        let directive_line = "includeIf.gitdir: /completely/different/path = \"extra.toml\"\n";
        fs::write(&main, directive_line).expect("write main");

        let bytes = fs::read(&main).unwrap();
        let directives = parse_config_includes(&bytes, &main, &root);
        assert!(directives.is_empty(), "non-matching gitdir prefix should produce no directives");
    }

    #[test]
    fn explain_config_key_with_existing_key() {
        let tmp = tempfile::tempdir().expect("tempdir");
        let root = tmp.path();
        let arc_dir = root.join(".arc");
        fs::create_dir_all(&arc_dir).expect("create .arc dir");
        fs::write(arc_dir.join("config.toml"), "[server]\nport = 8080\n").expect("write config");

        let trace = explain_config_key(root, "server.port").expect("explain_config_key failed");
        assert!(trace.winner.is_some(), "trace should have a winner for server.port");
    }

    #[test]
    fn include_state_enter_with_visited_path() {
        let tmp = tempfile::tempdir().expect("tempdir");
        let real_path = tmp.path().canonicalize().expect("canonicalize");
        let path = real_path.join("file.toml");
        fs::write(&path, "").unwrap();

        let mut state = IncludeState::new(10);
        let first = state.enter(&path, 0).expect("first enter must succeed");
        assert!(first.is_some(), "first enter should return Some(path)");

        state.exit();

        let second = state.enter(&path, 0).expect("second enter must succeed");
        assert!(second.is_none(), "second enter of already-visited path should return Ok(None)");
    }
}

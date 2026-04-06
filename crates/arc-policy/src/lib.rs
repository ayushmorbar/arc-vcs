use std::borrow::Cow;

pub mod conflict;

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum PolicyValue<T> {
    Present(T),
    Cleared,
    Unset,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub enum TrustLevel {
    Untrusted,
    User,
    Repo,
    Trusted,
}

/// Permission decision used for sensitive operations.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum Permission {
    /// Deny operation and fail closed.
    Forbid,
    /// Ignore operation quietly.
    Deny,
    /// Permit operation.
    Allow,
}

impl Permission {
    /// Return true if this permission allows the operation.
    pub fn is_allowed(self) -> bool {
        matches!(self, Permission::Allow)
    }
}

/// Trait for computing default values from trust level.
pub trait DefaultForTrust {
    /// Build a default value for a trust level.
    fn default_for_trust(level: TrustLevel) -> Self;
}

/// Mapping of values by trust-level classes.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct TrustMapping<T> {
    /// Value for fully trusted sources.
    pub trusted: T,
    /// Value for user-level sources.
    pub user: T,
    /// Value for repo-local sources.
    pub repo: T,
    /// Value for untrusted sources.
    pub untrusted: T,
}

impl<T> TrustMapping<T>
where
    T: Clone,
{
    /// Resolve value by trust level.
    pub fn by_level(&self, level: TrustLevel) -> T {
        match level {
            TrustLevel::Trusted => self.trusted.clone(),
            TrustLevel::User => self.user.clone(),
            TrustLevel::Repo => self.repo.clone(),
            TrustLevel::Untrusted => self.untrusted.clone(),
        }
    }
}

impl<T> Default for TrustMapping<T>
where
    T: DefaultForTrust,
{
    fn default() -> Self {
        Self {
            trusted: T::default_for_trust(TrustLevel::Trusted),
            user: T::default_for_trust(TrustLevel::User),
            repo: T::default_for_trust(TrustLevel::Repo),
            untrusted: T::default_for_trust(TrustLevel::Untrusted),
        }
    }
}

impl DefaultForTrust for Permission {
    fn default_for_trust(level: TrustLevel) -> Self {
        match level {
            TrustLevel::Trusted | TrustLevel::User | TrustLevel::Repo => Permission::Allow,
            TrustLevel::Untrusted => Permission::Forbid,
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum PolicyDomain {
    Ignore,
    Config,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SourceTrace<'a> {
    pub origin: Cow<'a, str>,
    pub depth: u16,
    pub trust: TrustLevel,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PolicyAtom<'a, T> {
    pub domain: PolicyDomain,
    pub key: Cow<'a, str>,
    pub value: PolicyValue<T>,
    pub source: SourceTrace<'a>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum TraceOutcome {
    Winning,
    Overridden,
    SkippedUnset,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PolicyTraceEntry<'a, T> {
    pub atom: PolicyAtom<'a, T>,
    pub outcome: TraceOutcome,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PolicyTrace<'a, T> {
    pub query: String,
    pub winner: Option<PolicyAtom<'a, T>>,
    pub evaluated: Vec<PolicyTraceEntry<'a, T>>,
}

#[derive(Debug, Clone, Default)]
pub struct PolicyLattice<'a, T> {
    atoms: Vec<PolicyAtom<'a, T>>,
}

impl<'a, T: Clone> PolicyLattice<'a, T> {
    pub fn new() -> Self {
        Self { atoms: Vec::new() }
    }

    pub fn push(&mut self, atom: PolicyAtom<'a, T>) {
        self.atoms.push(atom);
    }

    pub fn extend<I>(&mut self, atoms: I)
    where
        I: IntoIterator<Item = PolicyAtom<'a, T>>,
    {
        self.atoms.extend(atoms);
    }

    pub fn atoms(&self) -> &[PolicyAtom<'a, T>] {
        &self.atoms
    }

    pub fn resolve(&self, query: &str) -> PolicyTrace<'a, T> {
        self.resolve_with(query, |atom, q| atom.key.as_ref() == q)
    }

    pub fn resolve_with<F>(&self, query: &str, mut is_match: F) -> PolicyTrace<'a, T>
    where
        F: FnMut(&PolicyAtom<'a, T>, &str) -> bool,
    {
        let mut evaluated: Vec<PolicyTraceEntry<'a, T>> = Vec::new();
        let mut winner: Option<PolicyAtom<'a, T>> = None;

        for atom in self
            .atoms
            .iter()
            .filter(|atom| is_match(atom, query))
            .rev()
            .cloned()
        {
            let outcome = match atom.value {
                PolicyValue::Unset => TraceOutcome::SkippedUnset,
                _ if winner.is_none() => {
                    winner = Some(atom.clone());
                    TraceOutcome::Winning
                }
                _ => TraceOutcome::Overridden,
            };
            evaluated.push(PolicyTraceEntry { atom, outcome });
        }

        PolicyTrace {
            query: query.to_string(),
            winner,
            evaluated,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn mk_atom<'a>(
        key: &'a str,
        value: PolicyValue<&'a str>,
        depth: u16,
    ) -> PolicyAtom<'a, &'a str> {
        PolicyAtom {
            domain: PolicyDomain::Config,
            key: Cow::Borrowed(key),
            value,
            source: SourceTrace {
                origin: Cow::Borrowed("test"),
                depth,
                trust: TrustLevel::Repo,
            },
        }
    }

    #[test]
    fn reverse_scan_last_present_wins() {
        let mut lattice = PolicyLattice::new();
        lattice.push(mk_atom("ui.color", PolicyValue::Present("never"), 0));
        lattice.push(mk_atom("ui.color", PolicyValue::Present("always"), 1));

        let trace = lattice.resolve("ui.color");
        assert!(matches!(
            trace.winner.map(|w| w.value),
            Some(PolicyValue::Present("always"))
        ));
        assert_eq!(trace.evaluated.len(), 2);
        assert_eq!(trace.evaluated[0].outcome, TraceOutcome::Winning);
        assert_eq!(trace.evaluated[1].outcome, TraceOutcome::Overridden);
    }

    #[test]
    fn cleared_beats_previous_present() {
        let mut lattice = PolicyLattice::new();
        lattice.push(mk_atom("ui.color", PolicyValue::Present("auto"), 0));
        lattice.push(mk_atom("ui.color", PolicyValue::Cleared, 1));

        let trace = lattice.resolve("ui.color");
        assert!(matches!(
            trace.winner.map(|w| w.value),
            Some(PolicyValue::Cleared)
        ));
    }

    #[test]
    fn unset_is_skipped_until_real_value() {
        let mut lattice = PolicyLattice::new();
        lattice.push(mk_atom("ui.color", PolicyValue::Present("auto"), 0));
        lattice.push(mk_atom("ui.color", PolicyValue::Unset, 1));

        let trace = lattice.resolve("ui.color");
        assert!(matches!(
            trace.winner.map(|w| w.value),
            Some(PolicyValue::Present("auto"))
        ));
        assert_eq!(trace.evaluated[0].outcome, TraceOutcome::SkippedUnset);
        assert_eq!(trace.evaluated[1].outcome, TraceOutcome::Winning);
    }

    #[test]
    fn trust_mapping_resolves_expected_permission() {
        let mapping = TrustMapping::<Permission>::default();
        assert!(mapping.by_level(TrustLevel::Repo).is_allowed());
        assert!(!mapping.by_level(TrustLevel::Untrusted).is_allowed());
    }
}

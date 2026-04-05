use std::borrow::Cow;

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

    fn mk_atom<'a>(key: &'a str, value: PolicyValue<&'a str>, depth: u16) -> PolicyAtom<'a, &'a str> {
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
        assert!(matches!(trace.winner.map(|w| w.value), Some(PolicyValue::Present("always"))));
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
        assert!(matches!(trace.winner.map(|w| w.value), Some(PolicyValue::Cleared)));
    }

    #[test]
    fn unset_is_skipped_until_real_value() {
        let mut lattice = PolicyLattice::new();
        lattice.push(mk_atom("ui.color", PolicyValue::Present("auto"), 0));
        lattice.push(mk_atom("ui.color", PolicyValue::Unset, 1));

        let trace = lattice.resolve("ui.color");
        assert!(matches!(trace.winner.map(|w| w.value), Some(PolicyValue::Present("auto"))));
        assert_eq!(trace.evaluated[0].outcome, TraceOutcome::SkippedUnset);
        assert_eq!(trace.evaluated[1].outcome, TraceOutcome::Winning);
    }
}

//! AI-powered conflict resolution trait and test mock.

/// Trait for AI-powered conflict resolution.
///
/// Implementations receive the base (LCA) content and the two diverging
/// sides, plus their semantic intents, and produce a merged result.
pub trait AiResolver {
    /// Resolve a three-way conflict, returning the merged content.
    fn resolve(
        &self,
        base: &[u8],
        ours: &[u8],
        theirs: &[u8],
        intent_ours: &str,
        intent_theirs: &str,
    ) -> Result<Vec<u8>, String>;
}

/// A deterministic mock resolver for testing.
///
/// Concatenates both sides separated by a newline — just enough to verify
/// the resolution pipeline without an actual AI model.
pub struct MockResolver;

impl AiResolver for MockResolver {
    fn resolve(
        &self,
        _base: &[u8],
        ours: &[u8],
        theirs: &[u8],
        _intent_ours: &str,
        _intent_theirs: &str,
    ) -> Result<Vec<u8>, String> {
        let mut merged = ours.to_vec();
        merged.push(b'\n');
        merged.extend_from_slice(theirs);
        Ok(merged)
    }
}

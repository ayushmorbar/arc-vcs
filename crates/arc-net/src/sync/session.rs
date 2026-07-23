//! BLUF: `SyncSession` is a 5-state machine driving the arc sync pipeline.
//!
//! States: `Discover → Negotiate → Transfer → Materialize → Finalize`.
//!
//! Each state holds the minimum data required to progress; invalid
//! transitions are rejected at compile time via exhaustive match arms.
//!
//! ## Purity boundary
//!
//! This module owns *control flow* and *data requirements* only.
//! Actual I/O (CAS reads, TCP sends) is injected via trait callbacks
//! so the state machine itself stays unit-testable and deterministic.

use std::fmt;

use arc_algebra_types::Blake3Hash;

use super::protocol::NetError;

// ── State types ───────────────────────────────────────────────────────

/// Discover phase: local and remote frontiers have been collected.
#[derive(Debug, Clone)]
pub struct Discover {
    /// Local heads at session start.
    pub local_frontier: Vec<Blake3Hash>,
    /// Remote heads from the peer's handshake.
    pub remote_frontier: Vec<Blake3Hash>,
}

/// Negotiate phase: missing hashes computed, ready for transfer.
#[derive(Debug, Clone)]
pub struct Negotiate {
    /// Hashes the local store needs from the peer.
    pub missing: Vec<Blake3Hash>,
    /// Hashes the peer needs from the local store.
    pub needed_by_peer: Vec<Blake3Hash>,
}

/// Transfer phase: CAS blocks are being streamed.
#[derive(Debug, Clone)]
pub struct Transfer {
    /// Blocks received so far (hash → bytes).
    pub received: Vec<Vec<u8>>,
    /// Total blocks expected.
    pub total: usize,
}

/// Materialize phase: received blocks are being applied to the local DAG.
#[derive(Debug, Clone)]
pub struct Materialize {
    /// Number of blocks successfully materialized.
    pub applied: usize,
    /// Total blocks to apply.
    pub total: usize,
}

/// Finalize phase: merge complete, frontier updated.
#[derive(Debug, Clone)]
pub struct Finalize {
    /// The merged frontier after sync.
    pub merged_frontier: Vec<Blake3Hash>,
}

// ── SyncSession ───────────────────────────────────────────────────────

/// A state-machine session driving one full sync cycle.
///
/// Use the `From<Current>` → `Next` transitions to advance through
/// the pipeline. Each transition consumes the current state and
/// produces the next, carrying only the data relevant to that phase.
///
/// ```text
/// Discover ──→ Negotiate ──→ Transfer ──→ Materialize ──→ Finalize
/// ```
#[derive(Debug, Clone)]
pub enum SyncSession {
    /// Phase 1: frontiers collected, awaiting delta computation.
    Discover(Discover),
    /// Phase 2: deltas known, awaiting transfer.
    Negotiate(Negotiate),
    /// Phase 3: CAS blocks streaming.
    Transfer(Transfer),
    /// Phase 4: blocks applied, awaiting frontier merge.
    Materialize(Materialize),
    /// Phase 5: session complete.
    Finalize(Finalize),
}

impl SyncSession {
    /// Create a new session from local and remote frontiers.
    pub fn new(local_frontier: Vec<Blake3Hash>, remote_frontier: Vec<Blake3Hash>) -> Self {
        SyncSession::Discover(Discover { local_frontier, remote_frontier })
    }

    /// Return the current phase name.
    pub fn phase(&self) -> &'static str {
        match self {
            SyncSession::Discover(_) => "discover",
            SyncSession::Negotiate(_) => "negotiate",
            SyncSession::Transfer(_) => "transfer",
            SyncSession::Materialize(_) => "materialize",
            SyncSession::Finalize(_) => "finalize",
        }
    }

    /// Return `true` if the session has reached `Finalize`.
    pub fn is_complete(&self) -> bool {
        matches!(self, SyncSession::Finalize(_))
    }
}

// ── Transitions ───────────────────────────────────────────────────────

impl From<Discover> for Negotiate {
    fn from(d: Discover) -> Self {
        Negotiate { missing: d.remote_frontier.clone(), needed_by_peer: d.local_frontier.clone() }
    }
}

impl From<Negotiate> for Transfer {
    fn from(n: Negotiate) -> Self {
        Transfer { received: Vec::new(), total: n.missing.len() }
    }
}

impl From<Transfer> for Materialize {
    fn from(t: Transfer) -> Self {
        Materialize { applied: 0, total: t.received.len() }
    }
}

impl From<Materialize> for Finalize {
    fn from(_m: Materialize) -> Self {
        Finalize { merged_frontier: Vec::new() }
    }
}

// ── Display ───────────────────────────────────────────────────────────

impl fmt::Display for SyncSession {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            SyncSession::Discover(d) => {
                write!(
                    f,
                    "Discover(local={}, remote={})",
                    d.local_frontier.len(),
                    d.remote_frontier.len()
                )
            }
            SyncSession::Negotiate(n) => {
                write!(
                    f,
                    "Negotiate(missing={}, needed={})",
                    n.missing.len(),
                    n.needed_by_peer.len()
                )
            }
            SyncSession::Transfer(t) => {
                write!(f, "Transfer(received={}/{})", t.received.len(), t.total)
            }
            SyncSession::Materialize(m) => {
                write!(f, "Materialize(applied={}/{})", m.applied, m.total)
            }
            SyncSession::Finalize(fi) => {
                write!(f, "Finalize(frontier={})", fi.merged_frontier.len())
            }
        }
    }
}

// ── Errors ────────────────────────────────────────────────────────────

/// Errors specific to session state transitions.
#[derive(Debug, thiserror::Error)]
pub enum SessionError {
    /// Transition attempted from an incompatible state.
    #[error("invalid transition from {from} to {to}")]
    InvalidTransition {
        /// The phase the session was in.
        from: &'static str,
        /// The phase attempted.
        to: &'static str,
    },

    /// Session is already complete.
    #[error("session already finalized")]
    AlreadyComplete,

    /// Session error propagated from the network layer.
    #[error("network error: {0}")]
    Net(#[from] NetError),
}

// ── Tests ─────────────────────────────────────────────────────────────

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn new_session_starts_in_discover() {
        let s = SyncSession::new(vec![[1u8; 32]], vec![[2u8; 32]]);
        assert!(matches!(s, SyncSession::Discover(_)));
        assert_eq!(s.phase(), "discover");
        assert!(!s.is_complete());
    }

    #[test]
    fn discover_to_negotiate() {
        let s = SyncSession::new(vec![[1u8; 32]], vec![[2u8; 32]]);
        if let SyncSession::Discover(d) = s {
            let n: Negotiate = d.into();
            assert_eq!(n.missing.len(), 1);
            assert_eq!(n.needed_by_peer.len(), 1);
            assert_eq!(n.missing[0], [2u8; 32]);
            assert_eq!(n.needed_by_peer[0], [1u8; 32]);
        } else {
            panic!("expected Discover state");
        }
    }

    #[test]
    fn negotiate_to_transfer() {
        let n = Negotiate { missing: vec![[2u8; 32]], needed_by_peer: vec![] };
        let t: Transfer = n.into();
        assert_eq!(t.total, 1);
        assert!(t.received.is_empty());
    }

    #[test]
    fn transfer_to_materialize() {
        let t = Transfer { received: vec![vec![1, 2, 3], vec![4, 5, 6]], total: 2 };
        let m: Materialize = t.into();
        assert_eq!(m.total, 2);
        assert_eq!(m.applied, 0);
    }

    #[test]
    fn materialize_to_finalize() {
        let m = Materialize { applied: 2, total: 2 };
        let f: Finalize = m.into();
        assert!(f.merged_frontier.is_empty());
    }

    #[test]
    fn full_lifecycle() {
        let s = SyncSession::new(vec![[1u8; 32]], vec![[2u8; 32]]);
        let s = if let SyncSession::Discover(d) = s {
            SyncSession::Negotiate(d.into())
        } else {
            unreachable!()
        };
        let s = if let SyncSession::Negotiate(n) = s {
            SyncSession::Transfer(n.into())
        } else {
            unreachable!()
        };
        let s = if let SyncSession::Transfer(t) = s {
            SyncSession::Materialize(t.into())
        } else {
            unreachable!()
        };
        let s = if let SyncSession::Materialize(m) = s {
            SyncSession::Finalize(m.into())
        } else {
            unreachable!()
        };
        assert!(s.is_complete());
        assert_eq!(s.phase(), "finalize");
    }

    #[test]
    fn display_format() {
        let s = SyncSession::new(vec![[1u8; 32], [2u8; 32]], vec![[3u8; 32]]);
        assert!(s.to_string().contains("Discover"));
        assert!(s.to_string().contains("2"));
        assert!(s.to_string().contains("1"));
    }

    #[test]
    fn negotiate_display() {
        let n = Negotiate { missing: vec![[1u8; 32]], needed_by_peer: vec![[2u8; 32], [3u8; 32]] };
        let s = SyncSession::Negotiate(n);
        assert!(s.to_string().contains("Negotiate"));
        assert!(s.to_string().contains("1"));
        assert!(s.to_string().contains("2"));
    }

    #[test]
    fn empty_frontiers() {
        let s = SyncSession::new(vec![], vec![]);
        let n: Negotiate = if let SyncSession::Discover(d) = s { d.into() } else { unreachable!() };
        assert!(n.missing.is_empty());
        assert!(n.needed_by_peer.is_empty());
    }
}

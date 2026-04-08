//! Pure Git-compatibility scalar types used by core surfaces.

/// A 20-byte SHA-1 object identifier used by legacy Git interoperability.
pub type GitOid = [u8; 20];

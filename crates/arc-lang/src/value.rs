//! Borrowed/owned byte-value wrappers for zero-copy parser paths.

use std::borrow::Cow;

/// Owned byte value.
#[derive(Clone, Debug, PartialEq, Eq, Hash, Ord, PartialOrd, Default)]
pub struct ByteValue(Vec<u8>);

/// Borrowed byte value.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, Ord, PartialOrd)]
pub struct ByteValueRef<'a>(&'a [u8]);

impl<'a> ByteValueRef<'a> {
    /// Create a borrowed byte value from raw bytes.
    pub fn from_bytes(bytes: &'a [u8]) -> Self {
        Self(bytes)
    }

    /// Access raw bytes.
    pub fn as_bytes(self) -> &'a [u8] {
        self.0
    }

    /// Convert into owned bytes.
    pub fn to_owned(self) -> ByteValue {
        ByteValue(self.0.to_vec())
    }
}

impl ByteValue {
    /// Access this value as a borrowed view.
    pub fn as_ref(&self) -> ByteValueRef<'_> {
        ByteValueRef(&self.0)
    }

    /// Access raw owned bytes.
    pub fn as_bytes(&self) -> &[u8] {
        &self.0
    }
}

impl<'a> From<ByteValueRef<'a>> for ByteValue {
    fn from(value: ByteValueRef<'a>) -> Self {
        value.to_owned()
    }
}

impl From<&str> for ByteValue {
    fn from(value: &str) -> Self {
        Self(value.as_bytes().to_vec())
    }
}

/// Errors produced while unescaping escaped byte sequences.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum UnescapeError {
    /// The input ends with an escape byte without a following byte.
    TrailingEscape,
}

/// Unescape byte sequences using `escape` and return borrowed input when possible.
///
/// If no `escape` byte is present, the returned [`Cow`] is borrowed and no
/// allocation occurs. If escapes are present, a new owned byte vector is
/// materialized with escape bytes removed.
pub fn unescape_lazy(input: &[u8], escape: u8) -> Result<Cow<'_, [u8]>, UnescapeError> {
    if !input.contains(&escape) {
        return Ok(Cow::Borrowed(input));
    }

    let mut output = Vec::with_capacity(input.len());
    let mut iter = input.iter().copied();
    while let Some(byte) = iter.next() {
        if byte == escape {
            let escaped = iter.next().ok_or(UnescapeError::TrailingEscape)?;
            output.push(escaped);
        } else {
            output.push(byte);
        }
    }

    Ok(Cow::Owned(output))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn borrowed_owned_roundtrip() {
        let bytes = b"hello";
        let borrowed = ByteValueRef::from_bytes(bytes);
        let owned = borrowed.to_owned();
        assert_eq!(owned.as_bytes(), bytes);
        assert_eq!(owned.as_ref().as_bytes(), bytes);
    }

    #[test]
    fn unescape_lazy_keeps_borrowed_when_no_escape_present() {
        let input = b"alpha/beta";
        let out = unescape_lazy(input, b'\\').expect("unescape should succeed");
        assert!(matches!(out, Cow::Borrowed(_)));
        assert_eq!(out.as_ref(), input);
    }

    #[test]
    fn unescape_lazy_allocates_when_escape_present() {
        let input = b"a\\nb";
        let out = unescape_lazy(input, b'\\').expect("unescape should succeed");
        assert!(matches!(out, Cow::Owned(_)));
        assert_eq!(out.as_ref(), b"anb");
    }

    #[test]
    fn unescape_lazy_errors_on_trailing_escape() {
        let input = b"a\\";
        let out = unescape_lazy(input, b'\\');
        assert_eq!(out, Err(UnescapeError::TrailingEscape));
    }
}

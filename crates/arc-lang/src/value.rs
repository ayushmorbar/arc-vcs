//! Borrowed/owned byte-value wrappers for zero-copy parser paths.

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
}

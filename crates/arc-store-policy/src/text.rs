use std::borrow::Cow;

/// Normalize path separators to `/` with borrow-first behavior.
pub(crate) fn normalize_slashes(input: &str) -> Cow<'_, str> {
    if input.as_bytes().contains(&b'\\') {
        Cow::Owned(input.replace('\\', "/"))
    } else {
        Cow::Borrowed(input)
    }
}

/// A two-buffer helper for transform pipelines.
#[derive(Debug, Default)]
pub(crate) struct Buffers {
    src: Vec<u8>,
    dest: Vec<u8>,
}

impl Buffers {
    pub(crate) fn clear(&mut self) {
        self.src.clear();
        self.dest.clear();
    }

    pub(crate) fn use_foreign_src<'a, 'src>(
        &'a mut self,
        src: &'src [u8],
    ) -> WithForeignSource<'src, 'a> {
        self.clear();
        WithForeignSource {
            ro_src: Some(src),
            src: &mut self.src,
            dest: &mut self.dest,
        }
    }

}

pub(crate) struct WithForeignSource<'src, 'bufs> {
    ro_src: Option<&'src [u8]>,
    src: &'bufs mut Vec<u8>,
    dest: &'bufs mut Vec<u8>,
}

impl WithForeignSource<'_, '_> {
    pub(crate) fn src_and_dest(&mut self) -> (&[u8], &mut Vec<u8>) {
        match self.ro_src {
            Some(src) => (src, self.dest),
            None => (self.src, self.dest),
        }
    }

    pub(crate) fn swap(&mut self) {
        self.ro_src.take();
        std::mem::swap(&mut self.src, &mut self.dest);
        self.dest.clear();
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn normalize_slashes_borrow_first() {
        let clean = "src/main.rs";
        assert!(matches!(normalize_slashes(clean), Cow::Borrowed(_)));

        let dirty = "src\\main.rs";
        assert_eq!(normalize_slashes(dirty).as_ref(), "src/main.rs");
    }

    #[test]
    fn buffers_foreign_source_lifecycle() {
        let mut bufs = Buffers::default();
        let mut bufs = bufs.use_foreign_src(b"a");

        let (src, dest) = bufs.src_and_dest();
        assert_eq!(src, b"a");
        dest.extend_from_slice(b"b");

        bufs.swap();
        let (src, dest) = bufs.src_and_dest();
        assert_eq!(src, b"b");
        assert!(dest.is_empty());
    }
}
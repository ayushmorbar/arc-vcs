use std::borrow::Cow;

/// Normalize path separators to `/` with borrow-first behavior.
pub(crate) fn normalize_slashes(input: &str) -> Cow<'_, str> {
    if input.as_bytes().contains(&b'\\') {
        Cow::Owned(input.replace('\\', "/"))
    } else {
        Cow::Borrowed(input)
    }
}

/// One logical line in an input stream.
#[derive(Debug, Clone, Copy)]
pub(crate) struct LineView<'a> {
    /// Raw line bytes as read, including the trailing newline if present.
    pub raw: &'a [u8],
    /// Line bytes without trailing newline and optional carriage return.
    pub content: &'a [u8],
    /// 1-based line number.
    pub line_no: usize,
}

/// Iterate byte lines while preserving the original bytes.
pub(crate) fn iter_lines(input: &[u8]) -> LineIter<'_> {
    LineIter { input, pos: 0, line_no: 1 }
}

/// Losslessly rewrite a byte stream line-by-line.
pub(crate) fn rewrite_lossless(
    input: &[u8],
    mut rewrite: impl FnMut(LineView<'_>, &mut Vec<u8>),
) -> Vec<u8> {
    let mut out = Vec::with_capacity(input.len());
    for line in iter_lines(input) {
        rewrite(line, &mut out);
    }
    out
}

pub(crate) struct LineIter<'a> {
    input: &'a [u8],
    pos: usize,
    line_no: usize,
}

impl<'a> Iterator for LineIter<'a> {
    type Item = LineView<'a>;

    fn next(&mut self) -> Option<Self::Item> {
        if self.pos >= self.input.len() {
            return None;
        }

        let start = self.pos;
        let mut end = self.pos;
        while end < self.input.len() && self.input[end] != b'\n' {
            end += 1;
        }
        let raw_end = if end < self.input.len() { end + 1 } else { end };
        self.pos = raw_end;

        let raw = &self.input[start..raw_end];
        let mut content_end = raw.len();
        if content_end > 0 && raw[content_end - 1] == b'\n' {
            content_end -= 1;
        }
        if content_end > 0 && raw[content_end - 1] == b'\r' {
            content_end -= 1;
        }
        let content = &raw[..content_end];

        let line = LineView { raw, content, line_no: self.line_no };
        self.line_no += 1;
        Some(line)
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
    fn iter_lines_preserves_raw_and_content() {
        let input = b"a\r\n#b\nlast";
        let lines: Vec<LineView<'_>> = iter_lines(input).collect();
        assert_eq!(lines.len(), 3);
        assert_eq!(lines[0].raw, b"a\r\n");
        assert_eq!(lines[0].content, b"a");
        assert_eq!(lines[1].raw, b"#b\n");
        assert_eq!(lines[1].content, b"#b");
        assert_eq!(lines[2].raw, b"last");
        assert_eq!(lines[2].content, b"last");
    }

    #[test]
    fn rewrite_lossless_identity() {
        let input = b"one\r\ntwo\nthree";
        let out = rewrite_lossless(input, |line, dest| dest.extend_from_slice(line.raw));
        assert_eq!(out, input);
    }

    #[test]
    fn normalize_slashes_empty_string() {
        let result = normalize_slashes("");
        assert!(matches!(result, Cow::Borrowed(_)));
        assert_eq!(result.as_ref(), "");
    }

    #[test]
    fn normalize_slashes_only_backslashes() {
        let result = normalize_slashes("\\a\\b\\");
        assert_eq!(result.as_ref(), "/a/b/");
    }

    #[test]
    fn normalize_slashes_mixed_separators() {
        let result = normalize_slashes("a/b\\c/d");
        assert_eq!(result.as_ref(), "a/b/c/d");
    }

    #[test]
    fn iter_lines_empty_input() {
        let lines: Vec<LineView<'_>> = iter_lines(b"").collect();
        assert!(lines.is_empty());
    }

    #[test]
    fn iter_lines_single_newline() {
        let lines: Vec<LineView<'_>> = iter_lines(b"\n").collect();
        assert_eq!(lines.len(), 1);
        assert_eq!(lines[0].raw, b"\n");
        assert_eq!(lines[0].content, b"");
        assert_eq!(lines[0].line_no, 1);
    }

    #[test]
    fn iter_lines_only_cr_lf() {
        let lines: Vec<LineView<'_>> = iter_lines(b"\r\n\r\n").collect();
        assert_eq!(lines.len(), 2);
        assert_eq!(lines[0].content, b"");
        assert_eq!(lines[1].content, b"");
    }

    #[test]
    fn iter_lines_line_numbers_are_one_based() {
        let input = b"a\nb\nc";
        let lines: Vec<LineView<'_>> = iter_lines(input).collect();
        assert_eq!(lines[0].line_no, 1);
        assert_eq!(lines[1].line_no, 2);
        assert_eq!(lines[2].line_no, 3);
    }

    #[test]
    fn rewrite_lossless_can_modify_content() {
        let input = b"hello\nworld";
        let out = rewrite_lossless(input, |line, dest| {
            dest.extend_from_slice(line.content);
            dest.push(b'!');
            dest.push(b'\n');
        });
        assert_eq!(out, b"hello!\nworld!\n");
    }

    #[test]
    fn rewrite_lossless_empty_input() {
        let out = rewrite_lossless(b"", |line, dest| dest.extend_from_slice(line.raw));
        assert!(out.is_empty());
    }
}

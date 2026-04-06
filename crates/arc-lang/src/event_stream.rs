//! Zero-copy event stream parser for textual inputs.

use crate::value::{ByteValue, ByteValueRef};

/// Borrowed parser events emitted directly from input slices.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum EventRef<'a> {
    /// A newline token (`\n` or `\r\n`).
    Newline(ByteValueRef<'a>),
    /// A run of horizontal whitespace.
    Whitespace(ByteValueRef<'a>),
    /// A comment line body beginning with `#` up to but not including newline.
    Comment(ByteValueRef<'a>),
    /// A run of identifier-like bytes.
    Word(ByteValueRef<'a>),
    /// A single byte symbol.
    Symbol(u8),
}

/// Owned event representation.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Event {
    /// Owned newline bytes.
    Newline(ByteValue),
    /// Owned whitespace bytes.
    Whitespace(ByteValue),
    /// Owned comment bytes.
    Comment(ByteValue),
    /// Owned word bytes.
    Word(ByteValue),
    /// Single byte symbol.
    Symbol(u8),
}

impl<'a> From<EventRef<'a>> for Event {
    fn from(value: EventRef<'a>) -> Self {
        match value {
            EventRef::Newline(v) => Self::Newline(v.into()),
            EventRef::Whitespace(v) => Self::Whitespace(v.into()),
            EventRef::Comment(v) => Self::Comment(v.into()),
            EventRef::Word(v) => Self::Word(v.into()),
            EventRef::Symbol(v) => Self::Symbol(v),
        }
    }
}

/// Parse `input` and stream zero-copy events to `dispatch`.
pub fn parse_event_stream<'a>(input: &'a [u8], mut dispatch: impl FnMut(EventRef<'a>)) {
    let mut i = 0usize;
    while i < input.len() {
        match input[i] {
            b'\r' => {
                if i + 1 < input.len() && input[i + 1] == b'\n' {
                    dispatch(EventRef::Newline(ByteValueRef::from_bytes(&input[i..i + 2])));
                    i += 2;
                } else {
                    dispatch(EventRef::Symbol(b'\r'));
                    i += 1;
                }
            }
            b'\n' => {
                dispatch(EventRef::Newline(ByteValueRef::from_bytes(&input[i..i + 1])));
                i += 1;
            }
            b'#' => {
                let start = i;
                i += 1;
                while i < input.len() && input[i] != b'\n' && input[i] != b'\r' {
                    i += 1;
                }
                dispatch(EventRef::Comment(ByteValueRef::from_bytes(&input[start..i])));
            }
            b if is_hspace(b) => {
                let start = i;
                i += 1;
                while i < input.len() && is_hspace(input[i]) {
                    i += 1;
                }
                dispatch(EventRef::Whitespace(ByteValueRef::from_bytes(&input[start..i])));
            }
            b if is_word_char(b) => {
                let start = i;
                i += 1;
                while i < input.len() && is_word_char(input[i]) {
                    i += 1;
                }
                dispatch(EventRef::Word(ByteValueRef::from_bytes(&input[start..i])));
            }
            b => {
                dispatch(EventRef::Symbol(b));
                i += 1;
            }
        }
    }
}

fn is_hspace(byte: u8) -> bool {
    byte == b' ' || byte == b'\t'
}

fn is_word_char(byte: u8) -> bool {
    byte.is_ascii_alphanumeric() || matches!(byte, b'_' | b'-' | b'.' | b'/')
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn tokenizes_zero_copy_events() {
        let src = b"key = value #c\nnext";
        let mut events = Vec::new();
        parse_event_stream(src, |event| events.push(Event::from(event)));

        assert_eq!(
            events,
            vec![
                Event::Word(ByteValue::from("key")),
                Event::Whitespace(ByteValue::from(" ")),
                Event::Symbol(b'='),
                Event::Whitespace(ByteValue::from(" ")),
                Event::Word(ByteValue::from("value")),
                Event::Whitespace(ByteValue::from(" ")),
                Event::Comment(ByteValue::from("#c")),
                Event::Newline(ByteValue::from("\n")),
                Event::Word(ByteValue::from("next")),
            ]
        );
    }

    #[test]
    fn preserves_crlf_newline_event() {
        let src = b"a\r\nb";
        let mut events = Vec::new();
        parse_event_stream(src, |event| events.push(Event::from(event)));
        assert_eq!(
            events,
            vec![
                Event::Word(ByteValue::from("a")),
                Event::Newline(ByteValue::from("\r\n")),
                Event::Word(ByteValue::from("b"))
            ]
        );
    }
}

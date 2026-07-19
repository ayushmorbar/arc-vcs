//! Pure EWAH bitmap decoding and iteration.
//!
//! This module intentionally contains no I/O and is safe to use in Wasm.

/// Errors emitted while decoding an EWAH bitmap.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum EwahDecodeError {
    /// Input ended before reading the expected header bytes.
    UnexpectedEof(&'static str),
    /// The encoded payload length does not fit into platform memory.
    LengthOverflow,
}

impl std::fmt::Display for EwahDecodeError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::UnexpectedEof(ctx) => write!(f, "unexpected EOF while reading {ctx}"),
            Self::LengthOverflow => write!(f, "bitmap word length overflows platform usize"),
        }
    }
}

impl std::error::Error for EwahDecodeError {}

/// Decoded EWAH bitmap.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct EwahBitmap {
    num_bits: u32,
    words: Vec<u64>,
    rlw: u32,
}

impl EwahBitmap {
    /// Decode one EWAH bitmap from `input`, returning the remaining bytes.
    pub fn decode(input: &[u8]) -> Result<(Self, &[u8]), EwahDecodeError> {
        let (num_bits, rest) = read_u32_be(input, "num_bits")?;
        let (word_len, mut rest) = read_u32_be(rest, "word_len")?;
        let word_len = usize::try_from(word_len).map_err(|_| EwahDecodeError::LengthOverflow)?;

        let bytes_len = word_len
            .checked_mul(std::mem::size_of::<u64>())
            .ok_or(EwahDecodeError::LengthOverflow)?;
        if rest.len() < bytes_len {
            return Err(EwahDecodeError::UnexpectedEof("word payload"));
        }

        let (word_bytes, tail) = rest.split_at(bytes_len);
        rest = tail;

        let mut words = Vec::with_capacity(word_len);
        for chunk in word_bytes.chunks_exact(8) {
            words.push(u64::from_be_bytes([
                chunk[0], chunk[1], chunk[2], chunk[3], chunk[4], chunk[5], chunk[6], chunk[7],
            ]));
        }

        let (rlw, rest) = read_u32_be(rest, "rlw")?;
        Ok((Self { num_bits, words, rlw }, rest))
    }

    /// Number of addressable bits in this bitmap.
    pub fn num_bits(&self) -> usize {
        usize::try_from(self.num_bits).unwrap_or(usize::MAX)
    }

    /// RLE pointer from encoded payload.
    pub fn rlw(&self) -> u32 {
        self.rlw
    }

    /// Iterate all set bit indices in ascending order.
    ///
    /// Returning `false` from `f` stops iteration early.
    pub fn for_each_set_bit(&self, mut f: impl FnMut(usize) -> bool) {
        let limit = self.num_bits();
        let mut index = 0usize;
        let mut iter = self.words.iter();

        while let Some(word) = iter.next() {
            if index >= limit {
                return;
            }

            if rlw_runbit_is_set(*word) {
                let len = rlw_running_len_bits(*word);
                for _ in 0..len {
                    if index >= limit {
                        return;
                    }
                    if !f(index) {
                        return;
                    }
                    index += 1;
                }
            } else {
                let skip = usize::try_from(rlw_running_len_bits(*word)).unwrap_or(usize::MAX);
                index = index.saturating_add(skip);
            }

            let literals = rlw_literal_words(*word);
            for _ in 0..literals {
                if index >= limit {
                    return;
                }
                let Some(literal) = iter.next() else {
                    return;
                };
                for bit_idx in 0..64 {
                    if index >= limit {
                        return;
                    }
                    if (literal & (1u64 << bit_idx)) != 0 && !f(index) {
                        return;
                    }
                    index += 1;
                }
            }
        }
    }
}

fn read_u32_be<'a>(input: &'a [u8], ctx: &'static str) -> Result<(u32, &'a [u8]), EwahDecodeError> {
    if input.len() < 4 {
        return Err(EwahDecodeError::UnexpectedEof(ctx));
    }
    let (head, tail) = input.split_at(4);
    Ok((u32::from_be_bytes([head[0], head[1], head[2], head[3]]), tail))
}

const RLW_RUNNING_BITS: u64 = 32;
const RLW_LARGEST_RUNNING_COUNT: u64 = (1 << RLW_RUNNING_BITS) - 1;

fn rlw_running_len(word: u64) -> u64 {
    (word >> 1) & RLW_LARGEST_RUNNING_COUNT
}

fn rlw_running_len_bits(word: u64) -> u64 {
    rlw_running_len(word) * 64
}

fn rlw_literal_words(word: u64) -> u64 {
    word >> (1 + RLW_RUNNING_BITS)
}

fn rlw_runbit_is_set(word: u64) -> bool {
    (word & 1) == 1
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn decode_and_iterate_set_bits() {
        // Encodes 64 bits, 1 RLW word + 1 literal word.
        let encoded = [
            0, 0, 0, 64, // total bits
            0, 0, 0, 2, // number of u64 words
            0, 0, 0, 2, 0, 0, 0, 0, // RLW: one literal word
            0, 0, 0, 0, 0, 0, 0, 21, // literal with bits 0,2,4 set
            0, 0, 0, 0, // rlw pointer
            1, 2, 3, // trailing bytes
        ];

        let (bitmap, tail) = EwahBitmap::decode(&encoded).expect("decode must succeed");
        assert_eq!(bitmap.num_bits(), 64);
        assert_eq!(bitmap.rlw(), 0);
        assert_eq!(tail, &[1, 2, 3]);

        let mut bits = Vec::new();
        bitmap.for_each_set_bit(|idx| {
            bits.push(idx);
            true
        });
        assert_eq!(bits, vec![0, 2, 4]);
    }

    #[test]
    fn decode_rejects_short_input() {
        let err = EwahBitmap::decode(&[0, 1, 2]).expect_err("must fail");
        assert!(matches!(err, EwahDecodeError::UnexpectedEof("num_bits")));
    }

    #[test]
    fn early_stop_callback_works() {
        let encoded =
            [0, 0, 0, 64, 0, 0, 0, 2, 0, 0, 0, 2, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 21, 0, 0, 0, 0];
        let (bitmap, _) = EwahBitmap::decode(&encoded).expect("decode");

        let mut seen = Vec::new();
        bitmap.for_each_set_bit(|idx| {
            seen.push(idx);
            idx != 2
        });
        assert_eq!(seen, vec![0, 2]);
    }

    #[test]
    fn does_not_emit_bits_past_declared_num_bits() {
        // Declares only 3 bits, but literal sets bits 0, 2 and 4.
        let encoded =
            [0, 0, 0, 3, 0, 0, 0, 2, 0, 0, 0, 2, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 21, 0, 0, 0, 0];
        let (bitmap, _) = EwahBitmap::decode(&encoded).expect("decode");

        let mut seen = Vec::new();
        bitmap.for_each_set_bit(|idx| {
            seen.push(idx);
            true
        });
        assert_eq!(seen, vec![0, 2]);
    }

    #[test]
    fn huge_run_stops_at_declared_limit() {
        // RLW with runbit=1 and very large running length, no literal words.
        let run_word: u64 = 1u64 | (u64::from(u32::MAX) << 1);
        let mut encoded = Vec::new();
        encoded.extend_from_slice(&5u32.to_be_bytes()); // num_bits
        encoded.extend_from_slice(&1u32.to_be_bytes()); // one u64 word
        encoded.extend_from_slice(&run_word.to_be_bytes());
        encoded.extend_from_slice(&0u32.to_be_bytes()); // rlw pointer

        let (bitmap, _) = EwahBitmap::decode(&encoded).expect("decode");
        let mut count = 0usize;
        bitmap.for_each_set_bit(|_| {
            count += 1;
            true
        });
        assert_eq!(count, 5);
    }
}

#![deny(rust_2018_idioms, missing_docs)]
#![forbid(unsafe_code)]

//! Pure sequence differencing abstractions and fallback edit-script generation.
//!
//! This crate is intentionally free of filesystem/network I/O and is Wasm-portable.

/// A single edit operation transforming the old sequence into the new sequence.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Edit {
    /// Delete old elements in the half-open range [before.start, before.end).
    Delete {
        /// Range in the old sequence.
        before: std::ops::Range<usize>,
    },
    /// Insert new elements in the half-open range [after.start, after.end).
    Insert {
        /// Insertion position in the old sequence, before this index.
        at: usize,
        /// Range in the new sequence.
        after: std::ops::Range<usize>,
    },
    /// Replace one range in the old sequence with one range in the new sequence.
    Replace {
        /// Range in the old sequence.
        before: std::ops::Range<usize>,
        /// Range in the new sequence.
        after: std::ops::Range<usize>,
    },
}

/// A compact sequence of edit operations.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct EditScript {
    edits: Vec<Edit>,
}

impl EditScript {
    /// Create an empty script.
    pub fn new() -> Self {
        Self { edits: Vec::new() }
    }

    /// Append one edit.
    pub fn push(&mut self, edit: Edit) {
        self.edits.push(edit);
    }

    /// Borrow all edits.
    pub fn edits(&self) -> &[Edit] {
        &self.edits
    }
}

/// A trait for differencers that generate edit scripts from two sequences.
pub trait EditScriptGenerator<T> {
    /// Generate an edit script that transforms `before` into `after`.
    fn diff(&mut self, before: &[T], after: &[T]) -> EditScript;
}

/// Reusable scratch memory for diffing and path storage.
///
/// `path_backing` is a contiguous byte arena suitable for compact metadata storage.
#[derive(Debug, Default, Clone)]
pub struct ScratchBuffer {
    dp: Vec<u32>,
    path_backing: Vec<u8>,
}

impl ScratchBuffer {
    /// Create an empty scratch buffer.
    pub fn new() -> Self {
        Self::default()
    }

    /// Ensure DP capacity for a grid of `(rows * cols)` cells.
    pub fn prepare_dp(&mut self, rows: usize, cols: usize) {
        let needed = rows.saturating_mul(cols);
        if self.dp.len() < needed {
            self.dp.resize(needed, 0);
        } else {
            self.dp[..needed].fill(0);
        }
    }

    /// Mutable DP slice for the exact shape requested by [`prepare_dp`].
    pub fn dp_mut(&mut self, rows: usize, cols: usize) -> &mut [u32] {
        self.prepare_dp(rows, cols);
        let needed = rows.saturating_mul(cols);
        &mut self.dp[..needed]
    }

    /// Store bytes in contiguous backing and return their range.
    pub fn intern_path(&mut self, bytes: &[u8]) -> std::ops::Range<usize> {
        let start = self.path_backing.len();
        self.path_backing.extend_from_slice(bytes);
        start..self.path_backing.len()
    }

    /// Resolve an interned range to bytes.
    pub fn resolve_path(&self, range: std::ops::Range<usize>) -> Option<&[u8]> {
        self.path_backing.get(range)
    }

    /// Reset transient state while optionally preserving allocations.
    pub fn clear_keep_allocation(&mut self) {
        self.path_backing.clear();
        self.dp.fill(0);
    }
}

/// A basic dynamic-programming differencer for fallback use.
///
/// This implementation optimizes memory reuse through [`ScratchBuffer`].
/// It guarantees a correct edit script but may emit insert/delete pairs
/// instead of coalesced replace edits depending on tie-breaking.
#[derive(Debug, Default, Clone)]
pub struct BasicDiffer {
    scratch: ScratchBuffer,
}

impl BasicDiffer {
    /// Create a new differ with owned scratch space.
    pub fn new() -> Self {
        Self::default()
    }

    fn idx(i: usize, j: usize, cols: usize) -> usize {
        i * cols + j
    }
}

impl<T: PartialEq> EditScriptGenerator<T> for BasicDiffer {
    fn diff(&mut self, before: &[T], after: &[T]) -> EditScript {
        let n = before.len();
        let m = after.len();
        let cols = m + 1;
        let rows = n + 1;

        self.scratch.prepare_dp(rows, cols);
        let dp = self.scratch.dp_mut(rows, cols);

        // LCS table fill.
        for i in (0..n).rev() {
            for j in (0..m).rev() {
                let cur = Self::idx(i, j, cols);
                if before[i] == after[j] {
                    dp[cur] = dp[Self::idx(i + 1, j + 1, cols)] + 1;
                } else {
                    dp[cur] = dp[Self::idx(i + 1, j, cols)].max(dp[Self::idx(i, j + 1, cols)]);
                }
            }
        }

        let mut script = EditScript::new();
        let mut i = 0usize;
        let mut j = 0usize;

        while i < n && j < m {
            if before[i] == after[j] {
                i += 1;
                j += 1;
                continue;
            }

            let delete_score = dp[Self::idx(i + 1, j, cols)];
            let insert_score = dp[Self::idx(i, j + 1, cols)];
            if delete_score >= insert_score {
                script.push(Edit::Delete { before: i..(i + 1) });
                i += 1;
            } else {
                script.push(Edit::Insert { at: i, after: j..(j + 1) });
                j += 1;
            }
        }

        while i < n {
            script.push(Edit::Delete { before: i..(i + 1) });
            i += 1;
        }
        while j < m {
            script.push(Edit::Insert { at: i, after: j..(j + 1) });
            j += 1;
        }

        coalesce_replace(script)
    }
}

fn coalesce_replace(script: EditScript) -> EditScript {
    let mut out = EditScript::new();
    let edits = script.edits;
    let mut idx = 0usize;

    while idx < edits.len() {
        if idx + 1 < edits.len()
            && let Edit::Delete { before } = &edits[idx]
            && let Edit::Insert { at, after } = &edits[idx + 1]
            && (*at == before.start || *at == before.end)
        {
            out.push(Edit::Replace { before: before.clone(), after: after.clone() });
            idx += 2;
            continue;
        }
        out.push(edits[idx].clone());
        idx += 1;
    }

    out
}

#[cfg(test)]
mod tests {
    use super::*;

    fn apply_script<T: Clone>(old: &[T], new: &[T], script: &EditScript) -> Vec<T> {
        let mut out = Vec::new();
        let mut cursor = 0usize;
        for edit in script.edits() {
            match edit {
                Edit::Delete { before: br } => {
                    out.extend_from_slice(&old[cursor..br.start]);
                    cursor = br.end;
                }
                Edit::Insert { at, after: ar } => {
                    out.extend_from_slice(&old[cursor..*at]);
                    out.extend_from_slice(&new[ar.start..ar.end]);
                    cursor = *at;
                }
                Edit::Replace { before: br, after: ar } => {
                    out.extend_from_slice(&old[cursor..br.start]);
                    out.extend_from_slice(&new[ar.start..ar.end]);
                    cursor = br.end;
                }
            }
        }
        out.extend_from_slice(&old[cursor..]);
        out
    }

    #[test]
    fn scratch_intern_roundtrip() {
        let mut scratch = ScratchBuffer::new();
        let r = scratch.intern_path(b"a/b/c");
        assert_eq!(scratch.resolve_path(r), Some(&b"a/b/c"[..]));
    }

    #[test]
    fn basic_diff_emits_insert_and_delete() {
        let mut d = BasicDiffer::new();
        let before = vec![1, 2, 3];
        let after = vec![1, 4, 3, 5];
        let script = d.diff(&before, &after);
        let reconstructed = apply_script(&before, &after, &script);
        assert_eq!(reconstructed, after);
    }

    #[test]
    fn basic_diff_handles_equal_sequences() {
        let mut d = BasicDiffer::new();
        let before = vec![1, 2, 3];
        let after = vec![1, 2, 3];
        let script = d.diff(&before, &after);
        assert!(script.edits().is_empty());
    }

    #[test]
    fn basic_diff_replace_shape_is_still_correct() {
        let mut d = BasicDiffer::new();
        let before = vec![1, 2, 3];
        let after = vec![1, 4, 3];
        let script = d.diff(&before, &after);
        assert!(script.edits().iter().any(|e| matches!(e, Edit::Replace { .. })));
        let reconstructed = apply_script(&before, &after, &script);
        assert_eq!(reconstructed, after);
    }
}

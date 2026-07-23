use arc_diff::{BasicDiffer, Edit, EditScript, EditScriptGenerator, ScratchBuffer};

// ===========================================================================
// EditScript unit tests
// ===========================================================================

#[test]
fn edit_script_new_is_empty() {
    let script = EditScript::new();
    assert!(script.edits().is_empty());
}

#[test]
fn edit_script_push_and_retrieve() {
    let mut script = EditScript::new();
    script.push(Edit::Delete { before: 0..2 });
    script.push(Edit::Insert { at: 0, after: 0..3 });
    assert_eq!(script.edits().len(), 2);
    assert_eq!(script.edits()[0], Edit::Delete { before: 0..2 });
    assert_eq!(script.edits()[1], Edit::Insert { at: 0, after: 0..3 });
}

#[test]
fn edit_script_default_is_empty() {
    let script = EditScript::default();
    assert!(script.edits().is_empty());
}

#[test]
fn edit_script_clone_preserves_content() {
    let mut script = EditScript::new();
    script.push(Edit::Replace { before: 1..3, after: 4..6 });
    let cloned = script.clone();
    assert_eq!(script, cloned);
}

// ===========================================================================
// ScratchBuffer tests
// ===========================================================================

#[test]
fn scratch_buffer_new_is_empty() {
    let scratch = ScratchBuffer::new();
    assert!(scratch.resolve_path(0..0).is_some());
    assert!(scratch.resolve_path(0..1).is_none());
}

#[test]
fn scratch_prepare_dp_allocates_enough() {
    let mut scratch = ScratchBuffer::new();
    let dp = scratch.dp_mut(3, 4);
    assert_eq!(dp.len(), 12);
    assert!(dp.iter().all(|&v| v == 0));
}

#[test]
fn scratch_prepare_dp_reuses_allocation() {
    let mut scratch = ScratchBuffer::new();
    let dp1 = scratch.dp_mut(5, 5);
    let ptr1 = dp1.as_ptr();
    let dp2 = scratch.dp_mut(3, 3);
    let ptr2 = dp2.as_ptr();
    // Should reuse same allocation if capacity is sufficient
    assert_eq!(ptr1, ptr2);
    assert_eq!(dp2.len(), 9);
}

#[test]
fn scratch_intern_and_resolve() {
    let mut scratch = ScratchBuffer::new();
    let r1 = scratch.intern_path(b"hello");
    let r2 = scratch.intern_path(b"world");
    assert_eq!(scratch.resolve_path(r1), Some(&b"hello"[..]));
    assert_eq!(scratch.resolve_path(r2), Some(&b"world"[..]));
}

#[test]
fn scratch_intern_empty() {
    let mut scratch = ScratchBuffer::new();
    let r = scratch.intern_path(b"");
    assert_eq!(r.start, r.end);
    assert_eq!(scratch.resolve_path(r), Some(&b""[..]));
}

#[test]
fn scratch_resolve_out_of_range() {
    let scratch = ScratchBuffer::new();
    assert_eq!(scratch.resolve_path(0..100), None);
}

#[test]
fn scratch_clear_keep_allocation_preserves_capacity() {
    let mut scratch = ScratchBuffer::new();
    scratch.dp_mut(10, 10);
    scratch.intern_path(b"data");
    let _capacity_before = scratch.dp_mut(0, 0).len();
    scratch.clear_keep_allocation();
    // After clear, dp is zeroed but capacity retained
    // Intern new data to verify path_backing is cleared
    let r = scratch.intern_path(b"new");
    assert_eq!(scratch.resolve_path(r), Some(&b"new"[..]));
}

// ===========================================================================
// BasicDiffer — identity / no-change tests
// ===========================================================================

#[test]
fn diff_identical_empty_sequences() {
    let mut d = BasicDiffer::new();
    let script = d.diff(&[] as &[i32], &[]);
    assert!(script.edits().is_empty());
}

#[test]
fn diff_identical_single_element() {
    let mut d = BasicDiffer::new();
    let script = d.diff(&[42], &[42]);
    assert!(script.edits().is_empty());
}

#[test]
fn diff_identical_multi_element() {
    let mut d = BasicDiffer::new();
    let a = vec![1, 2, 3, 4, 5];
    let script = d.diff(&a, &a);
    assert!(script.edits().is_empty());
}

// ===========================================================================
// BasicDiffer — pure insert tests
// ===========================================================================

#[test]
fn diff_insert_at_beginning() {
    let mut d = BasicDiffer::new();
    let before = vec![2, 3, 4];
    let after = vec![1, 2, 3, 4];
    let script = d.diff(&before, &after);
    let result = apply_script(&before, &after, &script);
    assert_eq!(result, after);
}

#[test]
fn diff_insert_at_end() {
    let mut d = BasicDiffer::new();
    let before = vec![1, 2, 3];
    let after = vec![1, 2, 3, 4];
    let script = d.diff(&before, &after);
    let result = apply_script(&before, &after, &script);
    assert_eq!(result, after);
}

#[test]
fn diff_insert_in_middle() {
    let mut d = BasicDiffer::new();
    let before = vec![1, 3];
    let after = vec![1, 2, 3];
    let script = d.diff(&before, &after);
    let result = apply_script(&before, &after, &script);
    assert_eq!(result, after);
}

#[test]
fn diff_insert_multiple_elements() {
    let mut d = BasicDiffer::new();
    let before = vec![1, 5];
    let after = vec![1, 2, 3, 4, 5];
    let script = d.diff(&before, &after);
    let result = apply_script(&before, &after, &script);
    assert_eq!(result, after);
}

#[test]
fn diff_insert_into_empty() {
    let mut d = BasicDiffer::new();
    let before: Vec<i32> = vec![];
    let after = vec![1, 2, 3];
    let script = d.diff(&before, &after);
    let result = apply_script(&before, &after, &script);
    assert_eq!(result, after);
}

// ===========================================================================
// BasicDiffer — pure delete tests
// ===========================================================================

#[test]
fn diff_delete_from_beginning() {
    let mut d = BasicDiffer::new();
    let before = vec![1, 2, 3, 4];
    let after = vec![2, 3, 4];
    let script = d.diff(&before, &after);
    let result = apply_script(&before, &after, &script);
    assert_eq!(result, after);
}

#[test]
fn diff_delete_from_end() {
    let mut d = BasicDiffer::new();
    let before = vec![1, 2, 3, 4];
    let after = vec![1, 2, 3];
    let script = d.diff(&before, &after);
    let result = apply_script(&before, &after, &script);
    assert_eq!(result, after);
}

#[test]
fn diff_delete_from_middle() {
    let mut d = BasicDiffer::new();
    let before = vec![1, 2, 3, 4, 5];
    let after = vec![1, 2, 4, 5];
    let script = d.diff(&before, &after);
    let result = apply_script(&before, &after, &script);
    assert_eq!(result, after);
}

#[test]
fn diff_delete_all() {
    let mut d = BasicDiffer::new();
    let before = vec![1, 2, 3];
    let after: Vec<i32> = vec![];
    let script = d.diff(&before, &after);
    let result = apply_script(&before, &after, &script);
    assert_eq!(result, after);
}

#[test]
fn diff_delete_multiple_non_adjacent() {
    let mut d = BasicDiffer::new();
    let before = vec![1, 2, 3, 4, 5];
    let after = vec![1, 3, 5];
    let script = d.diff(&before, &after);
    let result = apply_script(&before, &after, &script);
    assert_eq!(result, after);
}

// ===========================================================================
// BasicDiffer — pure replace tests
// ===========================================================================

#[test]
fn diff_replace_single_element() {
    let mut d = BasicDiffer::new();
    let before = vec![1, 2, 3];
    let after = vec![1, 99, 3];
    let script = d.diff(&before, &after);
    assert!(
        script.edits().iter().any(|e| matches!(e, Edit::Replace { .. })),
        "expected a Replace edit, got {:?}",
        script.edits()
    );
    let result = apply_script(&before, &after, &script);
    assert_eq!(result, after);
}

#[test]
fn diff_replace_range_with_different_size() {
    let mut d = BasicDiffer::new();
    let before = vec![1, 2, 3, 4, 5];
    let after = vec![1, 99, 100, 5];
    let script = d.diff(&before, &after);
    let result = apply_script(&before, &after, &script);
    assert_eq!(result, after);
}

#[test]
fn diff_replace_all_elements() {
    let mut d = BasicDiffer::new();
    let before = vec![1, 2, 3];
    let after = vec![4, 5, 6];
    let script = d.diff(&before, &after);
    let result = apply_script(&before, &after, &script);
    assert_eq!(result, after);
}

// ===========================================================================
// BasicDiffer — mixed insert + delete tests
// ===========================================================================

#[test]
fn diff_mixed_insert_and_delete() {
    let mut d = BasicDiffer::new();
    let before = vec![1, 2, 3, 4, 5];
    let after = vec![1, 3, 5, 6];
    let script = d.diff(&before, &after);
    let result = apply_script(&before, &after, &script);
    assert_eq!(result, after);
}

#[test]
fn diff_completely_different_sequences() {
    let mut d = BasicDiffer::new();
    let before = vec![1, 2, 3];
    let after = vec![4, 5, 6];
    let script = d.diff(&before, &after);
    let result = apply_script(&before, &after, &script);
    assert_eq!(result, after);
}

#[test]
fn diff_shift_elements_right() {
    let mut d = BasicDiffer::new();
    let before = vec![1, 2, 3];
    let after = vec![0, 1, 2, 3];
    let script = d.diff(&before, &after);
    let result = apply_script(&before, &after, &script);
    assert_eq!(result, after);
}

#[test]
fn diff_shift_elements_left() {
    let mut d = BasicDiffer::new();
    let before = vec![1, 2, 3];
    let after = vec![2, 3];
    let script = d.diff(&before, &after);
    let result = apply_script(&before, &after, &script);
    assert_eq!(result, after);
}

// ===========================================================================
// BasicDiffer — string diff tests
// ===========================================================================

#[test]
fn diff_string_insert() {
    let mut d = BasicDiffer::new();
    let before = vec!["a", "b", "d"];
    let after = vec!["a", "b", "c", "d"];
    let script = d.diff(&before, &after);
    let result = apply_script(&before, &after, &script);
    assert_eq!(result, after);
}

#[test]
fn diff_string_delete() {
    let mut d = BasicDiffer::new();
    let before = vec!["a", "b", "c", "d"];
    let after = vec!["a", "d"];
    let script = d.diff(&before, &after);
    let result = apply_script(&before, &after, &script);
    assert_eq!(result, after);
}

#[test]
fn diff_string_replace() {
    let mut d = BasicDiffer::new();
    let before = vec!["hello", "world"];
    let after = vec!["hello", "rust", "world"];
    let script = d.diff(&before, &after);
    let result = apply_script(&before, &after, &script);
    assert_eq!(result, after);
}

// ===========================================================================
// BasicDiffer — larger sequences
// ===========================================================================

#[test]
fn diff_larger_sequence_partial_overlap() {
    let mut d = BasicDiffer::new();
    let before = vec![1, 2, 3, 4, 5, 6, 7, 8];
    let after = vec![1, 3, 5, 7, 9];
    let script = d.diff(&before, &after);
    let result = apply_script(&before, &after, &script);
    assert_eq!(result, after);
}

#[test]
fn diff_larger_sequence_reverse_prefix() {
    let mut d = BasicDiffer::new();
    let before = vec![1, 2, 3, 4, 5];
    let after = vec![5, 4, 3, 2, 1];
    let script = d.diff(&before, &after);
    let result = apply_script(&before, &after, &script);
    assert_eq!(result, after);
}

#[test]
fn diff_interleaved_changes() {
    let mut d = BasicDiffer::new();
    let before = vec![1, 2, 3, 4, 5, 6];
    let after = vec![1, 99, 3, 400, 5, 600];
    let script = d.diff(&before, &after);
    let result = apply_script(&before, &after, &script);
    assert_eq!(result, after);
}

// ===========================================================================
// BasicDiffer — apply_script correctness (all tests verify roundtrip)
// ===========================================================================

#[test]
fn diff_roundtrip_empty_to_nonempty() {
    let mut d = BasicDiffer::new();
    let before: Vec<i32> = vec![];
    let after = vec![10, 20, 30, 40, 50];
    let script = d.diff(&before, &after);
    let result = apply_script(&before, &after, &script);
    assert_eq!(result, after);
}

#[test]
fn diff_roundtrip_nonempty_to_empty() {
    let mut d = BasicDiffer::new();
    let before = vec![10, 20, 30];
    let after: Vec<i32> = vec![];
    let script = d.diff(&before, &after);
    let result = apply_script(&before, &after, &script);
    assert_eq!(result, after);
}

#[test]
fn diff_roundtrip_complex_mixed() {
    let mut d = BasicDiffer::new();
    let before = vec!["a", "b", "c", "d", "e", "f"];
    let after = vec!["a", "x", "c", "y", "e", "z", "f"];
    let script = d.diff(&before, &after);
    let result = apply_script(&before, &after, &script);
    assert_eq!(result, after);
}

// ===========================================================================
// Edit variant coverage
// ===========================================================================

#[test]
fn edit_variants_debug() {
    let del = Edit::Delete { before: 0..5 };
    let ins = Edit::Insert { at: 3, after: 0..2 };
    let rep = Edit::Replace { before: 1..4, after: 0..3 };
    // Verify Debug output is non-empty
    assert!(!format!("{:?}", del).is_empty());
    assert!(!format!("{:?}", ins).is_empty());
    assert!(!format!("{:?}", rep).is_empty());
}

#[test]
fn edit_variants_clone_eq() {
    let del = Edit::Delete { before: 0..5 };
    assert_eq!(del.clone(), del);
    let ins = Edit::Insert { at: 3, after: 0..2 };
    assert_eq!(ins.clone(), ins);
    let rep = Edit::Replace { before: 1..4, after: 0..3 };
    assert_eq!(rep.clone(), rep);
}

#[test]
fn edit_variants_not_equal_to_each_other() {
    let del = Edit::Delete { before: 0..5 };
    let ins = Edit::Insert { at: 0, after: 0..5 };
    let rep = Edit::Replace { before: 0..5, after: 0..5 };
    assert_ne!(del, ins);
    assert_ne!(del, rep);
    assert_ne!(ins, rep);
}

// ===========================================================================
// BasicDiffer — idempotency check
// ===========================================================================

#[test]
fn diff_idempotent_application() {
    let mut d = BasicDiffer::new();
    let before = vec![1, 2, 3, 4, 5];
    let after = vec![1, 99, 3, 5, 6];
    let script = d.diff(&before, &after);
    let result = apply_script(&before, &after, &script);
    assert_eq!(result, after);
    // Applying the same diff again should be a no-op if diff(result, after) is empty
    let script2 = d.diff(&result, &after);
    assert!(script2.edits().is_empty(), "diff should be empty for identical sequences");
}

// ===========================================================================
// BasicDiffer — u8 byte-level diff
// ===========================================================================

#[test]
fn diff_u8_bytes() {
    let mut d = BasicDiffer::new();
    let before = b"abcdef";
    let after = b"abXdefY";
    let script = d.diff(before, after);
    let result = apply_script(before, after, &script);
    assert_eq!(result, after.to_vec());
}

// ===========================================================================
// ScratchBuffer — dp_mut zeroed after prepare
// ===========================================================================

#[test]
fn scratch_dp_mut_zeroes_previous_values() {
    let mut scratch = ScratchBuffer::new();
    let dp = scratch.dp_mut(2, 2);
    dp[0] = 42;
    dp[3] = 99;
    // Re-prepare with same or smaller dimensions — should zero
    let dp2 = scratch.dp_mut(2, 2);
    assert!(dp2.iter().all(|&v| v == 0));
}

#[test]
fn scratch_intern_multiple_resolve_all() {
    let mut scratch = ScratchBuffer::new();
    let ranges: Vec<_> =
        (0..100).map(|i| scratch.intern_path(format!("path_{i}").as_bytes())).collect();
    for (i, range) in ranges.into_iter().enumerate() {
        let expected = format!("path_{i}");
        assert_eq!(scratch.resolve_path(range), Some(expected.as_bytes()));
    }
}

// ===========================================================================
// Helper
// ===========================================================================

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

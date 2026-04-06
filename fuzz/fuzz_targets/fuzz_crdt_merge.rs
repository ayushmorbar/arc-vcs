#![no_main]

use std::collections::HashSet;

use arc_algebra_types::{Atom, Blake3Hash};
use arc_change::Change;
use arc_core::algebra::commute::{commute_pair, commutes};
use arc_core::store::author::test_keypair;
use libfuzzer_sys::fuzz_target;

fn next_chunk<'a>(data: &'a [u8], cursor: &mut usize, len: usize) -> &'a [u8] {
    if data.is_empty() {
        return &[];
    }
    let start = *cursor % data.len();
    let end = (start + len).min(data.len());
    *cursor = end;
    &data[start..end]
}

fn mk_hash(seed: &[u8]) -> Blake3Hash {
    let mut out = [0u8; 32];
    for (i, b) in seed.iter().enumerate() {
        out[i % 32] ^= *b;
    }
    out
}

fn mk_path(seed: &[u8], prefix: &str) -> Vec<String> {
    let node = seed
        .iter()
        .take(8)
        .map(|b| char::from((b % 26) + b'a'))
        .collect::<String>();
    vec![prefix.to_string(), node]
}

fn mk_atom(seed: &[u8], alt: &[u8]) -> Atom {
    if seed.first().copied().unwrap_or(0) % 2 == 0 {
        Atom::Insert {
            at: mk_path(seed, "node"),
            content_hash: mk_hash(alt),
        }
    } else {
        Atom::Delete {
            at: mk_path(seed, "node"),
            prior_hash: mk_hash(alt),
        }
    }
}

fuzz_target!(|data: &[u8]| {
    let (author, signing_key) = test_keypair();
    let signer = (author.clone(), signing_key.clone());
    let mut cursor = 0usize;

    let a_seed = next_chunk(data, &mut cursor, 32);
    let b_seed = next_chunk(data, &mut cursor, 32);
    let a_alt = next_chunk(data, &mut cursor, 32);
    let b_alt = next_chunk(data, &mut cursor, 32);

    let a = Change::new(
        HashSet::new(),
        vec![mk_atom(a_seed, a_alt)],
        "fuzz-a",
        author.clone(),
        &signing_key,
    );

    let b_deps = if data.first().copied().unwrap_or(0) % 3 == 0 {
        HashSet::from([a.id])
    } else {
        HashSet::new()
    };

    let b = Change::new(
        b_deps,
        vec![mk_atom(b_seed, b_alt)],
        "fuzz-b",
        author,
        &signing_key,
    );

    let _ = commutes(&a, &b);
    let _ = commute_pair(&a, &b, &signer);

    let _ = a.verify_signature();
    let _ = b.verify_signature();
});

//! Semantic diff rendering engine — dual-view: text diff and AST diff.
//!
//! Translates arc's mathematical AST [`Atom`]s into two complementary
//! human-readable terminal views:
//!
//! ## `arc diff` — Sesame-Aligned Text Diff (Micro view)
//!
//! [`group_and_render`] re-projects atoms back into text and applies three
//! techniques from recent diff-UX research:
//!
//! 1. **RefactoringMiner intent annotation** — [`Atom::Move`] and
//!    [`Atom::SemanticsPreserving`] atoms are printed as labelled `≈ [Move]`
//!    / `≈ [Refactor]` lines *before* the textual diff so reviewers grasp
//!    intent first, rather than deciphering raw line noise.
//!
//! 2. **Sesame syntactic alignment** — before running the line differ, rigid
//!    newlines are injected at structural boundaries (`{`, `}`, `;`) so the
//!    algorithm cannot misalign braces across logical code blocks.
//!
//!    > **Note:** this heuristic operates on raw text and will also break
//!    > occurrences inside string literals (e.g. `let s = " {";`).  A future
//!    > enhancement will leverage tree-sitter byte-range information to
//!    > restrict substitution to non-literal regions.
//!
//! 3. **BDiff-inspired inline sub-expression highlighting** — the `similar`
//!    crate's `iter_inline_changes` identifies the exact changed sub-tokens
//!    within each line; those tokens are highlighted with a colour-reversed
//!    background while the surrounding unchanged text on the same line is
//!    displayed in a plain foreground colour.  This replicates the Kuhn–
//!    Munkres optimal-matching insight without a full graph solver.
//!
//! ## `arc diff --semantic` — Structural AST Diff (Macro view)
//!
//! [`group_and_render_semantic`] renders each pending atom as a named
//! structural operation instead of raw text lines.  Multi-mappings (e.g.
//! three deletion sites that all feed one extracted method) are shown
//! explicitly — something a text diff cannot express.
//!
//! **Recommended workflow:** use `--semantic` first to grasp architectural
//! intent, then run plain `arc diff` to verify exact syntax and formatting.

use std::collections::{BTreeMap, HashMap};
use std::path::Path;

use anyhow::Result;
use arc_core::algebra::Atom;
use owo_colors::OwoColorize;
use similar::{ChangeTag, TextDiff};

/// Render a semantic diff for all pending changes in one `arc diff` invocation.
///
/// Groups atoms by file path (sorted via [`BTreeMap`] for deterministic
/// output), reads the new text from disk (degrading gracefully to an empty
/// string when a file has been deleted), and delegates per-file rendering to
/// [`render_diff`].
///
/// Non-file structural atoms ([`Atom::Directory`], [`Atom::Blob`],
/// [`Atom::Mount`]) are printed as brief intent lines directly, mirroring the
/// colour conventions used by `arc status`.
pub fn group_and_render(
    atoms: &[Atom],
    old_texts: &HashMap<String, String>,
    work_root: &Path,
) -> Result<()> {
    // Bucket file-level atoms by their filepath (NodePath component [1]).
    // BTreeMap guarantees lexicographically sorted output with no extra dep.
    let mut per_file: BTreeMap<String, Vec<&Atom>> = BTreeMap::new();

    for atom in atoms {
        match atom {
            Atom::Insert { at, .. }
            | Atom::Delete { at, .. }
            | Atom::SemanticsPreserving { at, .. }
                if at.first().map(|s| s == "file").unwrap_or(false) && at.len() > 1 =>
            {
                per_file.entry(at[1].clone()).or_default().push(atom);
            }
            Atom::Move { from, to } => {
                if from.first().map(|s| s == "file").unwrap_or(false) && from.len() > 1 {
                    per_file.entry(from[1].clone()).or_default().push(atom);
                }
                // If the Move crosses files, also record the destination.
                if to.first().map(|s| s == "file").unwrap_or(false)
                    && to.len() > 1
                    && to[1] != from[1]
                {
                    per_file.entry(to[1].clone()).or_default().push(atom);
                }
            }
            // Structural atoms without associated file text: print a brief
            // intent line and continue — these must not crash the text diff.
            other => println!("{}", format_atom_brief(other)),
        }
    }

    for (filepath, file_atoms) in &per_file {
        let old_text = old_texts.get(filepath).map(String::as_str).unwrap_or("");
        // Gracefully degrade when the file was deleted: read_to_string returns
        // an OS NotFound error which we collapse to an empty string so that
        // the diff can still render the deletion correctly via old_text.
        let new_text = std::fs::read_to_string(work_root.join(filepath)).unwrap_or_default();

        render_diff(file_atoms, old_text, &new_text, filepath)?;
    }

    Ok(())
}

/// Render a full Sesame-aligned semantic diff for a single file.
///
/// # Pipeline
///
/// 1. **Header** — `diff --arc a/{path} b/{path}` (bold)
/// 2. **Intent annotation** — Move / SemanticsPreserving atoms are named
///    before the text hunks.
/// 3. **Boilerplate collapse** — if every changed line is a `use`/`import`/
///    `#include` directive, emit a single summary line instead of a full diff.
/// 4. **Mega-file guard** — files whose combined old+new size exceeds 1 MB
///    skip the inline LCS calculation to avoid a CPU lock.
/// 5. **Sesame alignment** — inject structural newlines before running `TextDiff`.
/// 6. **Inline sub-expression highlighting** via `similar::iter_inline_changes`.
/// 7. **Summary footer** — `∑ +N -N ~N refactorings`.
pub fn render_diff(atoms: &[&Atom], old_text: &str, new_text: &str, file_path: &str) -> Result<()> {
    // ── 1. Header ────────────────────────────────────────────────────────────
    println!(
        "{}",
        format!("diff --arc a/{file_path} b/{file_path}").bold()
    );

    // ── 2. Refactoring intent annotation ─────────────────────────────────────
    let mut insertions: usize = 0;
    let mut deletions: usize = 0;
    let mut refactors: usize = 0;

    for atom in atoms {
        match atom {
            Atom::Move { from, to } => {
                let from_node = from.last().map(String::as_str).unwrap_or("?");
                let to_node = to.last().map(String::as_str).unwrap_or("?");
                println!(
                    "  {} {} → {}",
                    "≈ [Move]".yellow().bold(),
                    from_node.dimmed(),
                    to_node.dimmed(),
                );
                refactors += 1;
            }
            Atom::SemanticsPreserving { at, description } => {
                let node = at.last().map(String::as_str).unwrap_or("?");
                println!(
                    "  {} {} ({})",
                    "≈ [Refactor]".yellow().bold(),
                    node.dimmed(),
                    description.as_str().dimmed(),
                );
                refactors += 1;
            }
            Atom::Insert { .. } => insertions += 1,
            Atom::Delete { .. } => deletions += 1,
            _ => {}
        }
    }

    // ── 3. Boilerplate collapse ───────────────────────────────────────────────
    // Only collapse when *both* sides are non-empty (an entirely new file made
    // of use-statements should still be shown, not silently suppressed).
    if !old_text.is_empty() && !new_text.is_empty() && is_pure_import_change(old_text, new_text) {
        println!(
            "{}",
            "@@ [Boilerplate] Import / use declarations modified @@".cyan()
        );
        println!(
            "  {} +{} -{}",
            "∑".magenta(),
            insertions.to_string().green(),
            deletions.to_string().red(),
        );
        return Ok(());
    }

    // ── 4. Sesame alignment ───────────────────────────────────────────────────
    // Inject newlines at structural boundaries so the line differ cannot
    // straddle opening braces or run statements together.
    // NOTE: operates on raw text — see module-level note about string literals.
    let old_aligned = sesame_align(old_text);
    let new_aligned = sesame_align(new_text);

    // ── 5. Mega-file guard ────────────────────────────────────────────────────
    // The `similar` crate's LCS algorithm is O(n²) in the worst case.
    // Skip inline diffing for very large files to prevent a CPU lock.
    const SIZE_LIMIT: usize = 1_000_000; // 1 MB combined
    if old_aligned.len() + new_aligned.len() > SIZE_LIMIT {
        println!(
            "{}",
            "∆ [Change] File too large for inline diff — AST atoms shown above.".yellow()
        );
        println!(
            "  {} +{} -{} ~{}",
            "∑".magenta(),
            insertions.to_string().green(),
            deletions.to_string().red(),
            refactors.to_string().yellow(),
        );
        return Ok(());
    }

    // ── 6. BDiff-inspired inline sub-expression highlighting ─────────────────
    let diff = TextDiff::from_lines(old_aligned.as_str(), new_aligned.as_str());

    for (hunk_index, group) in diff.grouped_ops(3).into_iter().enumerate() {
        if hunk_index > 0 {
            println!("{}", format!("{:-<50}", "").dimmed());
        }
        for op in &group {
            for change in diff.iter_inline_changes(op) {
                let tag = change.tag();

                // Print the +/- sigil with its colour, then a separator.
                match tag {
                    ChangeTag::Delete => print!("{} ", "-".red().bold()),
                    ChangeTag::Insert => print!("{} ", "+".green().bold()),
                    ChangeTag::Equal => print!("  "),
                }

                // Print each sub-token: emphasized (changed) tokens get a
                // reversed background; unchanged tokens on the same line get
                // a plain foreground colour.
                for (emphasized, value) in change.iter_strings_lossy() {
                    match (tag, emphasized) {
                        (ChangeTag::Delete, true) => print!("{}", value.bold().on_red()),
                        (ChangeTag::Delete, false) => print!("{}", value.red()),
                        (ChangeTag::Insert, true) => print!("{}", value.bold().on_green()),
                        (ChangeTag::Insert, false) => print!("{}", value.green()),
                        (ChangeTag::Equal, _) => print!("{value}"),
                    }
                }

                if change.missing_newline() {
                    println!();
                }
            }
        }
    }

    // ── 7. Summary footer ─────────────────────────────────────────────────────
    println!(
        "\n  {} +{} -{} ~{}",
        "∑".magenta(),
        insertions.to_string().green(),
        deletions.to_string().red(),
        refactors.to_string().yellow(),
    );

    Ok(())
}

// ── Semantic (AST intent) view ────────────────────────────────────────────────

/// Render a structural AST diff (the "Macro" view) for all pending changes.
///
/// Unlike [`group_and_render`] which re-renders atoms as a Sesame-aligned text
/// diff, this function shows the *intent* of each atom as a named structural
/// operation — [`Atom::Insert`], [`Atom::Delete`], [`Atom::Move`], and
/// [`Atom::SemanticsPreserving`] atoms — so reviewers can understand *what*
/// changed architecturally before drilling into the textual execution with
/// plain `arc diff`.
///
/// Multi-mappings are shown explicitly: if three deletion sites all map to one
/// newly-extracted method, each appears as its own `[-] Delete` line referencing
/// the same extracted target, something a text diff cannot express compactly.
///
/// Non-file structural atoms ([`Atom::Directory`], [`Atom::Blob`],
/// [`Atom::Mount`]) are printed inline using the same brief format as
/// [`group_and_render`].
pub fn group_and_render_semantic(atoms: &[Atom]) -> Result<()> {
    let mut per_file: BTreeMap<String, Vec<&Atom>> = BTreeMap::new();

    for atom in atoms {
        match atom {
            Atom::Insert { at, .. }
            | Atom::Delete { at, .. }
            | Atom::SemanticsPreserving { at, .. }
                if at.first().map(|s| s == "file").unwrap_or(false) && at.len() > 1 =>
            {
                per_file.entry(at[1].clone()).or_default().push(atom);
            }
            Atom::Move { from, to } => {
                if from.first().map(|s| s == "file").unwrap_or(false) && from.len() > 1 {
                    per_file.entry(from[1].clone()).or_default().push(atom);
                }
                if to.first().map(|s| s == "file").unwrap_or(false)
                    && to.len() > 1
                    && to[1] != from[1]
                {
                    per_file.entry(to[1].clone()).or_default().push(atom);
                }
            }
            other => println!("{}", format_atom_brief(other)),
        }
    }

    for (filepath, file_atoms) in &per_file {
        render_semantic_file(filepath, file_atoms);
    }

    Ok(())
}

/// Render the structural AST operations for a single file.
///
/// Prints a `semantic --arc <path>` header followed by one labelled line per
/// atom.  Colour conventions:
/// - Green `[+]` — insertion
/// - Red `[-]` — deletion
/// - Yellow `[~]` — move (cross- or intra-file)
/// - Yellow `[≈]` — semantics-preserving refactoring
fn render_semantic_file(file_path: &str, atoms: &[&Atom]) {
    println!("{}", format!("semantic --arc {file_path}").bold());

    let mut insertions: usize = 0;
    let mut deletions: usize = 0;
    let mut refactors: usize = 0;

    for atom in atoms {
        match atom {
            Atom::Insert { at, .. } => {
                let kind = infer_node_kind(at);
                let name = at.last().map(String::as_str).unwrap_or("?");
                println!(
                    "  {} {} {}",
                    "[+]".green().bold(),
                    format!("Insert {kind}:").green(),
                    format!("'{name}'").green().bold(),
                );
                insertions += 1;
            }
            Atom::Delete { at, .. } => {
                let kind = infer_node_kind(at);
                let name = at.last().map(String::as_str).unwrap_or("?");
                println!(
                    "  {} {} {}",
                    "[-]".red().bold(),
                    format!("Delete {kind}:").red(),
                    format!("'{name}'").red().bold(),
                );
                deletions += 1;
            }
            Atom::Move { from, to } => {
                let from_name = from.last().map(String::as_str).unwrap_or("?");
                let to_name = to.last().map(String::as_str).unwrap_or("?");
                // Detect cross-file moves and show the destination file.
                let to_file = if to.first().map(|s| s == "file").unwrap_or(false) && to.len() > 1 {
                    to[1].as_str()
                } else {
                    file_path
                };
                if to_file != file_path {
                    println!(
                        "  {} {} → {} {}",
                        "[~]".yellow().bold(),
                        format!("Move '{from_name}'").yellow(),
                        to_name.yellow().bold(),
                        format!("(in {to_file})").dimmed(),
                    );
                } else {
                    println!(
                        "  {} {} → {}",
                        "[~]".yellow().bold(),
                        format!("Move '{from_name}'").yellow(),
                        to_name.yellow().bold(),
                    );
                }
                refactors += 1;
            }
            Atom::SemanticsPreserving { at, description } => {
                let kind = infer_node_kind(at);
                let name = at.last().map(String::as_str).unwrap_or("?");
                println!(
                    "  {} {} '{}': {}",
                    "[≈]".yellow().bold(),
                    format!("Refactor {kind}").yellow(),
                    name.yellow().bold(),
                    description.as_str().dimmed(),
                );
                refactors += 1;
            }
            _ => {}
        }
    }

    println!(
        "\n  {} +{} -{} ~{}\n",
        "∑".magenta(),
        insertions.to_string().green(),
        deletions.to_string().red(),
        refactors.to_string().yellow(),
    );
}

/// Infer a human-readable node kind label from a [`NodePath`] last segment.
///
/// Arc node-path segments use a `<kind>_<name>` convention
/// (e.g. `fn_parse`, `struct_Config`, `field_id`).  This function maps the
/// prefix to a plain-English label used in semantic diff output.  Falls back
/// to `"node"` for unrecognised prefixes.
fn infer_node_kind(path: &[String]) -> &'static str {
    let last = path.last().map(String::as_str).unwrap_or("");
    if last.starts_with("fn_") || last.starts_with("func_") {
        "function"
    } else if last.starts_with("struct_") {
        "struct"
    } else if last.starts_with("enum_") {
        "enum"
    } else if last.starts_with("impl_") {
        "impl block"
    } else if last.starts_with("trait_") {
        "trait"
    } else if last.starts_with("mod_") {
        "module"
    } else if last.starts_with("use_") || last.starts_with("import_") {
        "import"
    } else if last.starts_with("field_") {
        "field"
    } else if last.starts_with("method_") {
        "method"
    } else if last.starts_with("let_") || last.starts_with("var_") {
        "variable"
    } else if last.starts_with("type_") {
        "type alias"
    } else if last.starts_with("const_") {
        "constant"
    } else if last.starts_with("static_") {
        "static"
    } else if last.starts_with("macro_") {
        "macro"
    } else if last.starts_with("class_") {
        "class"
    } else if last.starts_with("interface_") {
        "interface"
    } else {
        "node"
    }
}

// ── Private helpers ───────────────────────────────────────────────────────────

/// Inject newlines at syntactic block boundaries before running the line differ.
///
/// This "Sesame" preprocessing forces structural alignment, preventing the
/// underlying Myers/Histogram algorithm from misaligning opening braces across
/// logical code blocks.
fn sesame_align(text: &str) -> String {
    text.replace(" {", "\n{")
        .replace("} ", "}\n")
        .replace("; ", ";\n")
}

/// Return `true` if this line counts as import/use boilerplate.
///
/// Blank and whitespace-only lines are treated as transparent so that
/// import blocks separated by blank spacer lines still collapse correctly.
fn is_import_line(line: &str) -> bool {
    let t = line.trim();
    t.is_empty()
        || t.starts_with("use ")
        || t.starts_with("import ")
        || t.starts_with("#include ")
        || t.starts_with("extern crate ")
}

/// Return `true` when both text blobs consist solely of import boilerplate,
/// signalling that the full textual diff can be collapsed into a one-line
/// summary.
fn is_pure_import_change(old_text: &str, new_text: &str) -> bool {
    old_text.lines().all(is_import_line) && new_text.lines().all(is_import_line)
}

/// Format non-file structural atoms as brief intent lines.
///
/// Handles [`Atom::Directory`], [`Atom::Blob`], and [`Atom::Mount`] using the
/// same colour conventions as `arc status`.  This is a safety net for atoms
/// that do not map to a specific text file — they are never fed to the line
/// differ but must still appear in the output.
fn format_atom_brief(atom: &Atom) -> String {
    match atom {
        Atom::Directory { path } => format!("++ dir {}", path.join("/")).green().to_string(),
        Atom::Blob { path, .. } => format!("~~ blob {}", path.join("/")).yellow().to_string(),
        Atom::Mount { path, url, .. } => format!("~~ mount {} @ {}", path.join("/"), url)
            .cyan()
            .to_string(),
        // File atoms are handled by group_and_render — this arm is a safety net.
        other => format!("{other:?}").dimmed().to_string(),
    }
}

// ── Tests ─────────────────────────────────────────────────────────────────────

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_sesame_align_injects_newlines() {
        let src = "fn foo() { bar(); }";
        let aligned = sesame_align(src);
        assert!(
            aligned.contains('\n'),
            "sesame_align should inject newlines at structural boundaries"
        );
        assert!(
            aligned.contains("\n{"),
            "opening brace should be on its own line"
        );
    }

    #[test]
    fn test_is_import_line_accepts_blank() {
        assert!(is_import_line(""), "empty line must pass");
        assert!(is_import_line("   "), "whitespace-only line must pass");
        assert!(is_import_line("use std::collections::HashMap;"));
        assert!(!is_import_line("fn main() {}"));
    }

    #[test]
    fn test_is_pure_import_change_collapses() {
        let old = "use std::fs;\nuse std::io;\n";
        let new = "use std::fs;\nuse std::path::Path;\n";
        assert!(is_pure_import_change(old, new));
    }

    #[test]
    fn test_is_pure_import_change_rejects_logic() {
        let old = "use std::fs;\n";
        let new = "use std::fs;\nfn main() {}\n";
        assert!(!is_pure_import_change(old, new));
    }

    #[test]
    fn test_is_pure_import_change_allows_blank_lines() {
        let old = "use std::fs;\n\nuse std::io;\n";
        let new = "use std::fs;\n\nuse std::path::Path;\n";
        assert!(
            is_pure_import_change(old, new),
            "blank lines inside import blocks must not block collapsing"
        );
    }

    // ── Semantic view helpers ─────────────────────────────────────────────

    #[test]
    fn test_infer_node_kind_recognises_prefixes() {
        assert_eq!(
            infer_node_kind(&["file".into(), "lib.rs".into(), "fn_parse".into()]),
            "function"
        );
        assert_eq!(
            infer_node_kind(&["file".into(), "lib.rs".into(), "struct_Config".into()]),
            "struct"
        );
        assert_eq!(
            infer_node_kind(&["file".into(), "lib.rs".into(), "enum_State".into()]),
            "enum"
        );
        assert_eq!(
            infer_node_kind(&["file".into(), "lib.rs".into(), "trait_Display".into()]),
            "trait"
        );
        assert_eq!(
            infer_node_kind(&["file".into(), "lib.rs".into(), "field_id".into()]),
            "field"
        );
        assert_eq!(
            infer_node_kind(&["file".into(), "lib.rs".into(), "const_MAX".into()]),
            "constant"
        );
    }

    #[test]
    fn test_infer_node_kind_fallback() {
        assert_eq!(
            infer_node_kind(&["file".into(), "lib.rs".into(), "unknown_xyz".into()]),
            "node"
        );
        assert_eq!(infer_node_kind(&[]), "node");
    }

    #[test]
    fn test_group_and_render_semantic_empty() {
        // Should not panic or error on an empty atom slice.
        group_and_render_semantic(&[]).expect("empty atoms must not error");
    }
}

use arc_algebra_types::Atom;
use arc_lang::ast::{LanguagePlugin, dispatch_plugin, fallback::TextFallbackPlugin};
use arc_store_cas::ObjectStore;

fn make_store() -> (tempfile::TempDir, ObjectStore) {
    let dir = tempfile::tempdir().unwrap();
    let store = ObjectStore::new(dir.path());
    (dir, store)
}

// ──────────────────────────────────────────────────────────────────────
// Fallback plugin (.txt, unknown extensions)
// ──────────────────────────────────────────────────────────────────────

#[test]
fn fallback_dispatch_for_unknown_extension() {
    let plugin = dispatch_plugin("README.md");
    assert_eq!(plugin.name(), "text");
}

#[test]
fn fallback_dispatch_for_no_extension() {
    let plugin = dispatch_plugin("Makefile");
    assert_eq!(plugin.name(), "text");
}

#[test]
fn fallback_diff_adds_line() {
    let plugin = TextFallbackPlugin::new();
    let (_dir, store) = make_store();
    let old = "line 1\nline 2";
    let new = "line 1\nline 2\nline 3";
    let atoms = plugin.diff(old, new, &store).unwrap();
    assert!(atoms.iter().any(|a| matches!(a, Atom::Insert { .. })));
    assert!(!atoms.iter().any(|a| matches!(a, Atom::Delete { .. })));
}

#[test]
fn fallback_diff_removes_line() {
    let plugin = TextFallbackPlugin::new();
    let (_dir, store) = make_store();
    let old = "line 1\nline 2\nline 3";
    let new = "line 1\nline 3";
    let atoms = plugin.diff(old, new, &store).unwrap();
    assert!(atoms.iter().any(|a| matches!(a, Atom::Delete { .. })));
}

#[test]
fn fallback_diff_identical() {
    let plugin = TextFallbackPlugin::new();
    let (_dir, store) = make_store();
    let src = "hello world";
    let atoms = plugin.diff(src, src, &store).unwrap();
    assert!(atoms.is_empty());
}

#[test]
fn fallback_unparse_roundtrip() {
    use std::collections::HashMap;
    let plugin = TextFallbackPlugin::new();
    let mut state: HashMap<arc_algebra_types::NodePath, Vec<u8>> = HashMap::new();
    state.insert(vec!["file".into(), "line".into(), "0".into()], b"alpha".to_vec());
    state.insert(vec!["file".into(), "line".into(), "1".into()], b"beta".to_vec());
    let result = plugin.unparse(&state, "notes.txt").unwrap();
    assert!(result.contains("alpha"));
    assert!(result.contains("beta"));
}

// ──────────────────────────────────────────────────────────────────────
// TypeScript plugin (.ts, .tsx)
// ──────────────────────────────────────────────────────────────────────

#[test]
fn ts_dispatch_for_ts() {
    assert_eq!(dispatch_plugin("src/index.ts").name(), "typescript");
}

#[test]
fn ts_dispatch_for_tsx() {
    assert_eq!(dispatch_plugin("src/App.tsx").name(), "typescript");
}

#[test]
fn ts_diff_adds_function() {
    let plugin = dispatch_plugin("app.ts");
    let (_dir, store) = make_store();
    let old = "function greet() { return 'hi'; }";
    let new = "function greet() { return 'hi'; }\nfunction farewell() { return 'bye'; }";
    let atoms = plugin.diff(old, new, &store).unwrap();
    assert!(atoms.iter().any(|a| matches!(a, Atom::Insert { .. })));
}

#[test]
fn ts_diff_removes_function() {
    let plugin = dispatch_plugin("app.ts");
    let (_dir, store) = make_store();
    let old = "function greet() { return 'hi'; }\nfunction farewell() { return 'bye'; }";
    let new = "function greet() { return 'hi'; }";
    let atoms = plugin.diff(old, new, &store).unwrap();
    assert!(atoms.iter().any(|a| matches!(a, Atom::Delete { .. })));
}

#[test]
fn ts_diff_identical() {
    let plugin = dispatch_plugin("app.ts");
    let (_dir, store) = make_store();
    let src = "const x: number = 42;";
    let atoms = plugin.diff(src, src, &store).unwrap();
    assert!(atoms.is_empty());
}

#[test]
fn ts_path_generation() {
    let plugin = dispatch_plugin("mod.ts");
    let src = "function main() { return 1; }";
    let tree = plugin.parse(src).unwrap();
    let fn_node = tree.root_node().child(0).expect("expected fn decl");
    let path = arc_lang::ast::generate_path(fn_node, src.as_bytes());
    assert!(path.iter().any(|s| s.contains("main")), "expected 'main' in path: {path:?}");
}

#[test]
fn ts_unparse_reconstructs() {
    let plugin = dispatch_plugin("app.ts");
    let src = "function main() { return foo(); }\nimport { foo } from 'bar';";
    let tree = plugin.parse(src).unwrap();
    let root = tree.root_node();
    let mut state = std::collections::HashMap::new();
    let mut cursor = root.walk();
    for child in root.named_children(&mut cursor) {
        let gen_path = arc_lang::ast::generate_path(child, src.as_bytes());
        // unparse expects ["file", <filepath>, ...] prefix
        let path = {
            let mut p: Vec<String> = vec!["file".into(), "app.ts".into()];
            p.extend(gen_path);
            p
        };
        let content = src.as_bytes()[child.start_byte()..child.end_byte()].to_vec();
        state.insert(path, content);
    }
    let result = plugin.unparse(&state, "app.ts").unwrap();
    assert!(!result.is_empty(), "unparse produced empty result");
}

// ──────────────────────────────────────────────────────────────────────
// JavaScript plugin (.js, .jsx)
// ──────────────────────────────────────────────────────────────────────

#[test]
fn js_dispatch_for_js() {
    assert_eq!(dispatch_plugin("src/index.js").name(), "javascript");
}

#[test]
fn js_dispatch_for_jsx() {
    assert_eq!(dispatch_plugin("src/App.jsx").name(), "javascript");
}

#[test]
fn js_diff_adds_function() {
    let plugin = dispatch_plugin("app.js");
    let (_dir, store) = make_store();
    let old = "function greet() { return 'hi'; }";
    let new = "function greet() { return 'hi'; }\nfunction farewell() { return 'bye'; }";
    let atoms = plugin.diff(old, new, &store).unwrap();
    assert!(atoms.iter().any(|a| matches!(a, Atom::Insert { .. })));
}

#[test]
fn js_diff_removes_function() {
    let plugin = dispatch_plugin("app.js");
    let (_dir, store) = make_store();
    let old = "function greet() { return 'hi'; }\nfunction farewell() { return 'bye'; }";
    let new = "function greet() { return 'hi'; }";
    let atoms = plugin.diff(old, new, &store).unwrap();
    assert!(atoms.iter().any(|a| matches!(a, Atom::Delete { .. })));
}

#[test]
fn js_diff_identical() {
    let plugin = dispatch_plugin("app.js");
    let (_dir, store) = make_store();
    let src = "const x = 42;";
    let atoms = plugin.diff(src, src, &store).unwrap();
    assert!(atoms.is_empty());
}

#[test]
fn js_path_generation() {
    let plugin = dispatch_plugin("mod.js");
    let src = "function main() { return 1; }";
    let tree = plugin.parse(src).unwrap();
    let fn_node = tree.root_node().child(0).expect("expected fn decl");
    let path = arc_lang::ast::generate_path(fn_node, src.as_bytes());
    assert!(path.iter().any(|s| s.contains("main")), "expected 'main' in path: {path:?}");
}

// ──────────────────────────────────────────────────────────────────────
// Rust plugin (.rs)
// ──────────────────────────────────────────────────────────────────────

#[test]
fn rs_dispatch_for_rs() {
    assert_eq!(dispatch_plugin("src/lib.rs").name(), "rust");
}

#[test]
fn rs_diff_adds_function() {
    let plugin = dispatch_plugin("mod.rs");
    let (_dir, store) = make_store();
    let old = "fn main() {}";
    let new = "fn main() {}\nfn helper() { let x = 1; }";
    let atoms = plugin.diff(old, new, &store).unwrap();
    assert!(atoms.iter().any(|a| matches!(a, Atom::Insert { .. })));
}

#[test]
fn rs_diff_removes_function() {
    let plugin = dispatch_plugin("mod.rs");
    let (_dir, store) = make_store();
    let old = "fn main() {}\nfn helper() { let x = 1; }";
    let new = "fn main() {}";
    let atoms = plugin.diff(old, new, &store).unwrap();
    assert!(atoms.iter().any(|a| matches!(a, Atom::Delete { .. })));
}

#[test]
fn rs_diff_identical() {
    let plugin = dispatch_plugin("mod.rs");
    let (_dir, store) = make_store();
    let src = "fn main() {}";
    let atoms = plugin.diff(src, src, &store).unwrap();
    assert!(atoms.is_empty());
}

#[test]
fn rs_path_generation() {
    let plugin = dispatch_plugin("mod.rs");
    let src = "fn main() {}";
    let tree = plugin.parse(src).unwrap();
    let fn_node = tree.root_node().child(0).expect("expected fn decl");
    let path = arc_lang::ast::generate_path(fn_node, src.as_bytes());
    assert!(path.iter().any(|s| s.contains("main")), "expected 'main' in path: {path:?}");
}

// ──────────────────────────────────────────────────────────────────────
// Python plugin (.py)
// ──────────────────────────────────────────────────────────────────────

#[test]
fn py_dispatch() {
    assert_eq!(dispatch_plugin("src/main.py").name(), "python");
}

#[test]
fn py_diff_adds_function() {
    let plugin = dispatch_plugin("app.py");
    let (_dir, store) = make_store();
    let old = "def greet():\n    return 'hi'";
    let new = "def greet():\n    return 'hi'\n\ndef farewell():\n    return 'bye'";
    let atoms = plugin.diff(old, new, &store).unwrap();
    assert!(atoms.iter().any(|a| matches!(a, Atom::Insert { .. })));
}

#[test]
fn py_diff_removes_function() {
    let plugin = dispatch_plugin("app.py");
    let (_dir, store) = make_store();
    let old = "def greet():\n    return 'hi'\n\ndef farewell():\n    return 'bye'";
    let new = "def greet():\n    return 'hi'";
    let atoms = plugin.diff(old, new, &store).unwrap();
    assert!(atoms.iter().any(|a| matches!(a, Atom::Delete { .. })));
}

#[test]
fn py_diff_identical() {
    let plugin = dispatch_plugin("app.py");
    let (_dir, store) = make_store();
    let src = "x = 42";
    let atoms = plugin.diff(src, src, &store).unwrap();
    assert!(atoms.is_empty());
}

#[test]
fn py_parse_and_path() {
    let plugin = dispatch_plugin("mod.py");
    let src = "def main():\n    pass";
    let tree = plugin.parse(src).unwrap();
    let root = tree.root_node();
    assert!(root.child_count() > 0, "parsed Python tree must have children");
}

// ──────────────────────────────────────────────────────────────────────
// Java plugin (.java)
// ──────────────────────────────────────────────────────────────────────

#[test]
fn java_dispatch() {
    assert_eq!(dispatch_plugin("src/Main.java").name(), "java");
}

#[test]
fn java_diff_adds_class() {
    let plugin = dispatch_plugin("App.java");
    let (_dir, store) = make_store();
    let old = "class Foo {}";
    let new = "class Foo {}\nclass Bar {}";
    let atoms = plugin.diff(old, new, &store).unwrap();
    assert!(atoms.iter().any(|a| matches!(a, Atom::Insert { .. })));
}

#[test]
fn java_diff_removes_class() {
    let plugin = dispatch_plugin("App.java");
    let (_dir, store) = make_store();
    let old = "class Foo {}\nclass Bar {}";
    let new = "class Foo {}";
    let atoms = plugin.diff(old, new, &store).unwrap();
    assert!(atoms.iter().any(|a| matches!(a, Atom::Delete { .. })));
}

#[test]
fn java_diff_identical() {
    let plugin = dispatch_plugin("App.java");
    let (_dir, store) = make_store();
    let src = "class Foo {}";
    let atoms = plugin.diff(src, src, &store).unwrap();
    assert!(atoms.is_empty());
}

// ──────────────────────────────────────────────────────────────────────
// C plugin (.c, .h)
// ──────────────────────────────────────────────────────────────────────

#[test]
fn c_dispatch_for_c() {
    assert_eq!(dispatch_plugin("src/main.c").name(), "c");
}

#[test]
fn c_dispatch_for_h() {
    assert_eq!(dispatch_plugin("include/foo.h").name(), "c");
}

#[test]
fn c_diff_adds_function() {
    let plugin = dispatch_plugin("main.c");
    let (_dir, store) = make_store();
    let old = "int main() { return 0; }";
    let new = "int main() { return 0; }\nint helper() { return 1; }";
    let atoms = plugin.diff(old, new, &store).unwrap();
    assert!(atoms.iter().any(|a| matches!(a, Atom::Insert { .. })));
}

#[test]
fn c_diff_removes_function() {
    let plugin = dispatch_plugin("main.c");
    let (_dir, store) = make_store();
    let old = "int main() { return 0; }\nint helper() { return 1; }";
    let new = "int main() { return 0; }";
    let atoms = plugin.diff(old, new, &store).unwrap();
    assert!(atoms.iter().any(|a| matches!(a, Atom::Delete { .. })));
}

#[test]
fn c_diff_identical() {
    let plugin = dispatch_plugin("main.c");
    let (_dir, store) = make_store();
    let src = "int main() { return 0; }";
    let atoms = plugin.diff(src, src, &store).unwrap();
    assert!(atoms.is_empty());
}

// ──────────────────────────────────────────────────────────────────────
// C++ plugin (.cpp, .cc, .cxx, .hpp)
// ──────────────────────────────────────────────────────────────────────

#[test]
fn cpp_dispatch_for_cpp() {
    assert_eq!(dispatch_plugin("src/main.cpp").name(), "cpp");
}

#[test]
fn cpp_dispatch_for_cc() {
    assert_eq!(dispatch_plugin("src/main.cc").name(), "cpp");
}

#[test]
fn cpp_dispatch_for_hpp() {
    assert_eq!(dispatch_plugin("include/foo.hpp").name(), "cpp");
}

#[test]
fn cpp_diff_adds_class() {
    let plugin = dispatch_plugin("app.cpp");
    let (_dir, store) = make_store();
    let old = "class Foo {};";
    let new = "class Foo {};\nclass Bar {};";
    let atoms = plugin.diff(old, new, &store).unwrap();
    assert!(atoms.iter().any(|a| matches!(a, Atom::Insert { .. })));
}

#[test]
fn cpp_diff_identical() {
    let plugin = dispatch_plugin("app.cpp");
    let (_dir, store) = make_store();
    let src = "class Foo {};";
    let atoms = plugin.diff(src, src, &store).unwrap();
    assert!(atoms.is_empty());
}

// ──────────────────────────────────────────────────────────────────────
// Go plugin (.go)
// ──────────────────────────────────────────────────────────────────────

#[test]
fn go_dispatch() {
    assert_eq!(dispatch_plugin("cmd/main.go").name(), "go");
}

#[test]
fn go_diff_adds_function() {
    let plugin = dispatch_plugin("main.go");
    let (_dir, store) = make_store();
    let old = "package main\n\nfunc main() {}";
    let new = "package main\n\nfunc main() {}\n\nfunc helper() {}";
    let atoms = plugin.diff(old, new, &store).unwrap();
    assert!(atoms.iter().any(|a| matches!(a, Atom::Insert { .. })));
}

#[test]
fn go_diff_removes_function() {
    let plugin = dispatch_plugin("main.go");
    let (_dir, store) = make_store();
    let old = "package main\n\nfunc main() {}\n\nfunc helper() {}";
    let new = "package main\n\nfunc main() {}";
    let atoms = plugin.diff(old, new, &store).unwrap();
    assert!(atoms.iter().any(|a| matches!(a, Atom::Delete { .. })));
}

#[test]
fn go_diff_identical() {
    let plugin = dispatch_plugin("main.go");
    let (_dir, store) = make_store();
    let src = "package main\n\nfunc main() {}";
    let atoms = plugin.diff(src, src, &store).unwrap();
    assert!(atoms.is_empty());
}

// ──────────────────────────────────────────────────────────────────────
// Ruby plugin (.rb)
// ──────────────────────────────────────────────────────────────────────

#[test]
fn rb_dispatch() {
    assert_eq!(dispatch_plugin("lib/app.rb").name(), "ruby");
}

#[test]
fn rb_diff_adds_method() {
    let plugin = dispatch_plugin("app.rb");
    let (_dir, store) = make_store();
    let old = "def greet\n  'hi'\nend";
    let new = "def greet\n  'hi'\nend\n\ndef farewell\n  'bye'\nend";
    let atoms = plugin.diff(old, new, &store).unwrap();
    assert!(atoms.iter().any(|a| matches!(a, Atom::Insert { .. })));
}

#[test]
fn rb_diff_removes_method() {
    let plugin = dispatch_plugin("app.rb");
    let (_dir, store) = make_store();
    let old = "def greet\n  'hi'\nend\n\ndef farewell\n  'bye'\nend";
    let new = "def greet\n  'hi'\nend";
    let atoms = plugin.diff(old, new, &store).unwrap();
    assert!(atoms.iter().any(|a| matches!(a, Atom::Delete { .. })));
}

#[test]
fn rb_diff_identical() {
    let plugin = dispatch_plugin("app.rb");
    let (_dir, store) = make_store();
    let src = "x = 42";
    let atoms = plugin.diff(src, src, &store).unwrap();
    assert!(atoms.is_empty());
}

// ──────────────────────────────────────────────────────────────────────
// PHP plugin (.php)
// ──────────────────────────────────────────────────────────────────────

#[test]
fn php_dispatch() {
    assert_eq!(dispatch_plugin("public/index.php").name(), "php");
}

#[test]
fn php_diff_adds_function() {
    let plugin = dispatch_plugin("app.php");
    let (_dir, store) = make_store();
    let old = "<?php\nfunction greet() { return 'hi'; }";
    let new = "<?php\nfunction greet() { return 'hi'; }\nfunction farewell() { return 'bye'; }";
    let atoms = plugin.diff(old, new, &store).unwrap();
    assert!(atoms.iter().any(|a| matches!(a, Atom::Insert { .. })));
}

#[test]
fn php_diff_removes_function() {
    let plugin = dispatch_plugin("app.php");
    let (_dir, store) = make_store();
    let old = "<?php\nfunction greet() { return 'hi'; }\nfunction farewell() { return 'bye'; }";
    let new = "<?php\nfunction greet() { return 'hi'; }";
    let atoms = plugin.diff(old, new, &store).unwrap();
    assert!(atoms.iter().any(|a| matches!(a, Atom::Delete { .. })));
}

#[test]
fn php_diff_identical() {
    let plugin = dispatch_plugin("app.php");
    let (_dir, store) = make_store();
    let src = "<?php\n$x = 42;";
    let atoms = plugin.diff(src, src, &store).unwrap();
    assert!(atoms.is_empty());
}

// ──────────────────────────────────────────────────────────────────────
// C# plugin (.cs)
// ──────────────────────────────────────────────────────────────────────

#[test]
fn cs_dispatch() {
    assert_eq!(dispatch_plugin("src/Program.cs").name(), "csharp");
}

#[test]
fn cs_diff_adds_class() {
    let plugin = dispatch_plugin("App.cs");
    let (_dir, store) = make_store();
    let old = "class Foo {}";
    let new = "class Foo {}\nclass Bar {}";
    let atoms = plugin.diff(old, new, &store).unwrap();
    assert!(atoms.iter().any(|a| matches!(a, Atom::Insert { .. })));
}

#[test]
fn cs_diff_removes_class() {
    let plugin = dispatch_plugin("App.cs");
    let (_dir, store) = make_store();
    let old = "class Foo {}\nclass Bar {}";
    let new = "class Foo {}";
    let atoms = plugin.diff(old, new, &store).unwrap();
    assert!(atoms.iter().any(|a| matches!(a, Atom::Delete { .. })));
}

#[test]
fn cs_diff_identical() {
    let plugin = dispatch_plugin("App.cs");
    let (_dir, store) = make_store();
    let src = "class Foo {}";
    let atoms = plugin.diff(src, src, &store).unwrap();
    assert!(atoms.is_empty());
}

// ──────────────────────────────────────────────────────────────────────
// Bash plugin (.sh, .bash)
// ──────────────────────────────────────────────────────────────────────

#[test]
fn bash_dispatch_for_sh() {
    assert_eq!(dispatch_plugin("scripts/build.sh").name(), "bash");
}

#[test]
fn bash_dispatch_for_bash() {
    assert_eq!(dispatch_plugin("scripts/test.bash").name(), "bash");
}

#[test]
fn bash_diff_adds_command() {
    let plugin = dispatch_plugin("run.sh");
    let (_dir, store) = make_store();
    let old = "#!/bin/bash\necho hello";
    let new = "#!/bin/bash\necho hello\necho world";
    let atoms = plugin.diff(old, new, &store).unwrap();
    assert!(atoms.iter().any(|a| matches!(a, Atom::Insert { .. })));
}

#[test]
fn bash_diff_identical() {
    let plugin = dispatch_plugin("run.sh");
    let (_dir, store) = make_store();
    let src = "#!/bin/bash\necho hello";
    let atoms = plugin.diff(src, src, &store).unwrap();
    assert!(atoms.is_empty());
}

// ──────────────────────────────────────────────────────────────────────
// JSON plugin (.json)
// ──────────────────────────────────────────────────────────────────────

#[test]
fn json_dispatch() {
    assert_eq!(dispatch_plugin("config/settings.json").name(), "json");
}

#[test]
fn json_diff_adds_key() {
    let plugin = dispatch_plugin("data.json");
    let (_dir, store) = make_store();
    let old = r#"{"a": 1}"#;
    let new = r#"{"a": 1, "b": 2}"#;
    let atoms = plugin.diff(old, new, &store).unwrap();
    assert!(atoms.iter().any(|a| matches!(a, Atom::Insert { .. })));
}

#[test]
fn json_diff_removes_key() {
    let plugin = dispatch_plugin("data.json");
    let (_dir, store) = make_store();
    let old = r#"{"a": 1, "b": 2}"#;
    let new = r#"{"a": 1}"#;
    let atoms = plugin.diff(old, new, &store).unwrap();
    assert!(atoms.iter().any(|a| matches!(a, Atom::Delete { .. })));
}

#[test]
fn json_diff_identical() {
    let plugin = dispatch_plugin("data.json");
    let (_dir, store) = make_store();
    let src = r#"{"x": 42}"#;
    let atoms = plugin.diff(src, src, &store).unwrap();
    assert!(atoms.is_empty());
}

#[test]
fn json_parse_and_roundtrip() {
    let plugin = dispatch_plugin("config.json");
    let src = r#"{"name": "test", "version": "1.0"}"#;
    let tree = plugin.parse(src).unwrap();
    let root = tree.root_node();
    assert_eq!(root.kind(), "document");
    assert!(root.child_count() >= 1);
}

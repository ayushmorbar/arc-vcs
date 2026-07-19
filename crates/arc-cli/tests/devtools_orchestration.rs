use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex, OnceLock};

use arc_cli::devtools::interrupt::InterruptState;
use arc_cli::devtools::multicall::normalize_invocation_args;
use arc_cli::devtools::run_wrapper::run_with_telemetry;
use arc_testtools::{EnvGuard, FixtureMode, FixtureOptions, FixtureOrchestrator};

fn env_lock() -> &'static Mutex<()> {
    static LOCK: OnceLock<Mutex<()>> = OnceLock::new();
    LOCK.get_or_init(|| Mutex::new(()))
}

#[test]
fn multicall_normalization_injects_known_mode() {
    let args = vec!["arc-sync".to_string(), "--help".to_string()];
    let normalized = normalize_invocation_args(args);
    assert_eq!(normalized, vec!["arc".to_string(), "sync".to_string(), "--help".to_string()]);
}

#[test]
fn run_wrapper_executes_closure_and_returns_ok() {
    let interrupts = InterruptState::new();
    let seen = Arc::new(AtomicBool::new(false));
    let seen_closure = Arc::clone(&seen);

    let result = run_with_telemetry("test", &interrupts, move || {
        seen_closure.store(true, Ordering::SeqCst);
        Ok(())
    });

    assert!(result.is_ok());
    assert!(seen.load(Ordering::SeqCst));
}

#[test]
fn env_guard_restores_variable_value() {
    let _guard = env_lock().lock().expect("lock env");
    const KEY: &str = "ARC_CLI_TESTTOOLS_ENV_GUARD";
    // SAFETY: test-only; single-threaded test with env_lock() guard.
    unsafe {
        std::env::set_var(KEY, "base");
    }

    {
        let _override = EnvGuard::set(KEY, "override");
        assert_eq!(std::env::var(KEY).ok().as_deref(), Some("override"));
    }

    assert_eq!(std::env::var(KEY).ok().as_deref(), Some("base"));
    // SAFETY: test-only; single-threaded test with env_lock() guard.
    unsafe {
        std::env::remove_var(KEY);
    }
}

#[test]
fn fixture_orchestrator_produces_writable_copy() {
    let cache_root = tempfile::tempdir().expect("cache root");
    let source = tempfile::tempdir().expect("fixture source");
    std::fs::write(source.path().join("payload.txt"), "source").expect("write source fixture");

    let orchestrator = FixtureOrchestrator::new(cache_root.path().to_path_buf());
    let options = FixtureOptions::new("sample").with_mode(FixtureMode::WritableCopy);
    let writable_path =
        orchestrator.materialize(source.path(), &options).expect("materialize writable fixture");

    std::fs::write(writable_path.join("payload.txt"), "mutated").expect("write mutable copy");
    let source_contents =
        std::fs::read_to_string(source.path().join("payload.txt")).expect("read source");
    assert_eq!(source_contents, "source");
}

/// Invocation mode selected by executable stem.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum InvocationMode {
    /// Normal `arc` command execution.
    Arc,
    /// Delegate to daemon subprocess command.
    Daemon,
    /// Prefix invocation with `sync` command.
    Sync,
}

/// Infer invocation mode from an executable path.
#[must_use]
pub fn mode_from_executable(executable: &str) -> InvocationMode {
    let mut stem = std::path::Path::new(executable)
        .file_stem()
        .map(|value| value.to_string_lossy().to_string())
        .unwrap_or_else(|| executable.to_string());
    stem.make_ascii_lowercase();

    match stem.as_str() {
        "arc-daemon" => InvocationMode::Daemon,
        "arc-sync" => InvocationMode::Sync,
        _ => InvocationMode::Arc,
    }
}

/// Normalize raw CLI args based on multicall executable naming.
#[must_use]
pub fn normalize_invocation_args(mut raw_args: Vec<String>) -> Vec<String> {
    if raw_args.is_empty() {
        return vec!["arc".to_string()];
    }

    let mode = mode_from_executable(&raw_args[0]);
    if mode == InvocationMode::Arc {
        return raw_args;
    }

    raw_args[0] = "arc".to_string();
    if raw_args.get(1).is_some_and(|arg| !arg.starts_with('-')) {
        return raw_args;
    }

    let injected = match mode {
        InvocationMode::Daemon => "daemon",
        InvocationMode::Sync => "sync",
        InvocationMode::Arc => return raw_args,
    };
    raw_args.insert(1, injected.to_string());
    raw_args
}

#[cfg(test)]
mod tests {
    use super::{InvocationMode, mode_from_executable, normalize_invocation_args};

    #[test]
    fn dispatches_daemon_mode_from_stem() {
        assert_eq!(mode_from_executable("arc-daemon"), InvocationMode::Daemon);
    }

    #[cfg(target_os = "windows")]
    #[test]
    fn dispatches_sync_mode_from_windows_exe_stem() {
        assert_eq!(mode_from_executable(r"C:\\bin\\arc-sync.exe"), InvocationMode::Sync);
    }

    #[test]
    fn normalizes_arc_daemon_invocation() {
        let args = vec!["arc-daemon".to_string(), "--help".to_string()];
        let normalized = normalize_invocation_args(args);
        assert_eq!(normalized, vec!["arc".to_string(), "daemon".to_string(), "--help".to_string()]);
    }

    #[test]
    fn preserves_explicit_subcommand() {
        let args = vec!["arc-daemon".to_string(), "status".to_string()];
        let normalized = normalize_invocation_args(args);
        assert_eq!(normalized, vec!["arc".to_string(), "status".to_string()]);
    }

    #[test]
    fn dispatches_arc_mode_from_plain_name() {
        assert_eq!(mode_from_executable("arc"), InvocationMode::Arc);
    }

    #[test]
    fn dispatches_arc_mode_from_unknown_name() {
        assert_eq!(mode_from_executable("something-else"), InvocationMode::Arc);
    }

    #[test]
    fn mode_from_executable_strips_path_prefix() {
        assert_eq!(mode_from_executable("/usr/bin/arc-daemon"), InvocationMode::Daemon);
    }

    #[test]
    fn normalize_empty_args_returns_arc() {
        let normalized = normalize_invocation_args(Vec::new());
        assert_eq!(normalized, vec!["arc".to_string()]);
    }

    #[test]
    fn normalize_arc_mode_passes_through() {
        let args = vec!["arc".to_string(), "status".to_string()];
        let normalized = normalize_invocation_args(args.clone());
        assert_eq!(normalized, args);
    }

    #[test]
    fn normalize_sync_injects_sync_subcommand() {
        let args = vec!["arc-sync".to_string(), "--help".to_string()];
        let normalized = normalize_invocation_args(args);
        assert_eq!(normalized, vec!["arc".to_string(), "sync".to_string(), "--help".to_string()]);
    }
}

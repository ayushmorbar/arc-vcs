/// Rendering mode selected for the current execution context.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum RenderMode {
    HumanRich,
    HumanPlain,
    MachineJson,
}

/// Terminal capabilities used by the output layer.
#[derive(Debug, Clone)]
pub struct TerminalCapabilities {
    pub mode: RenderMode,
    pub supports_osc8: bool,
    pub supports_unicode: bool,
}

/// Auto-detect output capabilities based on environment and TTY state.
pub fn detect_capabilities(force_json: bool, quiet: bool) -> TerminalCapabilities {
    let is_ci = std::env::var("CI").is_ok();
    let no_color = std::env::var("NO_COLOR").is_ok();
    let term = std::env::var("TERM").unwrap_or_default();
    let dumb_term = term.eq_ignore_ascii_case("dumb");
    let is_tty = atty::is(atty::Stream::Stdout);
    let clicolor = std::env::var("CLICOLOR").unwrap_or_else(|_| "1".to_string());
    let color_disabled = clicolor == "0";

    let mode = if force_json {
        RenderMode::MachineJson
    } else if quiet || is_ci || !is_tty || no_color || dumb_term || color_disabled {
        RenderMode::HumanPlain
    } else {
        RenderMode::HumanRich
    };

    let term_program = std::env::var("TERM_PROGRAM").unwrap_or_default().to_lowercase();
    let supports_osc8 = is_tty
        && !dumb_term
        && (term_program.contains("vscode")
            || term_program.contains("wezterm")
            || term_program.contains("iterm")
            || term.contains("xterm")
            || term.contains("kitty"));

    TerminalCapabilities {
        mode,
        supports_osc8,
        supports_unicode: !dumb_term,
    }
}
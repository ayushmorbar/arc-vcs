use std::{error::Error as StdError, fmt};

use arc_error::Error as ArcError;
use miette::{Diagnostic, Report};

/// Convert arc_error::Error into a rich miette report.
pub fn arc_error_to_report(error: ArcError) -> Report {
    let msg = error.to_string();
    let code_num = stable_error_code(&msg);
    let code = format_error_code(code_num);
    let url = format!("https://arc-vcs.dev/errors/{code}");
    let chain = build_arrow_chain(&error);
    let help = "Inspect the cause chain below and retry with --json for machine-readable output"
        .to_string();

    Report::new(ArcMietteDiagnostic { message: msg, arrow_chain: chain, help, code, url })
}

/// Format code number as E0001-E1000.
pub fn format_error_code(code: u16) -> String {
    format!("E{:04}", code.clamp(1, 1000))
}

fn stable_error_code(message: &str) -> u16 {
    let digest = blake3::hash(message.as_bytes());
    let bytes = digest.as_bytes();
    let n = u16::from_le_bytes([bytes[0], bytes[1]]);
    (n % 1000) + 1
}

fn build_arrow_chain(error: &dyn StdError) -> String {
    let mut chain = Vec::new();
    let mut current = error.source();
    while let Some(source) = current {
        chain.push(source.to_string());
        current = source.source();
    }
    if chain.is_empty() { "no additional causes".to_string() } else { chain.join("\n  ╰─▶ ") }
}

#[derive(Debug)]
struct ArcMietteDiagnostic {
    message: String,
    arrow_chain: String,
    help: String,
    code: String,
    url: String,
}

impl fmt::Display for ArcMietteDiagnostic {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "{}\n  ╰─▶ {}", self.message, self.arrow_chain)
    }
}

impl StdError for ArcMietteDiagnostic {}

impl Diagnostic for ArcMietteDiagnostic {
    fn code<'a>(&'a self) -> Option<Box<dyn fmt::Display + 'a>> {
        Some(Box::new(self.code.clone()))
    }

    fn help<'a>(&'a self) -> Option<Box<dyn fmt::Display + 'a>> {
        Some(Box::new(self.help.clone()))
    }

    fn url<'a>(&'a self) -> Option<Box<dyn fmt::Display + 'a>> {
        Some(Box::new(self.url.clone()))
    }
}

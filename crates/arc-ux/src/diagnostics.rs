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

#[cfg(test)]
mod tests {
    use std::error::Error as StdError;

    use super::*;

    #[test]
    fn format_error_code_clamps_to_valid_range() {
        assert_eq!(format_error_code(0), "E0001");
        assert_eq!(format_error_code(1), "E0001");
        assert_eq!(format_error_code(500), "E0500");
        assert_eq!(format_error_code(1000), "E1000");
        assert_eq!(format_error_code(1001), "E1000");
        assert_eq!(format_error_code(65535), "E1000");
    }

    #[test]
    fn stable_error_code_is_deterministic() {
        let msg = "test error message";
        let code1 = stable_error_code(msg);
        let code2 = stable_error_code(msg);
        assert_eq!(code1, code2);
        assert!((1..=1000).contains(&code1));
    }

    #[test]
    fn stable_error_code_differs_for_different_messages() {
        let code_a = stable_error_code("message alpha");
        let code_b = stable_error_code("message beta");
        assert_ne!(code_a, code_b);
    }

    #[test]
    fn build_arrow_chain_empty_when_no_source() {
        use std::fmt;

        #[derive(Debug)]
        struct LeafError;

        impl fmt::Display for LeafError {
            fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
                write!(f, "leaf")
            }
        }
        impl StdError for LeafError {}

        let err = LeafError;
        assert_eq!(build_arrow_chain(&err), "no additional causes");
    }

    #[test]
    fn build_arrow_chain_joins_multiple_sources() {
        use std::fmt;

        #[derive(Debug)]
        struct Inner;

        impl fmt::Display for Inner {
            fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
                write!(f, "inner")
            }
        }
        impl StdError for Inner {}

        #[derive(Debug)]
        struct Outer(Inner);

        impl fmt::Display for Outer {
            fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
                write!(f, "outer")
            }
        }
        impl StdError for Outer {
            fn source(&self) -> Option<&(dyn StdError + 'static)> {
                Some(&self.0)
            }
        }

        let err = Outer(Inner);
        let chain = build_arrow_chain(&err);
        assert!(chain.contains("inner"), "chain should include inner error");
    }

    #[test]
    fn arc_miette_diagnostic_display_includes_message_and_arrow_chain() {
        let diag = ArcMietteDiagnostic {
            message: "something broke".to_string(),
            arrow_chain: "root cause".to_string(),
            help: "try again".to_string(),
            code: "E0001".to_string(),
            url: "https://arc-vcs.dev/errors/E0001".to_string(),
        };
        let display = format!("{diag}");
        assert!(display.contains("something broke"));
        assert!(display.contains("root cause"));
    }

    #[test]
    fn arc_miette_diagnostic_code_returns_formatted_code() {
        let diag = ArcMietteDiagnostic {
            message: "msg".to_string(),
            arrow_chain: "no additional causes".to_string(),
            help: "help".to_string(),
            code: "E0042".to_string(),
            url: "https://arc-vcs.dev/errors/E0042".to_string(),
        };
        let code = diag.code().expect("code must be present");
        assert_eq!(format!("{code}"), "E0042");
    }

    #[test]
    fn arc_miette_diagnostic_help_returns_help_text() {
        let diag = ArcMietteDiagnostic {
            message: "msg".to_string(),
            arrow_chain: "no additional causes".to_string(),
            help: "inspect the cause chain".to_string(),
            code: "E0001".to_string(),
            url: "https://arc-vcs.dev/errors/E0001".to_string(),
        };
        let help = diag.help().expect("help must be present");
        assert_eq!(format!("{help}"), "inspect the cause chain");
    }

    #[test]
    fn arc_miette_diagnostic_url_returns_url() {
        let diag = ArcMietteDiagnostic {
            message: "msg".to_string(),
            arrow_chain: "no additional causes".to_string(),
            help: "help".to_string(),
            code: "E0001".to_string(),
            url: "https://arc-vcs.dev/errors/E0001".to_string(),
        };
        let url = diag.url().expect("url must be present");
        assert!(format!("{url}").starts_with("https://"));
    }

    #[test]
    fn arc_error_to_report_produces_valid_report() {
        let err = ArcError::from_error(arc_error::message("test failure"));
        let report = arc_error_to_report(err);
        let report_str = format!("{report}");
        assert!(!report_str.is_empty());
    }
}

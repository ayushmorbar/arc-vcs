use std::io::Write as _;

use console::Style;
use indicatif::{ProgressBar, ProgressStyle};
use serde::Serialize;

use crate::event::OutputEvent;

/// Event renderer abstraction for CLI/TUI front-ends.
pub trait Renderer {
    fn render(&mut self, event: OutputEvent);
}

/// Rich, icon/color renderer with replace-on-done progress behavior.
pub struct HumanRichRenderer {
    spinner: Option<ProgressBar>,
    supports_osc8: bool,
}

impl HumanRichRenderer {
    pub fn new(supports_osc8: bool) -> Self {
        Self {
            spinner: None,
            supports_osc8,
        }
    }

    fn stop_spinner(&mut self) {
        if let Some(pb) = self.spinner.take() {
            pb.finish_and_clear();
        }
    }
}

impl Renderer for HumanRichRenderer {
    fn render(&mut self, event: OutputEvent) {
        let cyan = Style::new().cyan().bold();
        let green = Style::new().green().bold();
        let yellow = Style::new().yellow().bold();
        let red = Style::new().red().bold();
        let dim = Style::new().dim();

        match event {
            OutputEvent::Started(op) => {
                let pb = ProgressBar::new_spinner();
                pb.set_style(
                    ProgressStyle::with_template("{spinner} {msg}")
                        .expect("spinner template must be valid"),
                );
                pb.enable_steady_tick(std::time::Duration::from_millis(80));
                pb.set_message(format!("{} {}", dim.apply_to("●"), dim.apply_to(op)));
                self.spinner = Some(pb);
            }
            OutputEvent::Progress(current, total, message) => {
                let pb = self.spinner.get_or_insert_with(|| ProgressBar::new(total));
                pb.set_length(total);
                pb.set_position(current);
                pb.set_message(message);
            }
            OutputEvent::Success(summary, details) => {
                self.stop_spinner();
                println!("{} {}", green.apply_to("✔"), green.apply_to(summary));
                for line in details {
                    println!("  {}", dim.apply_to(line));
                }
            }
            OutputEvent::Warning(message) => {
                self.stop_spinner();
                println!("{} {}", yellow.apply_to("⚠"), yellow.apply_to(message));
            }
            OutputEvent::Diagnostic(report) => {
                self.stop_spinner();
                eprintln!("{} {}", red.apply_to("✖"), report);
                let _ = cyan;
            }
        }

        let _ = self.supports_osc8;
    }
}

/// Plain renderer for non-interactive, CI, and NO_COLOR contexts.
pub struct HumanPlainRenderer;

impl Renderer for HumanPlainRenderer {
    fn render(&mut self, event: OutputEvent) {
        match event {
            OutputEvent::Started(op) => println!("[pending] {op}"),
            OutputEvent::Progress(current, total, message) => {
                println!("[progress] {current}/{total} {message}")
            }
            OutputEvent::Success(summary, details) => {
                println!("[ok] {summary}");
                for line in details {
                    println!("  {line}");
                }
            }
            OutputEvent::Warning(message) => println!("[warn] {message}"),
            OutputEvent::Diagnostic(report) => eprintln!("[err] {report}"),
        }
    }
}

/// JSON/NDJSON renderer for machine consumers.
pub struct JsonRenderer {
    pub ndjson: bool,
}

#[derive(Serialize)]
struct JsonEnvelope {
    version: u8,
    event: &'static str,
    payload: serde_json::Value,
}

impl Renderer for JsonRenderer {
    fn render(&mut self, event: OutputEvent) {
        let (name, payload) = match event {
            OutputEvent::Started(op) => (
                "started",
                serde_json::json!({
                    "operation": op
                }),
            ),
            OutputEvent::Progress(current, total, message) => (
                "progress",
                serde_json::json!({
                    "current": current,
                    "total": total,
                    "message": message
                }),
            ),
            OutputEvent::Success(summary, details) => (
                "success",
                serde_json::json!({
                    "summary": summary,
                    "details": details
                }),
            ),
            OutputEvent::Warning(message) => (
                "warning",
                serde_json::json!({
                    "message": message
                }),
            ),
            OutputEvent::Diagnostic(report) => (
                "diagnostic",
                serde_json::json!({
                    "message": report.to_string()
                }),
            ),
        };

        let envelope = JsonEnvelope {
            version: 1,
            event: name,
            payload,
        };

        if self.ndjson {
            let line = serde_json::to_string(&envelope).expect("json serialization must succeed");
            println!("{line}");
        } else {
            let text = serde_json::to_string_pretty(&envelope)
                .expect("pretty json serialization must succeed");
            println!("{text}");
        }
        let _ = std::io::stdout().flush();
    }
}

/// Build an OSC8 hyperlink for a BLAKE3 hash.
pub fn hyperlink_for_hash(hash: &str, supports_osc8: bool) -> String {
    if !supports_osc8 {
        return hash.to_string();
    }
    let target = format!("arc://diff/{hash}");
    format!("\u{1b}]8;;{target}\u{1b}\\{hash}\u{1b}]8;;\u{1b}\\")
}

/// Build an OSC8 hyperlink for a file path + line.
pub fn hyperlink_for_path(path: &str, line: usize, supports_osc8: bool) -> String {
    if !supports_osc8 {
        return format!("{path}:{line}");
    }

    let normalized = std::fs::canonicalize(path)
        .ok()
        .and_then(|p| p.to_str().map(ToOwned::to_owned))
        .unwrap_or_else(|| path.to_string());
    let encoded = percent_encode_uri_path(&normalized);

    let editor = std::env::var("EDITOR").unwrap_or_else(|_| "editor".to_string());
    let target = if editor.contains("code") {
        format!("vscode://file/{encoded}:{line}")
    } else {
        format!("file://{encoded}#L{line}")
    };
    let label = format!("{path}:{line}");
    format!("\u{1b}]8;;{target}\u{1b}\\{label}\u{1b}]8;;\u{1b}\\")
}

fn percent_encode_uri_path(value: &str) -> String {
    let mut out = String::with_capacity(value.len());
    for &b in value.as_bytes() {
        let is_unreserved = b.is_ascii_alphanumeric() || matches!(b, b'-' | b'_' | b'.' | b'~' | b'/' | b':');
        if is_unreserved {
            out.push(char::from(b));
        } else {
            out.push('%');
            out.push_str(&format!("{b:02X}"));
        }
    }
    out
}
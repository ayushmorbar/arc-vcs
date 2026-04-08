#![forbid(unsafe_code)]

pub mod capabilities;
pub mod diagnostics;
pub mod event;
pub mod renderer;

pub use capabilities::{RenderMode, TerminalCapabilities, detect_capabilities};
pub use diagnostics::{arc_error_to_report, format_error_code};
pub use event::OutputEvent;
pub use renderer::{
    HumanPlainRenderer, HumanRichRenderer, JsonRenderer, Renderer, hyperlink_for_hash,
    hyperlink_for_path,
};
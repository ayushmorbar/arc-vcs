#![forbid(unsafe_code)]

pub mod app;
pub mod bridge;
pub mod components;
pub mod diff;
pub mod layout;
pub mod model;
pub mod provider;

pub use app::App;
pub use bridge::BackendBridge;
pub use model::{AppState, ChangeEntry, Message};
pub use provider::MockProvider;

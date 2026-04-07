use std::path::{Path, PathBuf};
use std::time::Duration;

use anyhow::Context as _;
use notify::{Config, Event, RecommendedWatcher, RecursiveMode, Watcher};
use tokio::sync::mpsc;
use tokio::time::timeout;

/// Background autosnapshot daemon.
pub struct AutoSnapDaemon;

impl AutoSnapDaemon {
    /// Start recursive watch + debounced autosnapshot loop.
    pub async fn start(path: PathBuf, debounce_ms: u64) -> anyhow::Result<()> {
        let debounce = Duration::from_millis(debounce_ms.max(1));
        let (_watcher, mut rx) = start_watcher(&path)?;

        tracing::info!(
            path = %path.display(),
            debounce_ms,
            "[arc-watch] watcher started"
        );

        while rx.recv().await.is_some() {
            loop {
                match timeout(debounce, rx.recv()).await {
                    Ok(Some(_)) => {
                        // Another event arrived inside the debounce window; reset timer.
                        continue;
                    }
                    Ok(None) => return Ok(()),
                    Err(_) => {
                        tracing::info!(
                            "[arc-watch] Debounce fired. Triggering auto-snapshot for changes..."
                        );
                        break;
                    }
                }
            }
        }

        Ok(())
    }
}

fn start_watcher(path: &Path) -> anyhow::Result<(RecommendedWatcher, mpsc::Receiver<Event>)> {
    let (tx, rx) = mpsc::channel::<Event>(1024);
    let callback_tx = tx.clone();

    let mut watcher = RecommendedWatcher::new(
        move |event: notify::Result<Event>| match event {
            Ok(event) => {
                // If the channel is saturated, dropping event bursts is fine:
                // debounce only needs a signal that "something changed".
                let _ = callback_tx.try_send(event);
            }
            Err(err) => {
                tracing::warn!(error = %err, "[arc-watch] filesystem watcher error");
            }
        },
        Config::default(),
    )
    .context("failed to create notify watcher")?;

    watcher
        .watch(path, RecursiveMode::Recursive)
        .with_context(|| format!("failed to watch {}", path.display()))?;

    Ok((watcher, rx))
}

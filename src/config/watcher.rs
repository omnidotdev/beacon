//! Config file watcher
//!
//! Watch the config TOML file for changes and send reload signals over a channel

use std::path::{Path, PathBuf};

use notify::{RecommendedWatcher, RecursiveMode, Watcher, event::ModifyKind};
use tokio::sync::mpsc;

/// Signal emitted when the config file changes on disk
#[derive(Debug, Clone)]
pub struct ReloadSignal {
    /// Path that triggered the reload
    pub path: PathBuf,
}

/// Start watching a config file for changes
///
/// Watches the parent directory of `config_path` for modify/create events
/// targeting the config file. Sends `ReloadSignal` on the returned channel.
///
/// # Errors
///
/// Returns error if the watcher cannot be created or the path cannot be watched.
pub fn watch_config(
    config_path: &Path,
) -> crate::Result<(RecommendedWatcher, mpsc::Receiver<ReloadSignal>)> {
    let (tx, rx) = mpsc::channel::<ReloadSignal>(16);
    let target = config_path.to_path_buf();

    let mut watcher =
        notify::recommended_watcher(move |res: Result<notify::Event, notify::Error>| {
            let Ok(event) = res else {
                return;
            };

            let dominated = matches!(
                event.kind,
                notify::EventKind::Modify(ModifyKind::Data(_) | ModifyKind::Any)
                    | notify::EventKind::Create(_)
            );

            if !dominated {
                return;
            }

            // Only signal when the event targets our config file
            let matches_target = event.paths.iter().any(|p| p == &target);
            if !matches_target {
                return;
            }

            let _ = tx.blocking_send(ReloadSignal {
                path: target.clone(),
            });
        })
        .map_err(|e| crate::Error::Config(format!("failed to create config watcher: {e}")))?;

    let parent = config_path
        .parent()
        .ok_or_else(|| crate::Error::Config("config path has no parent directory".to_string()))?;

    watcher
        .watch(parent, RecursiveMode::NonRecursive)
        .map_err(|e| crate::Error::Config(format!("failed to watch config directory: {e}")))?;

    tracing::info!(path = %config_path.display(), "config file watcher started");

    Ok((watcher, rx))
}

#[cfg(test)]
mod tests {
    use std::time::Duration;

    use super::*;

    #[tokio::test]
    async fn watch_detects_file_modification() {
        let dir = tempfile::tempdir().unwrap();
        let config_path = dir.path().join("config.toml");
        std::fs::write(&config_path, "[persona]\nname = \"orin\"").unwrap();

        let (_watcher, mut rx) = watch_config(&config_path).unwrap();

        // Small delay to let the watcher register
        tokio::time::sleep(Duration::from_millis(100)).await;

        // Modify the file
        std::fs::write(&config_path, "[persona]\nname = \"beacon\"").unwrap();

        // Should receive a signal within 2 seconds
        let signal = tokio::time::timeout(Duration::from_secs(2), rx.recv())
            .await
            .expect("timed out waiting for reload signal")
            .expect("channel closed without signal");

        assert_eq!(signal.path, config_path);
    }
}

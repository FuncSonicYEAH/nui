//! nui-preview: hot-reload previewer for `.nui` files (plan §8).
//!
//! Watches the document with notify; every saved change recompiles and
//! rebuilds the element tree (v1 full rebuild, state is lost). A document
//! that fails to compile keeps the last good frame on screen, reports the
//! rendered rustc-style diagnostics to stderr, and marks the window title.
//!
//! Run: `cargo run -p nui-preview -- path/to/app.nui [--width 420 --height 300]`
#![allow(clippy::unwrap_used)]

use std::path::PathBuf;
use std::sync::mpsc::{Receiver, channel};

use notify::Watcher;
use nui::{AppConfig, DocumentWatcher, ReloadWaker};
use nui_core::Size;

/// Watched-file source: notify signals a background channel and wakes the
/// event loop; `poll_reload` drains the notifications and returns the file
/// content when it actually changed (dedupes multi-event saves and
/// transient partial writes — the next save always differs again).
struct FileWatcher {
    path: PathBuf,
    last_seen: Option<String>,
    receiver: Receiver<()>,
    /// Kept alive for the watcher's lifetime; armed in `attach` when the
    /// event-loop waker becomes available.
    _watcher: Option<notify::RecommendedWatcher>,
}

impl FileWatcher {
    fn new(path: PathBuf) -> FileWatcher {
        return FileWatcher::with_channel(path, channel().1);
    }

    fn with_channel(path: PathBuf, receiver: Receiver<()>) -> FileWatcher {
        return FileWatcher {
            path,
            last_seen: None,
            receiver,
            _watcher: None,
        };
    }
}

impl DocumentWatcher for FileWatcher {
    fn attach(&mut self, waker: ReloadWaker) {
        let (sender, receiver) = channel();
        let path = self.path.clone();
        let mut watcher =
            notify::recommended_watcher(move |result: Result<notify::Event, notify::Error>| {
                if result.is_ok() {
                    // Content is compared at poll time; any event just pokes.
                    let _ = sender.send(());
                    waker.wake();
                }
            })
            .unwrap_or_else(|error| {
                eprintln!("nui-preview: watcher init failed: {error}");
                std::process::exit(1);
            });
        if let Err(error) = watcher.watch(&path, notify::RecursiveMode::NonRecursive) {
            eprintln!("nui-preview: cannot watch `{}`: {error}", path.display());
            std::process::exit(1);
        }
        self.receiver = receiver;
        self._watcher = Some(watcher);
    }

    fn poll_reload(&mut self) -> Option<String> {
        let mut notified = false;
        while self.receiver.try_recv().is_ok() {
            notified = true;
        }
        if !notified {
            return None;
        }
        let source = std::fs::read_to_string(&self.path).ok()?;
        if self.last_seen.as_deref() == Some(source.as_str()) {
            return None;
        }
        self.last_seen = Some(source.clone());
        return Some(source);
    }
}

fn main() {
    let Some(arguments) = parse_arguments() else {
        eprintln!(
            "usage: nui-preview <file.nui> [--width <dp>] [--height <dp>]\n\
             example: cargo run -p nui-preview -- examples/counter.nui --width 420"
        );
        std::process::exit(2);
    };
    let Ok(source) = std::fs::read_to_string(&arguments.path) else {
        eprintln!("nui-preview: cannot read `{}`", arguments.path.display());
        std::process::exit(1);
    };
    let title = format!(
        "nui preview — {}",
        arguments
            .path
            .file_name()
            .unwrap_or_default()
            .to_string_lossy()
    );
    let config = AppConfig::new(source, title, arguments.size).with_resizable(arguments.resizable);
    let watcher = FileWatcher::new(arguments.path);
    return nui::Application::new(config)
        .with_watcher(Box::new(watcher))
        .run();
}

struct Arguments {
    path: PathBuf,
    size: Size,
    resizable: bool,
}

/// Minimal CLI parsing: positional path plus optional
/// `--width`/`--height`/`--no-resize`.
fn parse_arguments() -> Option<Arguments> {
    let mut args = std::env::args().skip(1);
    let path = PathBuf::from(args.next()?);
    let mut size = Size::new(420.0, 300.0);
    let mut resizable = true;
    while let Some(flag) = args.next() {
        match flag.as_str() {
            "--width" => {
                size.width = args.next()?.parse().ok()?;
            }
            "--height" => {
                size.height = args.next()?.parse().ok()?;
            }
            "--no-resize" => {
                resizable = false;
            }
            other => {
                eprintln!("nui-preview: unknown argument `{other}`");
                return None;
            }
        }
    }
    return Some(Arguments {
        path,
        size,
        resizable,
    });
}

#[cfg(test)]
#[allow(clippy::unwrap_used)]
mod tests {
    use super::*;
    use std::sync::mpsc::channel;

    /// Writes `content` to a fresh temp file and returns its path.
    fn temp_file(name: &str, content: &str) -> PathBuf {
        let path = std::env::temp_dir().join(format!("nui-preview-test-{name}"));
        std::fs::write(&path, content).unwrap();
        return path;
    }

    #[test]
    fn polls_only_when_notified_and_content_changed() {
        let path = temp_file("dedupe.nui", "component A {}");
        let (sender, receiver) = channel();
        let mut watcher = FileWatcher::with_channel(path.clone(), receiver);

        // No notification: nothing to do.
        assert_eq!(watcher.poll_reload(), None);

        // Notified + changed content: reload.
        sender.send(()).unwrap();
        let source = watcher.poll_reload().unwrap();
        assert_eq!(source, "component A {}");

        // Notified again with identical content: deduped.
        sender.send(()).unwrap();
        assert_eq!(watcher.poll_reload(), None);

        // Changed content wins even without a fresh notification... no: a
        // notification is always required, so unchanged content stays put.
        std::fs::write(&path, "component B {}").unwrap();
        sender.send(()).unwrap();
        assert_eq!(watcher.poll_reload().unwrap(), "component B {}");
        std::fs::remove_file(&path).ok();
    }
}

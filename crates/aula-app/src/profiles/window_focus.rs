//! Which application has focus, on the platform we are on.
//!
//! The stub always returns `None` — the real implementations are added
//! per platform in their own branches, and the interface is fixed here so
//! the UI and the watcher do not have to know which one is present.
pub fn active_process_name() -> Option<String> {
    None
}

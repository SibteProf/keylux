//! Applies per-application profiles in the background.
//!
//! The tick lives in its own thread rather than in `App::update` because a
//! hidden egui window is never asked to paint — `update` stops running, and
//! a profile would only switch while the user was looking at the app.

use std::sync::{
    atomic::{AtomicBool, Ordering},
    mpsc::Sender,
    Arc, Mutex,
};
use std::time::Duration;

use crate::engine::{Cmd, Shared};
use crate::profiles::{window_focus, ProfilesStore};

/// Directly affects the profile switching delay.
const POLL_INTERVAL: Duration = Duration::from_millis(250);

pub struct ProfilesTicker {
    stop: Arc<AtomicBool>,
    handle: Option<std::thread::JoinHandle<()>>,
}

impl ProfilesTicker {
    /// Start the poller.
    ///
    /// `enabled` is shared with the UI so the checkbox takes effect
    /// immediately.
    pub fn spawn(
        profiles: Arc<Mutex<ProfilesStore>>,
        enabled: Arc<AtomicBool>,
        shared: Arc<Mutex<Shared>>,
        send: Sender<Cmd>,
    ) -> Self {
        let stop = Arc::new(AtomicBool::new(false));
        let stop_thread = Arc::clone(&stop);

        let handle = std::thread::Builder::new()
            .name("aula-profiles".into())
            .spawn(move || {
                let mut app_in_focus: Option<String> = None;
                while !stop_thread.load(Ordering::Relaxed) {
                    std::thread::sleep(POLL_INTERVAL);
                    if !enabled.load(Ordering::Relaxed) {
                        continue;
                    }
                    let active = window_focus::active_process_name();
                    if app_in_focus == active {
                        continue;
                    }
                    // Look up the effect assignet to profile.
                    // Hold the profile only for the lookup,
                    // not for the send.
                    let effect_id = {
                        let store = profiles.lock().unwrap();
                        active
                            .as_deref()
                            .and_then(|app| store.active_for(app))
                            .map(|p| p.effect_id.clone())
                    };
                    if let Some(effect_id) = effect_id {
                        let idx = shared
                            .lock()
                            .unwrap()
                            .effects
                            .iter()
                            .position(|e| e.meta.id == effect_id);
                        if let Some(idx) = idx {
                            let _ = send.send(Cmd::SelectEffect(idx));
                        }
                    }

                    app_in_focus = active
                }
            })
            .expect("spawn profiles ticker");

        Self {
            stop,
            handle: Some(handle),
        }
    }
}

impl Drop for ProfilesTicker {
    fn drop(&mut self) {
        self.stop.store(true, Ordering::Relaxed);
        if let Some(h) = self.handle.take() {
            let _ = h.join();
        }
    }
}

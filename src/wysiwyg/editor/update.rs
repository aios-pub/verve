//! Editor-facing update-check flow.
//!
//! velotype checks GitHub releases for a newer version and prompts the user.
//! verve ships its own updater (`crate::updater`), so in the embedded build
//! this is a no-op: `request_check_updates` simply defers to verve's flow
//! (callers can wire it to verve's updater from the integration layer).

use gpui::*;

use super::Editor;

impl Editor {
    /// No-op in the embedded build.
    ///
    /// verve owns update checks; wire this to `crate::updater` from the notes
    /// panel integration if a per-document "check for updates" entry is wanted.
    pub(crate) fn request_check_updates(&mut self, _window: &mut Window, _cx: &mut Context<Self>) {
        // Intentionally empty.
    }
}

//! Tray icon and menu, driven by `state::Health`.
//!
//! Owned by the daemon workstream, since the tray reflects engine state and
//! must live in the same process as the scheduler.

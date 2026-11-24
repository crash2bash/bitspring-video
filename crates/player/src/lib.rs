//! The player, as a library: playback, settings, and the state the interface is shown.
//!
//! Two programs are built on it. `bitspring-video` (this crate's binary) opens its own window;
//! on the Mac that window carries the SwiftUI interface. On Windows the WinUI application is
//! the program and loads this library through `crates/winhost`.

// Playback exists where there is a renderer for the system: the Mac and Windows.
#[cfg(any(target_os = "macos", windows))]
pub mod app;
pub mod history;
pub mod language;
pub mod launch;
#[cfg(any(target_os = "macos", windows))]
pub mod playback;
pub mod prefs;
pub mod queue;
pub mod report;
pub mod sidecars;
#[cfg(any(target_os = "macos", windows))]
pub mod surface;
#[cfg(any(target_os = "macos", windows))]
pub mod ui;

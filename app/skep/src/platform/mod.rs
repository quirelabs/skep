//! Everything that talks to AppKit. It lives here, in the app, and never in
//! the engine: comb knows about processes, not about menubars.

#[cfg(target_os = "macos")]
mod mac;
#[cfg(target_os = "macos")]
pub use mac::Menubar;

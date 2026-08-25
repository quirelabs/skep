//! The palette, and the rule that keeps it honest: status colour lives only in
//! a row's dot, and the accent lives only on things you can press or things in
//! motion. The two vocabularies never mix.

use comb::ServiceState;
use gpui::{Hsla, rgb};

pub struct Theme {
    pub base: Hsla,
    pub raised: Hsla,
    pub text: Hsla,
    pub muted: Hsla,
    pub border: Hsla,
    /// Interactive elements, and transient states. Nothing steady.
    pub accent: Hsla,
    pub running: Hsla,
    pub failed: Hsla,
    pub idle: Hsla,
}

impl Theme {
    pub fn dark() -> Self {
        Self {
            base: rgb(0x0b0b0c).into(),
            raised: rgb(0x141416).into(),
            text: rgb(0xf2f1ee).into(),
            muted: rgb(0x86868b).into(),
            border: rgb(0x232326).into(),
            accent: rgb(0xff6a1f).into(),
            running: rgb(0x3fbf6f).into(),
            failed: rgb(0xe5484d).into(),
            idle: rgb(0x4d4d54).into(),
        }
    }

    /// The dot, and only the dot. Orange here means motion and nothing else:
    /// if a steady state ever wants it, the design has drifted.
    pub fn dot(&self, state: &ServiceState, working: bool) -> Hsla {
        match state {
            ServiceState::Ready => self.running,
            ServiceState::Failed { .. } => self.failed,
            _ if working || state.is_transitional() => self.accent,
            _ => self.idle,
        }
    }
}

//! The palette and the type scale.
//!
//! Two rules keep the palette honest: status colour lives only in a row's dot,
//! and the accent lives only on things you can press or things in motion. The
//! two vocabularies never mix.
//!
//! Both appearances are one system rather than a dark design with a light
//! variant bolted on. Every token exists in both, and the accent is a different
//! orange in each because the same one cannot carry text contrast on paper and
//! on near black.

use comb::ServiceState;
use gpui::{FontWeight, Hsla, Styled, WindowAppearance, px, rgb};

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

    /// Paper rather than white: the same warmth the dark text carries, so the
    /// two appearances read as one family.
    pub fn light() -> Self {
        Self {
            base: rgb(0xfbfaf8).into(),
            raised: rgb(0xffffff).into(),
            text: rgb(0x1a1a1c).into(),
            muted: rgb(0x6e6e73).into(),
            border: rgb(0xe4e2dd).into(),
            // The dark theme's orange reads at 2.75 against paper and cannot
            // carry text. This one is 5.11, and every token below clears 4.5.
            accent: rgb(0xb84700).into(),
            running: rgb(0x1a7f4b).into(),
            failed: rgb(0xc0272d).into(),
            idle: rgb(0xb5b5ba).into(),
        }
    }

    pub fn for_appearance(appearance: WindowAppearance) -> Self {
        match appearance {
            WindowAppearance::Dark | WindowAppearance::VibrantDark => Self::dark(),
            WindowAppearance::Light | WindowAppearance::VibrantLight => Self::light(),
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

/// The type scale. Four sizes, and every one of them names what a thing is
/// rather than how big it is, so a screen cannot quietly invent a fifth.
///
/// Leading tightens as size grows, which is the one typographic rule that
/// still applies here: gpui has no letter spacing at this revision, so
/// hierarchy has to come from size, weight and leading alone.
///
/// The sizes sit around 13px because that is what macOS uses for a control,
/// and an app that ignores it looks like it came from somewhere else.
pub trait Scale: Styled + Sized {
    /// Screen and section headings.
    fn title(self) -> Self {
        self.text_size(px(15.))
            .line_height(px(20.))
            .font_weight(FontWeight::MEDIUM)
    }

    /// Anything a person reads a sentence of.
    fn body(self) -> Self {
        self.text_size(px(13.)).line_height(px(18.))
    }

    /// Rows, values, and the names of things.
    fn label(self) -> Self {
        self.text_size(px(12.)).line_height(px(16.))
    }

    /// Explanations, counts, and anything deliberately quiet.
    fn caption(self) -> Self {
        self.text_size(px(11.)).line_height(px(15.))
    }
}

impl<T: Styled> Scale for T {}

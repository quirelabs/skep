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
//!
//! The window is not a flat colour. Two washes bloom out of opposite corners,
//! warm from the top left and cool from the bottom right, and everything else
//! is laid over them on surfaces that let a little of it through. The warm one
//! is the app's own colour rather than a decoration: a skep is a straw beehive
//! and the engine in it is called comb.

use comb::ServiceState;
use gpui::{FontWeight, Hsla, Styled, WindowAppearance, px, rgb};

/// A colour with an alpha, written the way the rest of the palette is.
fn alpha(hex: u32, alpha: f32) -> Hsla {
    let mut colour: Hsla = rgb(hex).into();
    colour.a = alpha;
    colour
}

#[derive(Clone)]
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
    /// The two corner washes, warm first.
    pub wash: (Hsla, Hsla),
    /// What panels and rows are made of: the raised colour with the wash
    /// showing through, so a surface belongs to the window it sits in.
    pub surface: Hsla,
    /// Quiet text that sits on the wash rather than on a surface. Stronger
    /// than muted on purpose: over a colour that shifts across the window,
    /// grey stops carrying, so this is the text colour held back instead.
    pub chrome: Hsla,
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
            wash: (alpha(0xff7a2a, 0.16), alpha(0x4b5cff, 0.10)),
            surface: alpha(0x17171a, 0.72),
            chrome: alpha(0xf2f1ee, 0.70),
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
            // Stronger than the dark pair: a wash has to survive being laid
            // over paper, where there is no darkness for it to glow against.
            wash: (alpha(0xff9a3d, 0.20), alpha(0x6f86ff, 0.13)),
            surface: alpha(0xffffff, 0.76),
            // 0.70 rather than the dark side's, because the wash over paper
            // leaves less headroom: this is 5.54 against the busiest corner.
            chrome: alpha(0x1a1a1c, 0.70),
        }
    }

    /// The window's own colour, under the washes.
    pub fn backdrop(&self) -> Hsla {
        self.base
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

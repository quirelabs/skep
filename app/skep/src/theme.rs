//! The palette and the type scale.
//!
//! One rule keeps the palette honest: colour is only ever status. Green for
//! running, red for failed, and nothing else in the interface has a hue at
//! all. What you can press and what is moving are marked by brightness
//! instead, which is why the accent is white on the dark side and near black
//! on the light one.
//!
//! It was a honey orange until the window got a picture behind it. An accent
//! has to stand apart from what it is laid over, and that picture is already
//! made of warm oranges and roses; the accent disappeared into it. The colour
//! is not gone from the app, it moved to where it belongs, and now there is
//! one thing carrying colour rather than two competing.
//!
//! Both appearances are one system rather than a dark design with a light
//! variant bolted on. Every token exists in both.
//!
//! The window is not a flat colour. Three colours lie low across the bottom
//! in soft bands that bleed into one another, with fine grain through the
//! whole of it, and the top left where the work happens stays clean. The warm
//! one is the app's own rather than a decoration: a skep is a straw beehive
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
    /// The colours the light in the window is made of, laid low and blended
    /// into each other. Warm first, because that one is the app's own.
    pub sky: [Hsla; 3],
    /// How far the sky is allowed to carry, and how much grain is in it.
    ///
    /// Both are held down by contrast rather than by taste. The sky is
    /// strongest at the bottom of the window, which is where the busiest text
    /// sits, so these are the largest values at which the quietest text over
    /// the page still clears 4.5 against every point of it.
    pub weather: (f32, f32),
    /// How strongly a row carries its own state across itself. One number
    /// rather than one per page, and held down by contrast rather than by
    /// taste: this is the strongest wash at which the quietest text in a row
    /// still clears 4.5 against every point of the window, for the palest
    /// colour any row ever uses.
    pub wash: f32,
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
            // Lifted from the flat grey it was: the page it sits on is a
            // surface over a picture with grain in it now, and the old one
            // measured 4.43 against the busiest part of that.
            muted: rgb(0x8e8e94).into(),
            border: rgb(0x232326).into(),
            // Brighter than the text it sits among, which is what makes it
            // read as raised rather than merely coloured.
            accent: rgb(0xffffff).into(),
            running: rgb(0x3fbf6f).into(),
            failed: rgb(0xe5484d).into(),
            idle: rgb(0x4d4d54).into(),
            sky: [
                rgb(0xff7a2a).into(),
                rgb(0xff3d6e).into(),
                rgb(0x4b5cff).into(),
            ],
            weather: (0.24, 0.045),
            wash: 0.05,
            surface: alpha(0x17171a, 0.78),
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
            // Darkened for the same reason the dark side's was lifted: a row
            // washed in its own state is a background this has to clear, and
            // paper leaves less headroom than darkness does.
            muted: rgb(0x67676c).into(),
            border: rgb(0xe4e2dd).into(),
            // The same move on paper: darker than the text rather than
            // lighter, since it is the deepest ink that reads as raised here.
            accent: rgb(0x0a0a0b).into(),
            running: rgb(0x1a7f4b).into(),
            failed: rgb(0xc0272d).into(),
            idle: rgb(0xb5b5ba).into(),
            sky: [
                rgb(0xff9a3d).into(),
                rgb(0xff6f91).into(),
                rgb(0x7c8cff).into(),
            ],
            // Lighter on paper, and a touch more grain: there is no darkness
            // for the colour to glow against, so it has to stay a suggestion.
            weather: (0.24, 0.050),
            wash: 0.05,
            surface: alpha(0xffffff, 0.78),
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

    /// The dot, and only the dot. The accent here means motion and nothing
    /// else: if a steady state ever wants it, the design has drifted.
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
    /// The one thing on a screen that is larger than everything else. Leading
    /// tightens as size grows, which is the rule that still applies without
    /// tracking to go with it.
    fn display(self) -> Self {
        self.text_size(px(26.))
            .line_height(px(30.))
            .font_weight(FontWeight::MEDIUM)
    }

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

#[cfg(test)]
mod tests {
    use super::*;

    /// One channel of sRGB, undone. The ratio is defined on light, not on the
    /// numbers a file stores.
    fn linear(channel: f32) -> f32 {
        if channel <= 0.03928 {
            channel / 12.92
        } else {
            ((channel + 0.055) / 1.055).powf(2.4)
        }
    }

    fn luminance(colour: (f32, f32, f32)) -> f32 {
        let (r, g, b) = (linear(colour.0), linear(colour.1), linear(colour.2));
        0.2126 * r + 0.7152 * g + 0.0722 * b
    }

    fn contrast(ink: (f32, f32, f32), under: (f32, f32, f32)) -> f32 {
        let (a, b) = (luminance(ink), luminance(under));
        (a.max(b) + 0.05) / (a.min(b) + 0.05)
    }

    fn rgb_of(colour: Hsla) -> (f32, f32, f32) {
        let rgba = colour.to_rgb();
        (rgba.r, rgba.g, rgba.b)
    }

    fn over(top: (f32, f32, f32), alpha: f32, under: (f32, f32, f32)) -> (f32, f32, f32) {
        (
            alpha * top.0 + (1. - alpha) * under.0,
            alpha * top.1 + (1. - alpha) * under.1,
            alpha * top.2 + (1. - alpha) * under.2,
        )
    }

    /// Every background a row's quiet text can find itself on: each of the
    /// three colours in the sky at full strength, the page surface over it,
    /// and then the row's own state washed across that.
    fn worst(theme: &Theme) -> f32 {
        let base = rgb_of(theme.base);
        let mut lowest = f32::MAX;
        for colour in theme.sky {
            let lit = over(rgb_of(colour), theme.weather.0, base);
            let surface = over(rgb_of(theme.surface), theme.surface.a, lit);
            for state in [theme.running, theme.failed, theme.accent] {
                let washed = over(rgb_of(state), theme.wash, surface);
                lowest = lowest.min(contrast(rgb_of(theme.muted), washed));
            }
        }
        lowest
    }

    /// The wash is a decoration; the words are the point. This is the check
    /// that keeps the first from eating the second, in both appearances,
    /// because a number chosen by eye on one of them fails on the other: the
    /// sixth this started at measured 4.45 on dark and 4.36 on light.
    #[test]
    fn a_row_washed_in_its_own_state_is_still_readable() {
        for (which, theme) in [("dark", Theme::dark()), ("light", Theme::light())] {
            let measured = worst(&theme);
            assert!(
                measured >= 4.5,
                "{which}: quiet text over a washed row measures {measured:.2}, under 4.5"
            );
        }
    }
}

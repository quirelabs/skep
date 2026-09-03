//! The rail down the left, the title bar it shares with the traffic lights,
//! and the page header that carries the collapse control.

use super::*;

/// The rail shows what skep is designed to have, dimmed where it does not have
/// it yet. Settings sits apart at the bottom, where settings go.
pub(super) const RAIL: &[(&str, &str, Option<Page>)] = &[
    // A skep is a straw beehive and the engine inside it is called comb, so
    // the cell is the app's own shape rather than a borrowed one.
    ("Services", "hexagon", Some(Page::Services)),
    ("Sites", "globe-simple", Some(Page::Sites)),
    ("Projects", "squares-four", None),
    ("Logs", "list-dashes", None),
    ("Mail", "envelope-simple", Some(Page::Mail)),
    ("Agent", "sparkle", None),
];

/// The thin weight is eight units in a 256 unit box, so an icon lands on whole
/// device pixels only when its size divides by sixteen. At two times that
/// means 16 or 32 and nothing between, and 16 is the rail's size. Growing it
/// to 18 or 20 would soften every glyph, so extra room comes from padding.
/// The thin weight is eight units in a 256 unit box, so a glyph sits on whole
/// device pixels only at sizes that divide by sixteen: at two times that is 16
/// or 32 and nothing between. 20 is a quarter pixel off and softens very
/// slightly, which is the price of the presence it buys.
pub(super) const GLYPH: f32 = 20.;

pub(super) const SETTINGS_GLYPH: &str = "sliders-horizontal";
pub(super) const COLLAPSE_GLYPH: &str = "sidebar-simple";

pub(super) const RAIL_WIDE: f32 = 208.;

/// How far a page heading must stand clear of the traffic lights: whatever
/// width the rail is not currently covering for it.
pub(super) fn clearance(rail: f32, lights: bool) -> f32 {
    if !lights {
        // Full screen: there are no buttons in that corner to stand clear of.
        return 0.;
    }
    (LIGHTS - rail).max(0.)
}

/// The band across the top of the window. The traffic lights sit in its left,
/// in the rail while the rail is open and over the page header once it is not,
/// so both have to stand this tall and leave that corner alone.
pub(super) const TITLEBAR: f32 = 44.;

pub(super) const LIGHTS: f32 = 84.;

impl Skep {
    pub(super) fn rail(&self, cx: &mut Context<Self>) -> AnyElement {
        let mut items = Vec::with_capacity(RAIL.len());
        for (index, (name, glyph, page)) in RAIL.iter().enumerate() {
            items.push(self.rail_item(index, name, glyph, *page, cx));
        }

        let (from, to, moves) = (self.rail_from, self.rail_to, self.rail_moves);
        div()
            .flex()
            .flex_col()
            .h_full()
            .flex_shrink_0()
            // Clipped, so the words hold their shape on the way out instead of
            // rewrapping into a narrower and narrower column.
            .overflow_hidden()
            .pb_3()
            .child(self.rail_top())
            .gap_0p5()
            .children(items)
            .child(div().flex_1())
            .child(self.rail_item(
                RAIL.len(),
                "Settings",
                SETTINGS_GLYPH,
                Some(Page::Settings),
                cx,
            ))
            .with_animation(
                ("rail", moves),
                Animation::new(MOTION).with_easing(ease_in_out),
                move |rail, delta| rail.w(px(from + (to - from) * delta)),
            )
            .into_any_element()
    }

    /// The rail's own top band. Empty on purpose: the traffic lights are
    /// drawn over it by the window, and it drags the way a titlebar would.
    pub(super) fn toggle_rail(&mut self) {
        let current = self.rail_width();
        let opening = self.rail_to == 0.;
        self.rail_from = current;
        self.rail_to = if opening { RAIL_WIDE } else { 0. };
        self.rail_since = Instant::now();
        self.rail_moves += 1;
    }

    /// Where the rail is at this instant, which is not where it is going. A
    /// toggle part way through a slide starts from this rather than from the
    /// width the last slide began at, so reversing does not jump.
    pub(super) fn rail_width(&self) -> f32 {
        let progress =
            (self.rail_since.elapsed().as_secs_f32() / MOTION.as_secs_f32()).clamp(0., 1.);
        self.rail_from + (self.rail_to - self.rail_from) * ease_in_out(progress)
    }

    pub(super) fn rail_top(&self) -> AnyElement {
        div()
            .id("rail-top")
            .w_full()
            .h(px(if self.fullscreen { 8. } else { TITLEBAR }))
            .flex_shrink_0()
            .on_mouse_down(gpui::MouseButton::Left, |_, window, _| {
                window.start_window_move();
            })
            .into_any_element()
    }

    /// A page's heading, with the control that shuts the rail beside it. It
    /// lives here rather than in the rail because a control that goes away
    /// when you use it cannot bring back what it took.
    ///
    /// The padding grows as the rail shrinks, by exactly as much as the rail
    /// gives up, so the traffic lights never land on the words.
    pub(super) fn page_title(&self, title: &'static str, cx: &mut Context<Self>) -> AnyElement {
        let (from, to, moves) = (self.rail_from, self.rail_to, self.rail_moves);
        let lights = !self.fullscreen;
        div()
            .flex()
            .items_center()
            .gap_2()
            .flex_shrink_0()
            .child(
                div()
                    .id("collapse")
                    .flex()
                    .items_center()
                    .justify_center()
                    .size(px(28.))
                    .flex_shrink_0()
                    .rounded(px(CARD))
                    .cursor_pointer()
                    .hover(|style| style.bg(self.theme.raised))
                    .child(
                        svg()
                            .path(format!("icons/{COLLAPSE_GLYPH}.svg"))
                            .size(px(GLYPH))
                            .text_color(self.theme.muted),
                    )
                    .on_click(cx.listener(|skep, _, _, cx| {
                        skep.toggle_rail();
                        cx.notify();
                    })),
            )
            .child(div().title().child(SharedString::from(title)))
            .with_animation(
                ("title", moves),
                Animation::new(MOTION).with_easing(ease_in_out),
                move |title, delta| title.pl(px(clearance(from + (to - from) * delta, lights))),
            )
            .into_any_element()
    }

    /// The band every screen wears: the collapse control, the screen's name,
    /// and whatever that screen has to say about itself on the right. One
    /// function rather than four, so no screen can drift a pixel from the
    /// others.
    pub(super) fn page_header(
        &self,
        title: &'static str,
        trailing: Option<AnyElement>,
        cx: &mut Context<Self>,
    ) -> AnyElement {
        div()
            .flex()
            .items_center()
            .justify_between()
            .w_full()
            .px(px(MARGIN))
            .h(px(TITLEBAR))
            .flex_shrink_0()
            .border_b_1()
            .border_color(self.theme.border)
            .child(self.page_title(title, cx))
            .children(trailing)
            .into_any_element()
    }

    pub(super) fn rail_item(
        &self,
        index: usize,
        name: &'static str,
        glyph: &'static str,
        page: Option<Page>,
        cx: &mut Context<Self>,
    ) -> AnyElement {
        let built = page.is_some();
        let here = page == Some(self.page);
        // On the wash rather than on a surface, so the quiet colour is the
        // text colour held back rather than a grey.
        let colour = if here {
            self.theme.text
        } else if built {
            self.theme.chrome
        } else {
            self.theme.idle
        };

        let mut item = div()
            .id(("rail", index))
            .flex()
            .items_center()
            .w(px(RAIL_WIDE - 16.))
            .flex_shrink_0()
            .gap_3()
            .mx_2()
            .px_3()
            .py_2()
            .rounded(px(CARD))
            .text_color(colour);

        // The selected row is a surface rather than an accent fill: the
        // accent is reserved for what you press and what is moving, and a
        // whole row of it would drown both.
        if here {
            item = item.bg(self.theme.surface);
        }

        item = item.child(
            svg()
                .path(format!("icons/{glyph}.svg"))
                .size(px(GLYPH))
                .flex_shrink_0()
                .text_color(if here { self.theme.accent } else { colour }),
        );

        item = item.child(
            div()
                .body()
                .min_w_0()
                .font_weight(if here {
                    FontWeight::MEDIUM
                } else {
                    FontWeight::NORMAL
                })
                .child(SharedString::from(name)),
        );

        match page {
            Some(page) => item
                .cursor_pointer()
                .hover(|style| style.bg(self.theme.raised))
                .on_click(cx.listener(move |skep, _, _, cx| {
                    skep.page = page;
                    // A page that shows something fetched asks for it on the
                    // way in rather than showing yesterday's answer.
                    match page {
                        Page::Mail => {
                            let _ = skep.commands.send(Command::Mail);
                        }
                        Page::Sites => {
                            let _ = skep.commands.send(Command::CheckSites);
                        }
                        _ => {}
                    }
                    cx.notify();
                }))
                .into_any_element(),
            None => item.into_any_element(),
        }
    }
}

#[cfg(test)]
mod tests {
    use gpui::AssetSource;

    use super::{COLLAPSE_GLYPH, RAIL, SETTINGS_GLYPH};
    use crate::icons::Icons;

    /// A glyph that does not resolve fails silently at runtime: gpui draws
    /// nothing and the element stays clickable, so the control is invisible
    /// but still works. Checking only the rail missed exactly that, because
    /// the collapse control is not in it.
    #[test]
    fn every_icon_on_disk_is_registered() {
        let directory = concat!(env!("CARGO_MANIFEST_DIR"), "/assets/icons");
        let mut checked = 0;
        for entry in std::fs::read_dir(directory).expect("the icons directory exists") {
            let path = entry.expect("a readable entry").path();
            if path.extension().is_none_or(|kind| kind != "svg") {
                continue;
            }
            let name = path.file_stem().unwrap().to_string_lossy().into_owned();
            assert!(
                Icons.load(&format!("icons/{name}.svg")).unwrap().is_some(),
                "{name}.svg is in the assets directory but not in the icons table, \
                 so anything asking for it draws nothing"
            );
            checked += 1;
        }
        assert!(checked > 0, "no icons were found to check");
    }

    #[test]
    fn every_glyph_the_rail_asks_for_exists() {
        for (name, glyph, _) in RAIL.iter().chain([&("Settings", SETTINGS_GLYPH, None)]) {
            let found = Icons
                .load(&format!("icons/{glyph}.svg"))
                .expect("looking one up cannot fail");
            let bytes = found.unwrap_or_else(|| panic!("{name} wants {glyph}, which is not here"));
            assert!(
                bytes.starts_with(b"<svg"),
                "{glyph} does not look like an svg"
            );
        }
    }

    #[test]
    fn the_collapse_control_has_a_glyph() {
        assert!(
            Icons
                .load(&format!("icons/{COLLAPSE_GLYPH}.svg"))
                .unwrap()
                .is_some()
        );
    }

    #[test]
    fn a_glyph_that_is_not_there_is_absent_rather_than_wrong() {
        assert!(Icons.load("icons/nothing-like-this.svg").unwrap().is_none());
    }
}

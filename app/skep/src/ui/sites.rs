//! The Sites page: the list, the preview beside it, and the inline draft
//! for adding one.

use super::paint::{dither, snow};
use super::rail::GLYPH;

/// A tile keeps a page's proportions, because it is a picture of one.
const TILE_WIDE: f32 = 260.;
const TILE_TALL: f32 = 163.;
use super::*;

/// A site being written. Kept as text rather than as a port number, because
/// half a number is not a number and refusing to show what somebody typed is
/// worse than waiting until they finish.
#[derive(Default)]
pub(super) struct Draft {
    host: String,
    port: String,
    on_port: bool,
    pub(super) complaint: Option<String>,
}

impl Skep {
    /// Writing a site down. Inline, in the list, where the row it becomes will
    /// be: a window that opens over the top to ask two questions is a heavier
    /// thing than the answer deserves, and this app does not have one.
    pub(super) fn new_site(&self, cx: &mut Context<Self>) -> AnyElement {
        let theme = &self.theme;

        let Some(draft) = &self.draft else {
            return div()
                .id("add-site")
                .flex()
                .items_center()
                .gap_2()
                .m_3()
                .px_3()
                .py_2p5()
                .rounded_md()
                .cursor_pointer()
                .border_1()
                .border_dashed()
                .border_color(theme.border)
                // The accent, because this is a thing you press. It is the one
                // way into the screen and it was reading as another dim line
                // in a list of dim lines.
                .text_color(theme.accent)
                .hover(|style| style.bg(theme.raised).border_color(theme.accent))
                .on_click(cx.listener(|skep, _, window, cx| {
                    skep.draft = Some(Draft::default());
                    // Focus goes with it, or the keys would land nowhere.
                    skep.entry.clone().focus(window, cx);
                    cx.notify();
                }))
                .child(svg().path("icons/plus.svg").size(px(GLYPH)).flex_shrink_0())
                .child(SharedString::from("Add a site"))
                .child(div().flex_1())
                .child(
                    div()
                        .caption()
                        .text_color(theme.muted)
                        .child(SharedString::from(
                            "point a hostname at a port you already run something on",
                        )),
                )
                .into_any_element();
        };

        // The caret sits in whichever field is taking the keys, so there is
        // never a question about where typing goes.
        let field = |text: &str, here: bool, hint: &'static str, width: Option<f32>| {
            let shown = if text.is_empty() && !here {
                hint.to_string()
            } else if here {
                format!("{text}|")
            } else {
                text.to_string()
            };
            let mut cell = div()
                .px_2()
                .py_0p5()
                .rounded_sm()
                .min_w_0()
                .truncate()
                .text_color(if text.is_empty() && !here {
                    theme.idle
                } else {
                    theme.text
                })
                .bg(if here {
                    theme.base
                } else {
                    gpui::transparent_black()
                })
                .child(SharedString::from(shown));
            match width {
                Some(width) => cell = cell.w(px(width)).flex_shrink_0(),
                None => cell = cell.flex_1(),
            }
            cell
        };

        let mut row = div()
            .id("new-site")
            .track_focus(&self.entry)
            .flex()
            .flex_col()
            .w_full()
            .px_6()
            .py_2()
            .gap_1()
            .bg(theme.raised)
            .on_key_down(cx.listener(Self::typing))
            .child(
                div()
                    .flex()
                    .items_center()
                    .gap_3()
                    .w_full()
                    .min_w_0()
                    .child(
                        div()
                            .size(px(6.))
                            .flex_shrink_0()
                            .rounded_full()
                            .bg(theme.accent),
                    )
                    .child(field(&draft.host, !draft.on_port, "myapp.test", None))
                    .child(field(&draft.port, draft.on_port, "3000", Some(90.)))
                    .child(
                        div()
                            .w(px(150.))
                            .flex_shrink_0()
                            .caption()
                            .text_color(theme.muted)
                            .child(SharedString::from("return to add")),
                    ),
            )
            .child(
                div()
                    .caption()
                    .text_color(theme.muted)
                    .child(SharedString::from(
                        "A hostname ending in .test resolves on its own; any other name needs \
                         an /etc/hosts entry. The port is the one your app is already \
                         listening on. Tab moves between them, escape gives up.",
                    )),
            );

        if let Some(complaint) = &draft.complaint {
            row = row.child(
                div()
                    .caption()
                    .text_color(theme.muted)
                    .child(SharedString::from(complaint.clone())),
            );
        }
        row.into_any_element()
    }

    /// Two short fields do not need an editor. What they need is for the keys
    /// to land where the caret is, and for the wrong ones not to.
    pub(super) fn typing(
        &mut self,
        event: &gpui::KeyDownEvent,
        _: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let Some(draft) = self.draft.as_mut() else {
            return;
        };
        draft.complaint = None;

        match event.keystroke.key.as_str() {
            "escape" => {
                self.draft = None;
                cx.notify();
                return;
            }
            "tab" => draft.on_port = !draft.on_port,
            "backspace" => {
                if draft.on_port {
                    draft.port.pop();
                } else {
                    draft.host.pop();
                }
            }
            "enter" => {
                let host = draft.host.trim().to_string();
                let port: Option<u16> = draft.port.trim().parse().ok();
                match (comb::valid_hostname(&host), port) {
                    // The row stays until the file has it, so a refusal has
                    // somewhere to be said.
                    (Ok(_), Some(port)) if port > 0 => {
                        let _ = self.commands.send(Command::AddSite(host, port));
                    }
                    (Err(_), _) => {
                        draft.complaint = Some(format!(
                            "{host:?} is not a hostname a certificate can cover"
                        ))
                    }
                    (_, None) => draft.complaint = Some("that is not a port".to_string()),
                    _ => draft.complaint = Some("a port cannot be zero".to_string()),
                }
            }
            _ => {
                if let Some(typed) = &event.keystroke.key_char {
                    for character in typed.chars() {
                        if draft.on_port {
                            // Only digits, and no more than a port can be.
                            if character.is_ascii_digit() && draft.port.len() < 5 {
                                draft.port.push(character);
                            }
                        } else if character.is_ascii_alphanumeric()
                            || character == '-'
                            || character == '.'
                        {
                            draft.host.push(character);
                        }
                    }
                }
            }
        }
        cx.notify();
    }

    /// A site, shown. If something is behind it you get the thing itself; if
    /// nothing is, you get a dead channel, which is the truth said in the only
    /// way a picture can say it.
    pub(super) fn watching(&self, host: &str, cx: &mut Context<Self>) -> AnyElement {
        let theme = &self.theme;
        let alive = self.answering.get(host) == Some(&true);
        let ink = theme.idle;
        let preview = self.preview.clone();

        let body = if alive {
            div()
                .flex_1()
                .min_h_0()
                .w_full()
                .child(
                    gpui::canvas(
                        |_, _, _| (),
                        move |bounds, _, _, _| {
                            if let Some(preview) = preview.borrow().as_ref() {
                                preview.place(bounds);
                            }
                        },
                    )
                    .size_full(),
                )
                .into_any_element()
        } else {
            div()
                .relative()
                .flex_1()
                .min_h_0()
                .w_full()
                .overflow_hidden()
                .child(div().absolute().size_full().with_animation(
                    "snow",
                    Animation::new(Duration::from_millis(900)).repeat(),
                    move |field, delta| {
                        field.child(
                            gpui::canvas(
                                |_, _, _| (),
                                move |bounds, _, window, _| snow(bounds, ink, delta, window),
                            )
                            .size_full(),
                        )
                    },
                ))
                .child(
                    div()
                        .absolute()
                        .size_full()
                        .flex()
                        .items_center()
                        .justify_center()
                        .child(
                            div()
                                .label()
                                .px_2()
                                .py_1()
                                .rounded_md()
                                .bg(theme.raised)
                                .text_color(theme.muted)
                                .child(SharedString::from("no signal")),
                        ),
                )
                .into_any_element()
        };

        let shown = host.to_string();
        div()
            .flex()
            .flex_col()
            .w_full()
            .min_w_0()
            .flex_1()
            .min_h_0()
            .border_t_2()
            .border_color(theme.border)
            .bg(theme.raised)
            .child(
                div()
                    .flex()
                    .items_center()
                    .justify_between()
                    .w_full()
                    .min_w_0()
                    .px_6()
                    .py_2()
                    .flex_shrink_0()
                    .child(
                        div()
                            .min_w_0()
                            .truncate()
                            .label()
                            .child(SharedString::from(comb::site_url(&shown, self.site_port))),
                    )
                    .child(
                        div()
                            .flex()
                            .items_center()
                            .gap_3()
                            .flex_shrink_0()
                            .child(
                                div()
                                    .id("open-site")
                                    .caption()
                                    .cursor_pointer()
                                    .text_color(theme.accent)
                                    .on_click({
                                        let url = comb::site_url(&shown, self.site_port);
                                        move |_, _, cx| cx.open_url(&url)
                                    })
                                    .child(SharedString::from("open in browser")),
                            )
                            .child(
                                div()
                                    .id("close-site")
                                    .caption()
                                    .cursor_pointer()
                                    .text_color(theme.muted)
                                    .on_click(cx.listener(|skep, _, _, cx| {
                                        skep.site = None;
                                        cx.notify();
                                    }))
                                    .child(SharedString::from("close")),
                            ),
                    ),
            )
            .child(body)
            .into_any_element()
    }

    /// What each column of a site is. A hostname and a number side by side
    pub(super) fn sites_page(&self, cx: &mut Context<Self>) -> AnyElement {
        let answering = self.answering.values().filter(|alive| **alive).count();

        let mut notes = div().flex().flex_col().w_full();
        if !self.authority_trusted && !self.sites.is_empty() {
            notes = notes.child(
                self.note("Certificates are not trusted on this machine yet. Run skep trust."),
            );
        }
        for trouble in &self.site_trouble {
            notes = notes.child(self.note(trouble));
        }

        let mut tiles: Vec<AnyElement> = Vec::with_capacity(self.sites.len() + 1);
        for (host, port) in &self.sites {
            tiles.push(self.tile(host, *port, cx));
        }
        tiles.push(self.new_site(cx));

        div()
            .flex()
            .flex_col()
            .flex_1()
            .h_full()
            .min_w_0()
            .overflow_hidden()
            .child(self.page_header(
                "Sites",
                (!self.sites.is_empty()).then(|| {
                    div()
                        .caption()
                        .flex_shrink_0()
                        .text_color(self.theme.muted)
                        .child(SharedString::from(format!(
                            "{answering} of {} answering",
                            self.sites.len()
                        )))
                        .into_any_element()
                }),
                cx,
            ))
            .child(notes)
            .child(
                div()
                    .id("site-sheet")
                    .flex()
                    .flex_wrap()
                    .content_start()
                    .gap_4()
                    .p_6()
                    .flex_1()
                    .min_h_0()
                    .overflow_y_scroll()
                    .children(tiles),
            )
            .children(self.site.clone().map(|host| self.watching(&host, cx)))
            .into_any_element()
    }

    /// One site: its picture, then its name, port and whether anything is
    /// behind it. A site that has stopped answering keeps the last picture
    /// taken of it, drained of colour and snowed over, because what it looked
    /// like is more use than an empty rectangle.
    pub(super) fn tile(&self, host: &str, port: u16, cx: &mut Context<Self>) -> AnyElement {
        let alive = self.answering.get(host).copied();
        let here = self.site.as_deref() == Some(host);
        let chosen = host.to_string();
        let ink = self.theme.idle;
        let gone = alive == Some(false);

        let mut frame = div()
            .relative()
            .w_full()
            .h(px(TILE_TALL))
            .overflow_hidden()
            .rounded(px(8.))
            .bg(self.theme.raised)
            .border_1()
            .border_color(if here {
                self.theme.accent
            } else {
                self.theme.border
            });

        match self.shots.get(host).cloned() {
            Some(picture) => {
                let mut image = gpui::img(picture).size_full();
                // Drained rather than dropped: the last thing it looked like,
                // clearly in the past.
                if gone {
                    image = image.opacity(0.3);
                }
                frame = frame.child(image);
            }
            None => {
                frame = frame.child(
                    gpui::canvas(
                        |_, _, _| (),
                        move |bounds, _, window, _| dither(bounds, ink, window),
                    )
                    .absolute()
                    .size_full(),
                );
            }
        }
        if gone {
            frame = frame.child(div().absolute().size_full().with_animation(
                SharedString::from(format!("snow-{host}")),
                Animation::new(Duration::from_millis(900)).repeat(),
                move |field, delta| {
                    field.child(
                        gpui::canvas(
                            |_, _, _| (),
                            move |bounds, _, window, _| snow(bounds, ink, delta, window),
                        )
                        .size_full(),
                    )
                },
            ));
        }

        div()
            .id(SharedString::from(format!("site-{host}")))
            .flex()
            .flex_col()
            .gap_2()
            .w(px(TILE_WIDE))
            .flex_shrink_0()
            .cursor_pointer()
            .on_click(cx.listener(move |skep, _, _, cx| {
                skep.site = Some(chosen.clone());
                cx.notify();
            }))
            .child(frame)
            .child(
                div()
                    .flex()
                    .items_center()
                    .gap_2()
                    .w_full()
                    .min_w_0()
                    .child(
                        div()
                            .size(px(6.))
                            .rounded_full()
                            .flex_shrink_0()
                            .bg(match alive {
                                Some(true) => self.theme.running,
                                Some(false) => self.theme.failed,
                                None => self.theme.idle,
                            }),
                    )
                    .child(
                        div()
                            .flex_1()
                            .min_w_0()
                            .truncate()
                            .label()
                            .child(SharedString::from(host.to_string())),
                    )
                    .child(
                        div()
                            .flex_shrink_0()
                            .caption()
                            .font_family(MONO)
                            .text_color(self.theme.muted)
                            .child(SharedString::from(port.to_string())),
                    ),
            )
            .child(
                div()
                    .w_full()
                    .truncate()
                    .caption()
                    .text_color(self.theme.idle)
                    .child(SharedString::from(match alive {
                        Some(true) => "answering".to_string(),
                        Some(false) => format!("nothing on {port}"),
                        None => "not checked".to_string(),
                    })),
            )
            .into_any_element()
    }
}

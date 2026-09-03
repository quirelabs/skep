//! The Sites page: the contact sheet, the preview beside it, and the form
//! for adding a site.

use super::paint::{dither, faded, snow};

/// A tile keeps a page's proportions, because it is a picture of one.
const TILE_WIDE: f32 = 260.;
const TILE_TALL: f32 = 163.;
const TILE_RADIUS: f32 = CARD;
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
        // A cell of the same grid, the same size as the sites beside it:
        // adding one is the same kind of thing as having one. The form it
        // opens is not here; it opens over the window, because a card that
        // grows inside a grid pushes every tile after it sideways while you
        // type in it.
        div()
            .id("add-site")
            .flex()
            .flex_col()
            .gap_2()
            .w(px(TILE_WIDE))
            .flex_shrink_0()
            .cursor_pointer()
            .on_click(cx.listener(|skep, _, window, cx| {
                skep.draft = Some(Draft::default());
                // Focus goes with it, or the keys would land nowhere.
                skep.entry.clone().focus(window, cx);
                cx.notify();
            }))
            .child(
                div()
                    .group("add")
                    .flex()
                    .flex_col()
                    .items_center()
                    .justify_center()
                    .gap_2()
                    .w_full()
                    .h(px(TILE_TALL))
                    .rounded(px(TILE_RADIUS))
                    .border_1()
                    .border_dashed()
                    .border_color(theme.border)
                    .text_color(theme.accent)
                    .hover(|style| style.bg(theme.raised).border_color(theme.accent))
                    .child(
                        svg()
                            .path("icons/plus.svg")
                            .size(px(32.))
                            .flex_shrink_0()
                            // Its own colour: a parent's text colour does
                            // not reach inside an svg.
                            .text_color(theme.muted),
                    ),
            )
            // The same two lines every other tile carries, so the grid
            // reads across as well as down: a name where a name goes, and
            // what it is for where a state goes.
            .child(
                div()
                    .flex()
                    .items_center()
                    .gap_2()
                    .w_full()
                    .min_w_0()
                    .child(div().size(px(6.)).rounded_full().flex_shrink_0())
                    .child(
                        div()
                            .flex_1()
                            .min_w_0()
                            .truncate()
                            .label()
                            .child(SharedString::from("Add a site")),
                    ),
            )
            .child(
                div()
                    .w_full()
                    .truncate()
                    .caption()
                    .text_color(theme.idle)
                    .child(SharedString::from("a name for a port you already run")),
            )
            .into_any_element()
    }

    /// The form for a new site, over the window rather than in it.
    ///
    /// This is the one modal in the app. The rule against them holds for
    /// anything you might want to keep looking at the list while doing, and
    /// this is the opposite: naming a site is a sentence with two blanks, and
    /// it is done or abandoned in seconds.
    pub(super) fn adding(&self, cx: &mut Context<Self>) -> Option<AnyElement> {
        let draft = self.draft.as_ref()?;
        let theme = &self.theme;

        let field = |name: &'static str, hint: &'static str, text: &str, here: bool| {
            let empty = text.is_empty();
            div()
                .flex()
                .flex_col()
                .gap_1p5()
                .w_full()
                .min_w_0()
                .child(
                    div()
                        .caption()
                        .text_color(theme.muted)
                        .child(SharedString::from(name)),
                )
                .child(
                    div()
                        .w_full()
                        .min_w_0()
                        .truncate()
                        .px_3()
                        .py_2()
                        .rounded(px(CHIP))
                        .bg(theme.base)
                        .border_1()
                        .border_color(if here { theme.accent } else { theme.border })
                        .body()
                        .font_family(MONO)
                        .text_color(if empty && !here {
                            theme.idle
                        } else {
                            theme.text
                        })
                        .child(SharedString::from(if empty && !here {
                            hint.to_string()
                        } else if here {
                            format!("{text}|")
                        } else {
                            text.to_string()
                        })),
                )
        };

        Some(
            div()
                .id("adding")
                .track_focus(&self.entry)
                .on_key_down(cx.listener(Self::typing))
                .absolute()
                .inset_0()
                .flex()
                .items_center()
                .justify_center()
                // The window behind is pushed back rather than hidden, so it
                // is plain that the list is still there and still yours.
                .bg(faded(theme.base, 0.72))
                .on_click(cx.listener(|skep, _, _, cx| {
                    skep.draft = None;
                    cx.notify();
                }))
                .child(
                    div()
                        .id("adding-card")
                        .flex()
                        .flex_col()
                        .gap_4()
                        .w(px(380.))
                        .p(px(MARGIN))
                        .rounded(px(PANEL_RADIUS))
                        .bg(theme.raised)
                        .border_1()
                        .border_color(theme.border)
                        // Clicks inside belong to the form, not to the
                        // backdrop that closes it.
                        .on_click(|_, _, cx| cx.stop_propagation())
                        .child(
                            div()
                                .flex()
                                .flex_col()
                                .gap_1()
                                .child(div().title().child(SharedString::from("Add a site")))
                                .child(div().caption().text_color(theme.muted).child(
                                    SharedString::from(
                                        "A name for something you already run. Skep puts the \
                                         name and a certificate in front of it; it does not \
                                         start it.",
                                    ),
                                )),
                        )
                        .child(field("Hostname", "myapp.test", &draft.host, !draft.on_port))
                        .child(field("Port", "3000", &draft.port, draft.on_port))
                        .child(
                            div()
                                .caption()
                                .text_color(if draft.complaint.is_some() {
                                    theme.failed
                                } else {
                                    theme.idle
                                })
                                .child(SharedString::from(match &draft.complaint {
                                    Some(complaint) => complaint.clone(),
                                    None => "A name ending in .test resolves on its own. Tab \
                                             moves between the fields."
                                        .to_string(),
                                })),
                        )
                        .child(
                            div()
                                .flex()
                                .items_center()
                                .justify_end()
                                .gap_2()
                                .child(
                                    div()
                                        .id("cancel")
                                        .px_3()
                                        .py_1p5()
                                        .rounded(px(CHIP))
                                        .label()
                                        .text_color(theme.muted)
                                        .cursor_pointer()
                                        .hover(|style| style.bg(theme.base))
                                        .on_click(cx.listener(|skep, _, _, cx| {
                                            skep.draft = None;
                                            cx.notify();
                                        }))
                                        .child(SharedString::from("Cancel")),
                                )
                                .child(
                                    div()
                                        .id("add")
                                        .px_3()
                                        .py_1p5()
                                        .rounded(px(CHIP))
                                        .border_1()
                                        .border_color(theme.border)
                                        .label()
                                        .text_color(theme.accent)
                                        .cursor_pointer()
                                        .hover(|style| style.border_color(theme.accent))
                                        .on_click(cx.listener(|skep, _, _, cx| {
                                            skep.submit_site();
                                            cx.notify();
                                        }))
                                        .child(SharedString::from("Add site")),
                                ),
                        ),
                )
                .into_any_element(),
        )
    }

    /// Two short fields do not need an editor. What they need is for the keys
    /// to land where the caret is, and for the wrong ones not to.
    /// Takes what has been typed, or says what is wrong with it. The form
    /// stays until the file has the site, so a refusal has somewhere to be
    /// said.
    pub(super) fn submit_site(&mut self) {
        let Some(draft) = self.draft.as_mut() else {
            return;
        };
        draft.complaint = None;
        let host = draft.host.trim().to_string();
        let port: Option<u16> = draft.port.trim().parse().ok();
        match (comb::valid_hostname(&host), port) {
            (Ok(_), Some(port)) if port > 0 => {
                let _ = self.commands.send(Command::AddSite(host, port));
            }
            (Err(_), _) => {
                draft.complaint = Some(format!(
                    "{host:?} is not a hostname a certificate can cover"
                ))
            }
            (_, None) if draft.port.trim().is_empty() => {
                draft.complaint =
                    Some("a port is needed: the one your app already listens on".to_string())
            }
            (_, None) => draft.complaint = Some("that is not a port".to_string()),
            _ => draft.complaint = Some("a port cannot be zero".to_string()),
        }
    }

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
                self.submit_site();
                cx.notify();
                return;
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
                                .rounded(px(CARD))
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
                    .gap(px(MARGIN))
                    .p(px(MARGIN))
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
            .rounded(px(TILE_RADIUS))
            .bg(self.theme.raised)
            .border_1()
            .border_color(if here {
                self.theme.accent
            } else {
                self.theme.border
            });
        if !here {
            frame = frame.hover(|style| style.border_color(self.theme.muted));
        }

        match self.shots.get(host).cloned() {
            Some(picture) => {
                // Rounded here as well as on the frame: an image is its own
                // layer and is not cut by the corners of what holds it.
                let mut image = gpui::img(picture).size_full().rounded(px(TILE_RADIUS));
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
                if skep.sites_in_browser {
                    cx.open_url(&comb::site_url(&chosen, skep.site_port));
                    return;
                }
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

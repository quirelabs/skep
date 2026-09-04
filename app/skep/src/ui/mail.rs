//! The Mail page: the inbox, the reading pane, and the source and checks
//! views beside it.

use super::paint::fingerprint;
use super::*;

/// The three ways to look at one message: as it renders, as it arrived, and as
/// it would fare elsewhere.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(super) enum MailView {
    Rendered,
    Source,
    Checks,
}

/// Every url in a message, in the order they appear and without repeats. The
/// converter writes a link as its words followed by its target in brackets, so
/// finding them is a matter of reading to the next space or bracket.
pub(super) fn links(text: &str) -> Vec<String> {
    let mut found: Vec<String> = Vec::new();
    let mut rest = text;
    while let Some(at) = rest.find("http") {
        rest = &rest[at..];
        if !rest.starts_with("http://") && !rest.starts_with("https://") {
            rest = &rest["http".len()..];
            continue;
        }
        let end = rest
            .find(|c: char| c.is_whitespace() || c == ')' || c == '>')
            .unwrap_or(rest.len());
        let link = rest[..end].trim_end_matches(['.', ',', ';']).to_string();
        if !link.is_empty() && !found.contains(&link) {
            found.push(link);
        }
        rest = &rest[end..];
    }
    found
}

/// The time of day out of an iso timestamp. A message caught minutes ago does
/// not need its date spelled out.
pub(super) fn clock(at: &str) -> String {
    at.split('T')
        .nth(1)
        .and_then(|rest| rest.get(..5))
        .unwrap_or(at)
        .to_string()
}

impl Skep {
    /// What the mail catcher caught. The same shape as everything else here:
    /// a list of rows that open in place, because a message is one more thing
    /// to look inside rather than somewhere else to go.
    pub(super) fn mail_page(&self, cx: &mut Context<Self>) -> AnyElement {
        let theme = &self.theme;
        let open = self.opened.as_ref().map(|body| body.id.clone());

        let mut rows = div().flex().flex_col().w_full();
        if let Some(trouble) = &self.mail_trouble {
            rows = rows.child(self.note(trouble));
        } else if self.mail.is_empty() {
            rows = rows.child(self.nothing("no mail caught yet"));
        }

        for (index, message) in self.mail.iter().enumerate() {
            let id = message.id.clone();
            let showing = open.as_deref() == Some(message.id.as_str());
            let head = div()
                .id(("mail", index))
                .group("mail")
                .relative()
                // The same two marks a service row wears: an edge under the
                // pointer, and the state carried across the row from the side
                // it starts on. Unread is the only state a message has.
                .child(
                    div()
                        .absolute()
                        .left_0()
                        .top_0()
                        .bottom_0()
                        .w(px(2.))
                        .bg(theme.accent)
                        .opacity(0.)
                        .group_hover("mail", |style| style.opacity(1.)),
                )
                .children((!message.read).then(|| {
                    div().absolute().inset_0().bg(gpui::linear_gradient(
                        90.,
                        gpui::linear_color_stop(paint::faded(theme.accent, theme.wash), 0.),
                        gpui::linear_color_stop(paint::faded(theme.accent, 0.), 0.3),
                    ))
                }))
                .flex()
                .items_center()
                .gap_3()
                .w_full()
                .min_w_0()
                .px(px(MARGIN))
                .py(px(14.))
                .cursor_pointer()
                .hover(|style| style.bg(theme.raised))
                .on_click(cx.listener(move |skep, _, _, cx| {
                    if skep.opened.as_ref().is_some_and(|body| body.id == id) {
                        skep.opened = None;
                    } else {
                        let _ = skep.commands.send(Command::ReadMail(id.clone()));
                    }
                    cx.notify();
                }))
                // Unread sits where a service's dot sits, so the one place
                // status is allowed to live keeps holding it.
                .child(
                    div()
                        .size(px(6.))
                        .rounded_full()
                        .flex_shrink_0()
                        .bg(if message.read {
                            gpui::transparent_black()
                        } else {
                            theme.accent
                        }),
                )
                .child(self.sender_mark(&message.from))
                .child(
                    div()
                        .w(px(160.))
                        .truncate()
                        .text_color(theme.muted)
                        .child(SharedString::from(message.from.clone())),
                )
                .child(
                    // Unread is said twice: by the dot, and by the weight of
                    // the subject. A dot alone is a thing to hunt for down a
                    // column; weight is what the eye actually sorts on.
                    {
                        let mut subject = div().w(px(220.)).truncate();
                        if !message.read {
                            subject = subject.font_weight(FontWeight::MEDIUM);
                        }
                        subject.child(SharedString::from(message.subject.clone()))
                    },
                )
                .child(
                    div()
                        .flex_1()
                        .min_w_0()
                        .truncate()
                        .label()
                        .text_color(theme.muted)
                        .child(SharedString::from(message.snippet.clone())),
                )
                .children((message.attachments > 0).then(|| {
                    div()
                        .flex()
                        .items_center()
                        .gap_0p5()
                        .flex_shrink_0()
                        .text_color(theme.muted)
                        .child(svg().path("icons/paperclip.svg").size(px(13.)))
                        .child(
                            div()
                                .caption()
                                .child(SharedString::from(message.attachments.to_string())),
                        )
                }))
                .child(
                    div()
                        .caption()
                        .flex_shrink_0()
                        .text_color(theme.muted)
                        .child(SharedString::from(clock(&message.at))),
                );

            let mut row = div()
                .flex()
                .flex_col()
                .w_full()
                .overflow_hidden()
                .border_b_1()
                .border_color(theme.border);
            if showing {
                row = row.bg(theme.raised);
            }
            rows = rows.child(row.child(head));
        }

        div()
            .flex()
            .flex_col()
            .flex_1()
            .h_full()
            .min_w_0()
            .overflow_hidden()
            .child(
                self.page_header(
                    "Mail",
                    Some(
                        div()
                            .flex()
                            .items_center()
                            .gap_2()
                            .child(
                                div()
                                    .label()
                                    .text_color(theme.muted)
                                    .child(SharedString::from(if self.unread == 0 {
                                        format!("{} caught", self.mail.len())
                                    } else {
                                        format!("{} unread of {}", self.unread, self.mail.len())
                                    })),
                            )
                            .children((!self.mail.is_empty()).then(|| {
                                self.quiet("clear-mail", "Clear").on_click(cx.listener(
                                    |skep, _, _, cx| {
                                        skep.opened = None;
                                        let _ = skep.commands.send(Command::ClearMail);
                                        cx.notify();
                                    },
                                ))
                            }))
                            .into_any_element(),
                    ),
                    cx,
                ),
            )
            .child(self.mail_columns())
            .child(
                div()
                    .relative()
                    .flex()
                    .flex_col()
                    .flex_1()
                    .min_h_0()
                    .child(
                        div()
                            .id("mail-list")
                            .flex()
                            .flex_col()
                            .size_full()
                            .overflow_y_scroll()
                            .track_scroll(&self.mail_scroll)
                            .child(rows),
                    )
                    .children(self.scrollbar()),
            )
            .children(self.opened.as_ref().map(|body| self.reading(body, cx)))
            .into_any_element()
    }

    /// One thing the message uses that is not supported everywhere. Shut, it
    /// is the name and how widely it works. Open, it is which clients fall
    /// short, in what way, and what the support database says about it.
    pub(super) fn warning(
        &self,
        index: usize,
        warning: &comb_services::mail::Warning,
        cx: &mut Context<Self>,
    ) -> AnyElement {
        let theme = &self.theme;
        let open = self.open_warning == Some(index);

        let head = div()
            .id(("warning", index))
            .flex()
            .items_center()
            .gap_3()
            .w_full()
            .min_w_0()
            .py_1p5()
            .cursor_pointer()
            .hover(|style| style.text_color(theme.text))
            .on_click(cx.listener(move |skep, _, _, cx| {
                skep.open_warning = if open { None } else { Some(index) };
                cx.notify();
            }))
            .child(
                div()
                    .label()
                    .min_w_0()
                    .truncate()
                    .child(SharedString::from(warning.what.clone())),
            )
            .child(
                div()
                    .caption()
                    .flex_shrink_0()
                    .text_color(theme.muted)
                    .child(SharedString::from(format!(
                        "{} · used {}×",
                        warning.category, warning.found
                    ))),
            )
            .child(div().flex_1())
            .child(div().w(px(90.)).flex_shrink_0().child(self.ratio(
                warning.supported,
                warning.partial,
                warning.unsupported,
            )))
            .child(
                div()
                    .caption()
                    .w(px(34.))
                    .flex_shrink_0()
                    .text_color(theme.muted)
                    .child(SharedString::from(format!("{:.0}%", warning.supported))),
            );

        let mut row = div()
            .flex()
            .flex_col()
            .w_full()
            .min_w_0()
            .border_b_1()
            .border_color(theme.border)
            .child(head);

        if open {
            let mut detail = div().flex().flex_col().gap_2().w_full().pb_3();
            if !warning.description.is_empty() {
                detail = detail.child(
                    div()
                        .caption()
                        .text_color(theme.muted)
                        .child(SharedString::from(warning.description.clone())),
                );
            }

            // The note first, and in full ink. It is the sentence that says
            // what happens instead, which is the thing anybody opened this to
            // find out. The clients it applies to sit under it, quieter,
            // because they are the scope of the sentence rather than the point
            // of it.
            for note in &warning.notes {
                let names: Vec<String> = note
                    .clients
                    .iter()
                    .map(|client| client.name.clone())
                    .collect();
                detail =
                    detail.child(
                        div()
                            .flex()
                            .flex_col()
                            .gap_0p5()
                            .w_full()
                            .min_w_0()
                            .pl_3()
                            .border_l_2()
                            .border_color(theme.border)
                            .child(div().label().child(SharedString::from(note.says.clone())))
                            .child(div().caption().text_color(theme.muted).child(
                                SharedString::from(format!(
                                    "{} {}",
                                    names.len(),
                                    if names.len() == 1 {
                                        "client"
                                    } else {
                                        "clients"
                                    }
                                )),
                            ))
                            .child(
                                div()
                                    .caption()
                                    .text_color(theme.muted)
                                    .child(SharedString::from(names.join(", "))),
                            ),
                    );
            }

            if !warning.silent.is_empty() {
                let (partial, none): (Vec<_>, Vec<_>) =
                    warning.silent.iter().partition(|client| client.partial);
                for (title, group) in [("no support", &none), ("partial", &partial)] {
                    if group.is_empty() {
                        continue;
                    }
                    let names: Vec<String> =
                        group.iter().map(|client| client.name.clone()).collect();
                    detail = detail.child(
                        div()
                            .flex()
                            .flex_col()
                            .gap_0p5()
                            .w_full()
                            .min_w_0()
                            .child(div().caption().text_color(theme.muted).child(
                                SharedString::from(format!("{title}, with nothing said about why")),
                            ))
                            .child(
                                div()
                                    .caption()
                                    .text_color(theme.muted)
                                    .child(SharedString::from(names.join(", "))),
                            ),
                    );
                }
            }

            if !warning.url.is_empty() {
                let url = warning.url.clone();
                detail = detail.child(
                    div()
                        .id(("warning-url", index))
                        .caption()
                        .cursor_pointer()
                        .text_color(theme.accent)
                        .on_click(move |_, _, cx| cx.open_url(&url))
                        .child(SharedString::from("where this comes from")),
                );
            }
            row = row.child(detail);
        }

        row.into_any_element()
    }

    /// The same three parts as the summary, drawn small enough to sit in a
    /// row. No key: the summary above already named the shades.
    pub(super) fn ratio(&self, supported: f32, partial: f32, unsupported: f32) -> AnyElement {
        let theme = &self.theme;
        let total = (supported + partial + unsupported).max(0.01);
        let mut bar = div().flex().w_full().h(px(4.)).gap_0p5();
        for (amount, ink) in [
            (supported, theme.text),
            (partial, theme.muted),
            (unsupported, theme.idle),
        ] {
            if amount > 0. {
                bar = bar.child(
                    div()
                        .h_full()
                        .w(gpui::relative(amount / total))
                        .rounded_full()
                        .bg(ink),
                );
            }
        }
        bar.into_any_element()
    }

    /// How widely the message is supported, as a bar rather than three
    /// numbers in a row.
    ///
    /// The three parts are ordered rather than merely different, so they are
    /// drawn as one ink getting fainter rather than as three colours. That is
    /// also what keeps status colour where it belongs, which is in a row's
    /// dot and nowhere else.
    pub(super) fn support(&self, clients: &comb_services::mail::Compatibility) -> AnyElement {
        let theme = &self.theme;
        let total = (clients.supported + clients.partial + clients.unsupported).max(0.01);
        let parts = [
            ("supported", clients.supported, theme.text),
            ("partial", clients.partial, theme.muted),
            ("unsupported", clients.unsupported, theme.idle),
        ];

        let mut bar = div().flex().w_full().h(px(8.)).gap_0p5();
        for (_, amount, ink) in parts {
            if amount <= 0. {
                continue;
            }
            bar = bar.child(
                div()
                    .h_full()
                    .w(gpui::relative(amount / total))
                    .rounded_full()
                    .bg(ink),
            );
        }

        // Each part is named beside its own mark: the shade alone is not
        // enough to tell anyone which is which.
        let mut key = div().flex().items_center().gap_4().flex_wrap();
        for (name, amount, ink) in parts {
            key = key.child(
                div()
                    .flex()
                    .items_center()
                    .gap_1p5()
                    .child(div().size(px(6.)).rounded_full().bg(ink))
                    .child(
                        div()
                            .caption()
                            .text_color(theme.muted)
                            .child(SharedString::from(format!("{name} {amount:.0}%"))),
                    ),
            );
        }

        div()
            .flex()
            .flex_col()
            .gap_2()
            .w_full()
            .child(div().label().child(SharedString::from(format!(
                "{:.0}% supported across {} client tests",
                clients.supported, clients.tests
            ))))
            .child(bar)
            .child(key)
            .into_any_element()
    }

    /// A sender's own mark: a small comb of cells, filled from the address
    /// itself so the same sender always draws the same shape. Mirrored down
    /// the middle, because a symmetric pattern reads as made rather than as
    /// noise.
    ///
    /// It carries no information a person could not get from the address. What
    /// it gives is a shape to recognise, so a list of mail stops being a wall
    /// of words.
    pub(super) fn sender_mark(&self, from: &str) -> AnyElement {
        let bits = fingerprint(from);
        let ink = self.theme.muted;

        let mut comb = div().flex().flex_col().gap_px().flex_shrink_0();
        for row in 0..3 {
            let mut across = div().flex().gap_px();
            for column in 0..3 {
                // The outer columns are the same, so the mark has an axis.
                let bit = row * 2 + column.min(2 - column);
                let filled = bits >> bit & 1 == 1;
                across = across.child(div().size(px(4.)).rounded(px(CHIP)).bg(if filled {
                    ink
                } else {
                    gpui::transparent_black()
                }));
            }
            comb = comb.child(across);
        }
        comb.into_any_element()
    }

    /// What each column of the list is. Without these the times read as
    /// numbers and the sender reads as just another line of text.
    pub(super) fn mail_columns(&self) -> AnyElement {
        let theme = &self.theme;
        let label = |text: &'static str| {
            div()
                .caption()
                .text_color(theme.muted)
                .child(SharedString::from(text))
        };
        div()
            .flex()
            .items_center()
            .gap_3()
            .w_full()
            .px(px(MARGIN))
            .py_1()
            .flex_shrink_0()
            .border_b_1()
            .border_color(theme.border)
            .child(div().size(px(6.)).flex_shrink_0())
            .child(div().w(px(14.)).flex_shrink_0())
            .child(label("From").w(px(160.)))
            .child(label("Subject").w(px(220.)))
            .child(label("Preview").flex_1().min_w_0())
            .child(label("Time").flex_shrink_0())
            .into_any_element()
    }

    /// How far down the list is, drawn. gpui has no scrollbar of its own at
    /// this revision, so this is the scroll handle's numbers made visible.
    pub(super) fn scrollbar(&self) -> Option<AnyElement> {
        // Worked in plain numbers: pixels do not divide into each other.
        let viewport = f32::from(self.mail_scroll.bounds().size.height);
        let hidden = f32::from(self.mail_scroll.max_offset().y);
        if hidden <= 1. || viewport <= 0. {
            return None;
        }

        let content = viewport + hidden;
        let tall = (viewport / content * viewport).max(28.);
        let travelled = (-f32::from(self.mail_scroll.offset().y) / hidden).clamp(0., 1.);
        let top = (viewport - tall) * travelled;

        Some(
            div()
                .absolute()
                .top_0()
                .right_0()
                .w(px(10.))
                .h_full()
                .child(
                    div()
                        .absolute()
                        .top(px(top))
                        .right(px(2.))
                        .w(px(4.))
                        .h(px(tall))
                        .rounded_full()
                        .bg(self.theme.idle),
                )
                .into_any_element(),
        )
    }

    /// Where a message is read. A fixed pane rather than a row that grows,
    /// because what fills it is a native view: gpui cannot clip it to a
    /// scrolling list and cannot draw anything over it.
    pub(super) fn reading(
        &self,
        body: &comb_services::mail::Body,
        cx: &mut Context<Self>,
    ) -> AnyElement {
        let theme = &self.theme;

        let shown = match self.mail_view {
            MailView::Rendered if body.html.is_empty() => self.message(body, cx),
            MailView::Rendered => self.rendered(body, cx),
            MailView::Source => self.source_view(),
            MailView::Checks => self.checks_view(cx),
        };

        div()
            .flex()
            .flex_col()
            .w_full()
            .min_w_0()
            .flex_1()
            .min_h_0()
            // A message and the list of them get the same room, and the seam
            // between them is a band rather than a hairline: they are two
            // different things, not two halves of one.
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
                    .px(px(MARGIN))
                    .py_3()
                    .flex_shrink_0()
                    .child(
                        // The message gets a face: its mark, its subject at
                        // the size of a thing being read rather than a thing
                        // being listed, and who it is from underneath.
                        div()
                            .flex()
                            .items_center()
                            .gap_3()
                            .min_w_0()
                            .child(self.sender_mark(&body.from))
                            .child(
                                div()
                                    .flex()
                                    .flex_col()
                                    .gap_0p5()
                                    .min_w_0()
                                    .child(
                                        div()
                                            .min_w_0()
                                            .truncate()
                                            .title()
                                            .child(SharedString::from(body.subject.clone())),
                                    )
                                    .child(
                                        div()
                                            .min_w_0()
                                            .truncate()
                                            .caption()
                                            .text_color(theme.muted)
                                            .child(SharedString::from(body.from.clone())),
                                    ),
                            ),
                    )
                    .child(
                        div()
                            .flex()
                            .items_center()
                            .gap_3()
                            .flex_shrink_0()
                            .child(self.mail_tabs(body, cx))
                            .child(self.copy_message(body, cx))
                            .child(self.close_message(cx)),
                    ),
            )
            .child(shown)
            .into_any_element()
    }

    pub(super) fn mail_tabs(
        &self,
        body: &comb_services::mail::Body,
        cx: &mut Context<Self>,
    ) -> AnyElement {
        let theme = &self.theme;
        // A track with the chosen one raised out of it, rather than three
        // words that happen to sit together. The track is what says these are
        // one choice with three positions.
        let mut tabs = div()
            .flex()
            .items_center()
            .gap_0p5()
            .p(px(2.))
            .rounded(px(CARD))
            .bg(theme.base);

        for (which, name) in [
            (MailView::Rendered, "Rendered"),
            (MailView::Source, "Source"),
            (MailView::Checks, "Checks"),
        ] {
            // A plain text message has nothing to render differently, so the
            // rendered tab would be a lie about there being a choice.
            if which == MailView::Rendered && body.html.is_empty() {
                continue;
            }
            let here = self.mail_view == which;
            let id = body.id.clone();
            tabs = tabs.child(
                div()
                    .id(SharedString::from(format!("mail-tab-{name}")))
                    .px_2p5()
                    .py_1()
                    .rounded(px(CARD - 2.))
                    .label()
                    .cursor_pointer()
                    .text_color(if here { theme.text } else { theme.muted })
                    .bg(if here {
                        theme.raised
                    } else {
                        gpui::transparent_black()
                    })
                    .hover(|style| style.text_color(theme.text))
                    .on_click(cx.listener(move |skep, _, _, cx| {
                        skep.mail_view = which;
                        // Asked for on the way in rather than kept fresh: the
                        // source never changes and the checks reach out over
                        // the network.
                        // Choosing a view is the asking. Making somebody then
                        // press a button to get what the view is for is one
                        // act too many, and the rule was never that checks
                        // should be hard to reach: it was that opening a
                        // message must not reach out on its own. It still
                        // does not.
                        match which {
                            MailView::Source if skep.source.is_none() => {
                                let _ = skep.commands.send(Command::MailSource(id.clone()));
                            }
                            MailView::Checks if skep.checks.is_none() => {
                                let _ = skep.commands.send(Command::MailChecks(id.clone()));
                            }
                            _ => {}
                        }
                        cx.notify();
                    }))
                    .child(SharedString::from(name)),
            );
        }
        tabs.into_any_element()
    }

    /// The message as it renders, with a word about what was held back to make
    /// that safe.
    pub(super) fn rendered(
        &self,
        body: &comb_services::mail::Body,
        cx: &mut Context<Self>,
    ) -> AnyElement {
        let preview = self.preview.clone();
        let mut pane = div().flex().flex_col().flex_1().min_h_0().w_full();

        if !self.images_shown && (body.images > 0 || body.pixels > 0) {
            pane = pane.child(self.held_back(body, cx));
        }

        pane.child(
            div().flex_1().min_h_0().w_full().child(
                gpui::canvas(
                    |_, _, _| (),
                    move |bounds, _, _, _| {
                        if let Some(preview) = preview.borrow().as_ref() {
                            preview.place(bounds);
                        }
                    },
                )
                .size_full(),
            ),
        )
        .into_any_element()
    }

    /// What was not loaded, said plainly, with the way to change it. A pixel
    /// is named apart from an image because it is the reason for the rule.
    pub(super) fn held_back(
        &self,
        body: &comb_services::mail::Body,
        cx: &mut Context<Self>,
    ) -> AnyElement {
        let theme = &self.theme;
        let images = body.images;
        let pixels = body.pixels;
        let counted =
            |n: usize, one: &str, many: &str| format!("{n} {}", if n == 1 { one } else { many });
        let said = match (images, pixels) {
            (0, p) => counted(p, "tracking pixel", "tracking pixels"),
            (i, 0) => counted(i, "image", "images"),
            (i, p) => format!(
                "{} and {}",
                counted(i, "image", "images"),
                counted(p, "tracking pixel", "tracking pixels")
            ),
        };
        let id = body.id.clone();

        div()
            .flex()
            .items_center()
            .justify_between()
            .gap_3()
            .w_full()
            .px(px(MARGIN))
            .py_2()
            .flex_shrink_0()
            .bg(theme.base)
            .child(
                div()
                    .caption()
                    .min_w_0()
                    .text_color(theme.muted)
                    .child(SharedString::from(format!("{said} not loaded"))),
            )
            .child(
                div()
                    .id("load-images")
                    .caption()
                    .flex_shrink_0()
                    .cursor_pointer()
                    .text_color(theme.accent)
                    .on_click(cx.listener(move |skep, _, _, cx| {
                        skep.images_shown = true;
                        let _ = skep.commands.send(Command::ShowImages(id.clone()));
                        cx.notify();
                    }))
                    .child(SharedString::from("load them")),
            )
            .into_any_element()
    }

    /// The message exactly as it arrived. Every line copyable, because this is
    /// the view somebody is reading to find out what went wrong.
    pub(super) fn source_view(&self) -> AnyElement {
        let theme = &self.theme;
        let Some(source) = &self.source else {
            return self.nothing("reading the message").into_any_element();
        };

        let mut lines = div().flex().flex_col().w_full();
        for line in source.lines() {
            lines = lines.child(
                div()
                    .w_full()
                    .min_w_0()
                    .caption()
                    .font_family(MONO)
                    .text_color(theme.muted)
                    .child(SharedString::from(line.to_string())),
            );
        }

        div()
            .id("mail-source")
            .flex()
            .flex_col()
            .flex_1()
            .min_h_0()
            .w_full()
            .px(px(MARGIN))
            .py_2()
            .overflow_y_scroll()
            .child(lines)
            .into_any_element()
    }

    /// How the message would fare elsewhere. Coming to this view is what asks
    /// for it, which is a deliberate act; opening a message still reaches out
    /// to nothing at all, which was the promise the rendered view makes.
    pub(super) fn checks_view(&self, cx: &mut Context<Self>) -> AnyElement {
        let theme = &self.theme;
        if self.opened.is_none() {
            return self.nothing("no message").into_any_element();
        }

        let Some(found) = &self.checks else {
            return div()
                .flex()
                .flex_col()
                .flex_1()
                .min_h_0()
                .w_full()
                .px(px(MARGIN))
                .py_3()
                .gap_1()
                .child(self.nothing("checking"))
                .child(
                    div()
                        .caption()
                        .text_color(theme.muted)
                        .child(SharedString::from(
                            "Testing how this message renders in real mail clients, and following \
                         every link in it to see whether it answers.",
                        )),
                )
                .into_any_element();
        };

        let (clients, links) = found.as_ref();
        let mut out = div().flex().flex_col().w_full().gap_3();

        out = out.child(self.support(clients));

        if !clients.warnings.is_empty() {
            out = out.child(
                div()
                    .caption()
                    .text_color(theme.muted)
                    .child(SharedString::from(format!(
                        "{} of the things this message uses are not supported everywhere",
                        clients.warnings.len()
                    ))),
            );
        }

        for (index, warning) in clients.warnings.iter().enumerate() {
            out = out.child(self.warning(index, warning, cx));
        }

        out = out.child(
            div()
                .label()
                .child(SharedString::from(if links.errors == 0 {
                    format!("{} links, all answering", links.links.len())
                } else {
                    format!(
                        "{} of {} links did not answer",
                        links.errors,
                        links.links.len()
                    )
                })),
        );

        for link in links.links.iter().take(12) {
            let bad = link.status >= 400 || link.status == 0;
            out = out.child(
                div()
                    .flex()
                    .items_center()
                    .justify_between()
                    .gap_3()
                    .w_full()
                    .min_w_0()
                    .child(
                        div()
                            .caption()
                            .font_family(MONO)
                            .min_w_0()
                            .truncate()
                            .text_color(theme.muted)
                            .child(SharedString::from(link.url.clone())),
                    )
                    .child(
                        div()
                            .caption()
                            .flex_shrink_0()
                            .text_color(if bad { theme.failed } else { theme.running })
                            .child(SharedString::from(if link.said.is_empty() {
                                link.status.to_string()
                            } else {
                                link.said.clone()
                            })),
                    ),
            );
        }

        div()
            .id("mail-checks")
            .flex()
            .flex_col()
            .flex_1()
            .min_h_0()
            .w_full()
            .px(px(MARGIN))
            .py_3()
            .overflow_y_scroll()
            .child(out)
            .into_any_element()
    }

    pub(super) fn close_message(&self, cx: &mut Context<Self>) -> AnyElement {
        self.quiet("close-message", "Close")
            .on_click(cx.listener(|skep, _, _, cx| {
                skep.opened = None;
                cx.notify();
            }))
            .into_any_element()
    }

    /// The message itself, in the same monospaced treatment the logs get:
    /// what was sent is closer to output than to prose.
    ///
    /// gpui has no text selection at this revision, so nothing here can be
    /// dragged over. Every line is a click instead, and any link in the
    /// message is pulled out and made one of its own, because a link or a code
    /// is what anyone is actually after.
    pub(super) fn message(
        &self,
        body: &comb_services::mail::Body,
        cx: &mut Context<Self>,
    ) -> AnyElement {
        let theme = &self.theme;

        let mut lines = div().flex().flex_col().w_full().gap_0p5();
        for (place, line) in body.text.lines().enumerate() {
            let done = self
                .copied
                .is_some_and(|(what, _)| what == Copied::Piece(place));
            let words = line.to_string();
            let mut shown = div()
                .id(("mail-line", place))
                .w_full()
                .min_w_0()
                .px_1()
                .rounded(px(CHIP))
                .label()
                .font_family(MONO)
                .cursor_pointer();
            if done {
                shown = shown.bg(theme.base);
            }
            lines = lines.child(
                shown
                    .hover(|style| style.bg(theme.base))
                    .on_click(cx.listener(move |skep, _, _, cx| {
                        cx.write_to_clipboard(ClipboardItem::new_string(words.clone()));
                        skep.copied = Some((Copied::Piece(place), Instant::now()));
                        cx.notify();
                    }))
                    .child(SharedString::from(line.to_string())),
            );
        }

        let mut aside = div().flex().flex_col().w_full().gap_1();
        // The pane's own header carries the copy action, so this says only
        // what the header does not.
        aside = aside.child(
            div()
                .caption()
                .text_color(theme.muted)
                .child(SharedString::from(format!("to {}", body.to.join(", ")))),
        );
        if body.converted {
            aside = aside.child(
                div()
                    .caption()
                    .text_color(theme.muted)
                    .child(SharedString::from("sent as html, read here as text")),
            );
        }
        if !body.attachments.is_empty() {
            aside = aside.child(
                div()
                    .caption()
                    .text_color(theme.muted)
                    .child(SharedString::from(format!(
                        "attached: {}",
                        body.attachments.join(", ")
                    ))),
            );
        }

        // Links get their own row, because a url wrapped in a sentence is the
        // one thing that most needs copying on its own.
        for (place, link) in links(&body.text).into_iter().enumerate() {
            let done = self
                .copied
                .is_some_and(|(what, _)| what == Copied::Piece(usize::MAX - place));
            let target = link.clone();
            aside = aside.child(
                div()
                    .id(("mail-link", place))
                    .flex()
                    .items_center()
                    .gap_2()
                    .max_w_full()
                    .min_w_0()
                    .px_2()
                    .py_1()
                    .rounded(px(CARD))
                    .cursor_pointer()
                    .bg(theme.base)
                    .hover(|style| style.bg(theme.border))
                    .on_click(cx.listener(move |skep, _, _, cx| {
                        cx.write_to_clipboard(ClipboardItem::new_string(target.clone()));
                        skep.copied = Some((Copied::Piece(usize::MAX - place), Instant::now()));
                        cx.notify();
                    }))
                    .child(
                        div()
                            .min_w_0()
                            .truncate()
                            .caption()
                            .font_family(MONO)
                            .child(SharedString::from(link)),
                    )
                    .child(
                        div()
                            .caption()
                            .flex_shrink_0()
                            .text_color(if done { theme.text } else { theme.muted })
                            .child(SharedString::from(if done { "copied" } else { "copy" })),
                    ),
            );
        }

        div()
            .id("message-body")
            .flex()
            .flex_col()
            .flex_1()
            .min_h_0()
            .w_full()
            .min_w_0()
            .px(px(MARGIN))
            .pb_3()
            .gap_2()
            .overflow_y_scroll()
            .child(aside)
            .child(lines)
            .into_any_element()
    }

    pub(super) fn copy_message(
        &self,
        body: &comb_services::mail::Body,
        cx: &mut Context<Self>,
    ) -> AnyElement {
        let done = self.copied.is_some_and(|(what, _)| what == Copied::Message);
        let text = body.text.clone();
        // The label is the whole acknowledgement. A control that also changes
        // colour to say it worked is saying it twice.
        self.quiet("copy-message", if done { "Copied" } else { "Copy" })
            .on_click(cx.listener(move |skep, _, _, cx| {
                cx.write_to_clipboard(ClipboardItem::new_string(text.clone()));
                skep.copied = Some((Copied::Message, Instant::now()));
                cx.notify();
            }))
            .into_any_element()
    }
}

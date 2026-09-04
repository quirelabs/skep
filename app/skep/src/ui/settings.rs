//! The Settings page, in named sections.

use super::*;

/// What the screen needs to say about certificates. Not "trusted" or not, but
/// which authority, out of which home, with which fingerprint: two homes make
/// two authorities carrying the same name, and only the last of those tells
/// them apart.
pub(super) struct Trust {
    pub(super) home: String,
    pub(super) root: String,
    pub(super) fingerprint: String,
    pub(super) trusted: bool,
}

impl Skep {
    /// Named sections rather than a list of everything skep knows. Each one
    /// says what it is for before it says what it holds, and every value is
    /// honest about whether anybody chose it.
    pub(super) fn settings(&self, cx: &mut Context<Self>) -> AnyElement {
        div()
            .flex()
            .flex_col()
            .flex_1()
            .h_full()
            .overflow_hidden()
            .child(self.page_header("Settings", Some(self.open_settings(cx)), cx))
            .child(
                div()
                    .id("settings-list")
                    .flex()
                    .flex_col()
                    .flex_1()
                    .w_full()
                    .min_w_0()
                    .overflow_y_scroll()
                    .child(self.behaviour(cx))
                    .child(self.certificates(cx))
                    .child(self.service_settings()),
            )
            .into_any_element()
    }

    /// What the app does, as opposed to what it holds. Written to
    /// config.toml, so it is the machine's preference and outlives the window.
    pub(super) fn behaviour(&self, cx: &mut Context<Self>) -> AnyElement {
        div()
            .flex()
            .flex_col()
            .w_full()
            .child(self.section(
                "Behaviour",
                "How this window acts. Kept in config.toml, beside everything else it remembers.",
                true,
            ))
            .child(self.choice(
                "Open sites in the browser",
                "A site opens in your own browser instead of the pane beside the list. The \
                 pictures are taken either way.",
                self.sites_in_browser,
                cx,
            ))
            .into_any_element()
    }

    /// A preference, its explanation, and the switch that sets it.
    pub(super) fn choice(
        &self,
        name: &'static str,
        about: &'static str,
        on: bool,
        cx: &mut Context<Self>,
    ) -> impl IntoElement {
        let theme = &self.theme;
        div()
            .id(SharedString::from(name))
            .group("choice")
            .relative()
            // The same mark every other row in the window wears when the
            // pointer is on it: an edge, not a light across the middle.
            .child(
                div()
                    .absolute()
                    .left_0()
                    .top_0()
                    .bottom_0()
                    .w(px(2.))
                    .bg(theme.accent)
                    .opacity(0.)
                    .group_hover("choice", |style| style.opacity(1.)),
            )
            .flex()
            .items_start()
            .justify_between()
            .gap_4()
            .w_full()
            .px(px(MARGIN))
            .py_3()
            .cursor_pointer()
            .hover(|style| style.bg(theme.raised))
            .on_click(cx.listener(move |skep, _, _, cx| {
                // Said once, to the one place that writes it down. The window
                // waits to be told what the file now says rather than
                // assuming, so a failed write cannot leave a switch lying.
                let _ = skep.commands.send(Command::Prefer("sites_in_browser", !on));
                cx.notify();
            }))
            .child(
                div()
                    .flex()
                    .flex_col()
                    .gap_0p5()
                    .flex_1()
                    .min_w_0()
                    .child(div().label().child(SharedString::from(name)))
                    .child(
                        div()
                            .caption()
                            .text_color(theme.muted)
                            .child(SharedString::from(about)),
                    ),
            )
            .child(
                // A switch rather than a tick: it is a thing with two
                // positions, and the knob moving across says which.
                div()
                    .flex()
                    .items_center()
                    .flex_shrink_0()
                    .w(px(34.))
                    .h(px(20.))
                    .mt_0p5()
                    .px(px(2.))
                    .rounded_full()
                    .bg(if on { theme.accent } else { theme.raised })
                    .border_1()
                    .border_color(if on { theme.accent } else { theme.border })
                    // The spacer leads when it is on, so the knob is where
                    // the eye expects it: left for off, right for on.
                    .children(on.then(|| div().flex_1()))
                    .child(div().size(px(14.)).rounded_full().bg(theme.base))
                    .children((!on).then(|| div().flex_1())),
            )
    }

    /// A section's name and what it is for. Everything below it belongs to it
    /// until the next rule, which is what the rule is for: whitespace alone
    /// left three groups looking like one long list.
    pub(super) fn section(
        &self,
        title: &'static str,
        about: &'static str,
        first: bool,
    ) -> impl IntoElement {
        let mut band = div()
            .flex()
            .flex_col()
            .gap_0p5()
            .w_full()
            .px(px(MARGIN))
            .pt_5()
            .pb_2();
        if !first {
            band = band.mt_2().border_t_1().border_color(self.theme.border);
        }
        band.child(div().title().child(SharedString::from(title)))
            .child(
                div()
                    .caption()
                    .text_color(self.theme.muted)
                    .child(SharedString::from(about)),
            )
    }

    /// One fact and its value, with the value set apart so a column of them
    /// reads down rather than across.
    pub(super) fn fact(&self, name: &'static str, value: String, mono: bool) -> impl IntoElement {
        let mut shown = div()
            .flex_1()
            .min_w_0()
            .truncate()
            .label()
            .child(SharedString::from(value));
        if mono {
            shown = shown.font_family(MONO);
        }
        div()
            .flex()
            .items_center()
            .gap_3()
            .w_full()
            .min_w_0()
            .px(px(MARGIN))
            .py_1p5()
            .child(
                div()
                    .w(px(120.))
                    .flex_shrink_0()
                    .caption()
                    .text_color(self.theme.muted)
                    .child(SharedString::from(name)),
            )
            .child(shown)
    }

    /// Which authority, out of which home, and whether this machine accepts
    /// it. Naming the home is the whole point: two of them make two
    /// authorities with the same name, and the difference is invisible until
    /// something refuses a certificate that looks perfectly good.
    pub(super) fn certificates(&self, cx: &mut Context<Self>) -> AnyElement {
        let theme = &self.theme;
        let mut out = div().flex().flex_col().w_full().child(self.section(
            "Certificates",
            "The authority skep signs local sites with. A browser accepts a site only if this \
             machine trusts this authority.",
            false,
        ));

        let Some(trust) = &self.trust else {
            return out
                .child(self.fact("state", "no authority yet".to_string(), false))
                .into_any_element();
        };

        out = out
            .child(
                div()
                    .flex()
                    .items_center()
                    .gap_3()
                    .w_full()
                    .px(px(MARGIN))
                    .py_1p5()
                    .child(
                        div()
                            .w(px(120.))
                            .flex_shrink_0()
                            .caption()
                            .text_color(theme.muted)
                            .child(SharedString::from("trusted")),
                    )
                    .child(
                        div()
                            .size(px(6.))
                            .rounded_full()
                            .flex_shrink_0()
                            .bg(if trust.trusted {
                                theme.running
                            } else {
                                theme.failed
                            }),
                    )
                    .child(div().label().child(SharedString::from(if trust.trusted {
                        "yes, this machine accepts it"
                    } else {
                        "no, browsers will refuse these sites"
                    }))),
            )
            .child(self.fact("home", trust.home.clone(), true))
            .child(self.fact("authority", trust.root.clone(), true))
            .child(self.fact("fingerprint", trust.fingerprint.clone(), true));

        if !trust.trusted {
            // Trusting it writes to the system keychain, which needs an
            // administrator, which an app cannot become. Saying exactly what to
            // run is the honest offer.
            let command = "sudo skep trust".to_string();
            out = out.child(
                div()
                    .flex()
                    .items_center()
                    .gap_3()
                    .w_full()
                    .px(px(MARGIN))
                    .py_2()
                    .child(div().w(px(120.)).flex_shrink_0())
                    .child(
                        div()
                            .id("copy-trust")
                            .flex()
                            .items_center()
                            .gap_2()
                            .px_2()
                            .py_1()
                            .rounded(px(CARD))
                            .bg(theme.base)
                            .cursor_pointer()
                            .hover(|style| style.border_color(theme.accent))
                            .border_1()
                            .border_color(theme.border)
                            .on_click(cx.listener(move |skep, _, _, cx| {
                                cx.write_to_clipboard(ClipboardItem::new_string(
                                    "sudo skep trust".to_string(),
                                ));
                                skep.copied = Some((Copied::Everything, Instant::now()));
                                cx.notify();
                            }))
                            .child(
                                div()
                                    .label()
                                    .font_family(MONO)
                                    .child(SharedString::from(command)),
                            )
                            .child(
                                div()
                                    .caption()
                                    .text_color(theme.muted)
                                    .child(SharedString::from("copy")),
                            ),
                    ),
            );
        }

        out.into_any_element()
    }

    /// What each service is set to, and by whom. A value nobody chose says so,
    /// because "default" and "somebody decided this" are different facts and
    /// the screen used to show them the same way.
    pub(super) fn service_settings(&self) -> AnyElement {
        let theme = &self.theme;
        let services: Vec<_> = self.mirror.services().cloned().collect();

        let mut out = div().flex().flex_col().w_full().pb_6().child(self.section(
            "Ports and versions",
            "Set in config.toml. A project's skep.toml wins wherever both speak, so a \
             repository always gets what it asks for.",
            false,
        ));

        let last = services.len().saturating_sub(1);
        for (index, status) in services.into_iter().enumerate() {
            let mut ports = div().flex().flex_col().gap_0p5().flex_1().min_w_0();
            for (name, number) in &status.ports {
                let source = status.ports_from.get(name);
                ports =
                    ports.child(
                        div()
                            .flex()
                            .items_baseline()
                            .gap_2()
                            .child(
                                div()
                                    .label()
                                    .font_family(MONO)
                                    .child(SharedString::from(format!("{name} {number}"))),
                            )
                            .child(div().caption().text_color(theme.muted).child(
                                SharedString::from(match source {
                                    Some(from) => format!("set in {from}"),
                                    None => "default".to_string(),
                                }),
                            )),
                    );
            }

            // A rule between services, and none under the last of them: a line
            // with nothing after it is a promise the page does not keep.
            let mut row = div()
                .flex()
                .w_full()
                .min_w_0()
                .items_start()
                .gap_3()
                .px(px(MARGIN))
                .py_3();
            if index < last {
                row = row.border_b_1().border_color(theme.border);
            }
            out = out.child(
                row.child(
                    div()
                        .w(px(120.))
                        .flex_shrink_0()
                        .truncate()
                        .child(SharedString::from(status.id.service.as_str().to_string())),
                )
                .child(ports),
            );
        }
        out.into_any_element()
    }

    pub(super) fn open_settings(&self, cx: &mut Context<Self>) -> AnyElement {
        self.chip("open-settings", "Open config.toml")
            .on_click(cx.listener(|skep, _, _, cx| {
                skep.reveal_settings();
                cx.notify();
            }))
            .into_any_element()
    }

    /// Writes a commented starting point if there is nothing there, then hands
    /// the file to whatever the machine opens .toml with.
    pub(super) fn reveal_settings(&mut self) {
        match comb_services::project::ensure_settings(&comb::Paths::from_env()) {
            Ok(path) => {
                if let Err(error) = std::process::Command::new("open").arg(&path).spawn() {
                    self.problem =
                        Some(format!("could not open {}: {error}", path.display()).into());
                }
            }
            Err(error) => self.problem = Some(error.to_string().into()),
        }
    }
}

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
            .child(
                self.page_header(
                    "Settings",
                    Some(
                        div()
                            .flex()
                            .items_center()
                            .gap_3()
                            .flex_shrink_0()
                            .child(self.settings_tabs(cx))
                            // Beside the tabs rather than on one of them: every
                            // page here is written to that file, so it belongs to
                            // the screen rather than to a section of it.
                            .child(self.open_settings(cx))
                            .into_any_element(),
                    ),
                    cx,
                ),
            )
            .child(
                div()
                    .id(SharedString::from(format!("settings-{:?}", self.tab)))
                    .flex()
                    .flex_col()
                    .flex_1()
                    .w_full()
                    .min_w_0()
                    .overflow_y_scroll()
                    .children(match self.tab {
                        Tab::General => {
                            vec![self.appearance(cx), self.behaviour(cx), self.sidebar(cx)]
                        }
                        Tab::Machine => vec![
                            self.version(cx),
                            self.command_line(cx),
                            self.certificates(cx),
                        ],
                        Tab::Services => vec![self.service_settings()],
                    }),
            )
            .into_any_element()
    }

    /// Three pages rather than one long column. What was here was every
    /// setting the app has in a single scroll, which is fine while there are
    /// four of them and stops being fine before anybody notices.
    fn settings_tabs(&self, cx: &mut Context<Self>) -> impl IntoElement {
        let mut tabs = self.track();
        for (tab, name) in [
            (Tab::General, "General"),
            (Tab::Machine, "This machine"),
            (Tab::Services, "Services"),
        ] {
            tabs = tabs.child(
                self.segment(
                    SharedString::from(format!("tab-{name}")),
                    name,
                    self.tab == tab,
                )
                .on_click(cx.listener(move |skep, _, _, cx| {
                    skep.tab = tab;
                    cx.notify();
                })),
            );
        }
        tabs
    }

    /// Which appearance to wear. The window follows the system unless it is
    /// told otherwise, which is right for nearly everybody; this is for the
    /// people who keep one appearance whatever the hour.
    pub(super) fn appearance(&self, cx: &mut Context<Self>) -> AnyElement {
        let mut choices = self.track();
        for (wearing, name) in [
            (Wearing::System, "System"),
            (Wearing::Light, "Light"),
            (Wearing::Dark, "Dark"),
        ] {
            choices = choices.child(
                self.segment(
                    SharedString::from(format!("wear-{name}")),
                    name,
                    self.wearing == wearing,
                )
                .on_click(cx.listener(move |skep, _, _, cx| {
                    // Said to the one place that writes it down, and worn when
                    // that place says what it now holds. A window that dressed
                    // itself first would be showing something the file might
                    // not agree with.
                    let _ = skep.commands.send(Command::Wear(wearing.written()));
                    cx.notify();
                })),
            );
        }

        div()
            .flex()
            .flex_col()
            .w_full()
            .child(self.section(
                "Appearance",
                "Light or dark, or whichever the system is in. Kept in config.toml with \
                 everything else this window remembers.",
                true,
            ))
            .child(
                div()
                    .flex()
                    .items_center()
                    .justify_between()
                    .gap_4()
                    .w_full()
                    .px(px(MARGIN))
                    .py_3()
                    .child(
                        div()
                            .flex()
                            .flex_col()
                            .gap_0p5()
                            .flex_1()
                            .min_w_0()
                            .child(div().label().child(SharedString::from("Theme")))
                            .child(div().caption().text_color(self.theme.muted).child(
                                SharedString::from(match self.wearing {
                                    Wearing::System => {
                                        "Following the system, and changing with it."
                                    }
                                    _ => "Held here whatever the system does.",
                                }),
                            )),
                    )
                    .child(choices),
            )
            .into_any_element()
    }

    /// Which skep this is, and whether there is a newer one.
    ///
    /// The check is a button rather than something that happens when the
    /// window opens. Every other thing in this application that reaches out
    /// does so because somebody asked it to, and an updater is not the place
    /// to make the first exception.
    pub(super) fn version(&self, cx: &mut Context<Self>) -> AnyElement {
        let theme = &self.theme;
        let mut out = div().flex().flex_col().w_full().child(self.section(
            "Version",
            "Skep updates by disk image: this fetches it, checks it against what the release \
             promised, and opens it for you to drag across.",
            false,
        ));

        out = out.child(self.fact("running", env!("CARGO_PKG_VERSION").to_string(), true));

        let (said, colour) = match &self.newer {
            Newer::Unasked => (String::new(), theme.muted),
            Newer::Looking => ("looking".to_string(), theme.muted),
            Newer::Current => ("this is the newest there is".to_string(), theme.muted),
            Newer::There(release) => (format!("{} is out", release.version), theme.text),
            Newer::Fetching => ("fetching".to_string(), theme.muted),
            Newer::Fetched => (
                "in your Downloads, and opened. Drag it across.".to_string(),
                theme.muted,
            ),
            Newer::Unknown(why) => (why.to_string(), theme.failed),
        };
        if !said.is_empty() {
            out = out.child(
                div()
                    .flex()
                    .items_center()
                    .gap_3()
                    .w_full()
                    .px(px(MARGIN))
                    .py_1p5()
                    .child(div().w(px(120.)).flex_shrink_0())
                    .child(
                        div()
                            .label()
                            .text_color(colour)
                            .child(SharedString::from(said)),
                    ),
            );
        }

        let control = match &self.newer {
            Newer::There(release) => {
                let release = release.clone();
                self.chip("get-update", "Download it").on_click(cx.listener(
                    move |skep, _, _, cx| {
                        skep.newer = Newer::Fetching;
                        let _ = skep.commands.send(Command::GetUpdate(release.clone()));
                        cx.notify();
                    },
                ))
            }
            _ => self
                .chip("look-for-update", "Check for updates")
                .on_click(cx.listener(|skep, _, _, cx| {
                    skep.newer = Newer::Looking;
                    let _ = skep.commands.send(Command::LookForUpdate);
                    cx.notify();
                })),
        };

        out.child(
            div()
                .flex()
                .items_center()
                .gap_3()
                .w_full()
                .px(px(MARGIN))
                .py_2()
                .child(div().w(px(120.)).flex_shrink_0())
                .child(control),
        )
        .into_any_element()
    }

    /// Where the command is, and how to put it somewhere if it is nowhere.
    ///
    /// The window hosts the engine, so somebody who only opens the app has a
    /// working skep already; what they do not have is the thing they type in
    /// a project, or the path an agent's config has to name. This says which
    /// of those is true right now rather than assuming either.
    pub(super) fn command_line(&self, cx: &mut Context<Self>) -> AnyElement {
        let path = std::env::var("PATH").unwrap_or_default();
        let beside = crate::tools::beside();
        let found = beside
            .as_ref()
            .map(|from| crate::tools::placed("skep", &path, &from.join("skep")));

        let mut out = div().flex().flex_col().w_full().child(self.section(
            "Command line",
            "skep in a terminal, and skep-mcp for an agent. They are links to the copies inside \
             this application, so the command and the window are never two different versions.",
            false,
        ));

        out = match &found {
            Some(crate::tools::Placed::Ours(at)) => out.child(self.fact(
                "installed",
                at.parent().unwrap_or(at).display().to_string(),
                true,
            )),
            // Named rather than replaced. A command already on PATH may not be
            // this application's to take.
            Some(crate::tools::Placed::Other(at)) => out.child(self.fact(
                "another skep",
                format!("{} is first on your PATH", at.display()),
                true,
            )),
            _ => out.child(self.fact(
                "installed",
                "not yet, so skep in a terminal will not be found".to_string(),
                false,
            )),
        };

        if !matches!(found, Some(crate::tools::Placed::Ours(_))) {
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
                        self.chip("install-cli", "Install the command")
                            .on_click(cx.listener(|skep, _, _, cx| {
                                skep.place_tools();
                                cx.notify();
                            })),
                    ),
            );
        }

        out.into_any_element()
    }

    /// Links the tools into the first place that will have them, and says
    /// where. Nothing here asks for a password: where one would be needed,
    /// the sentence is the offer, the same way trusting the authority is.
    pub(super) fn place_tools(&mut self) {
        let Some(from) = crate::tools::beside() else {
            self.problem = Some("could not find this application on disk".into());
            return;
        };
        let home = std::env::var("HOME").map(std::path::PathBuf::from);
        let Ok(home) = home else {
            self.problem = Some("no home directory to install into".into());
            return;
        };
        let places = crate::tools::candidates(&home);
        let Some(into) = crate::tools::writable(&places, &home) else {
            self.problem = Some(
                format!(
                    "nowhere writable to put it. Run: sudo ln -sf {}/skep /usr/local/bin/skep",
                    from.display()
                )
                .into(),
            );
            return;
        };
        match crate::tools::install(&from, &into) {
            Ok(into) => {
                let path = std::env::var("PATH").unwrap_or_default();
                if !crate::tools::on_path(&into, &path) {
                    // Installed, and still not findable. Saying so is the
                    // difference between a button that worked and one that
                    // appeared to.
                    self.problem = Some(
                        format!(
                            "installed in {}, which is not on your PATH. Add it with: export \
                             PATH=\"{}:$PATH\"",
                            into.display(),
                            into.display()
                        )
                        .into(),
                    );
                }
            }
            Err(error) => self.problem = Some(format!("could not install: {error}").into()),
        }
    }

    /// What the sidebar carries. Skep is designed to hold more than any one
    /// person needs, and a screen somebody never opens is clutter no matter
    /// how good it is, so putting one away is a thing they can do rather than
    /// a thing they have to ask for.
    ///
    /// Away, not off. What is hidden keeps running: mail is still caught,
    /// sites are still served. Anything else would make this a switch that
    /// silently breaks a project, and this list is here to tidy a sidebar.
    pub(super) fn sidebar(&self, cx: &mut Context<Self>) -> AnyElement {
        let mut out = div().flex().flex_col().w_full().child(self.section(
            "Sidebar",
            "Screens you do not want. What you put away keeps working, it just stops taking up \
             room, and this is where it comes back from.",
            false,
        ));

        for (name, _, page) in super::rail::RAIL {
            // Settings is not offered: it is the way back from every other
            // one of these.
            let unbuilt = page.is_none();
            let shown = !self.is_hidden(name);
            out = out.child(self.showing(name, unbuilt, shown, cx));
        }
        out.into_any_element()
    }

    /// One screen and whether it is in the sidebar. The same row and the same
    /// switch as every other preference, because it is one.
    fn showing(
        &self,
        name: &'static str,
        unbuilt: bool,
        shown: bool,
        cx: &mut Context<Self>,
    ) -> impl IntoElement {
        let theme = &self.theme;
        let mut hidden: Vec<String> = self.hidden.iter().cloned().collect();
        let key = Self::key(name);
        // The whole list, either way: what is written down is the answer, not
        // the change that produced it.
        if shown {
            hidden.push(key);
            hidden.sort();
            hidden.dedup();
        } else {
            hidden.retain(|held| held != &key);
        }

        div()
            .id(SharedString::from(format!("show-{name}")))
            .group("showing")
            .relative()
            .child(
                div()
                    .absolute()
                    .left_0()
                    .top_0()
                    .bottom_0()
                    .w(px(2.))
                    .bg(theme.accent)
                    .opacity(0.)
                    .group_hover("showing", |style| style.opacity(1.)),
            )
            .flex()
            .items_center()
            .justify_between()
            .gap_4()
            .w_full()
            .px(px(MARGIN))
            .py_2p5()
            .cursor_pointer()
            .hover(|style| style.bg(theme.raised))
            .on_click(cx.listener(move |skep, _, _, cx| {
                let _ = skep.commands.send(Command::Hide(hidden.clone()));
                cx.notify();
            }))
            .child(
                div()
                    .flex()
                    .items_center()
                    .gap_2()
                    .flex_1()
                    .min_w_0()
                    .child(div().label().child(SharedString::from(name)))
                    // An honest label on the two the rail shows dimmed. A
                    // switch for something that does not exist yet should say
                    // so rather than look broken.
                    .children(unbuilt.then(|| {
                        div()
                            .caption()
                            .text_color(theme.idle)
                            .child(SharedString::from("not built yet"))
                    })),
            )
            .child(self.switch(name, shown))
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
                false,
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
            .child(self.switch(name, on))
    }

    /// A switch rather than a tick: it is a thing with two positions, and the
    /// knob moving across says which. One of them, so no preference anywhere
    /// can invent a second shape.
    ///
    /// The knob slides and the track fills rather than both changing between
    /// one frame and the next. This is the one place in the window where
    /// motion carries meaning rather than smoothing an arrival: which way it
    /// travelled is what says which way it just went, and a switch that
    /// teleports is the control the platform is unanimous about.
    pub(super) fn switch(&self, name: &str, on: bool) -> impl IntoElement {
        let (base, raised, border, lit) = (
            self.theme.base,
            self.theme.raised,
            self.theme.border,
            self.theme.on,
        );
        div()
            .relative()
            .flex_shrink_0()
            .w(px(34.))
            .h(px(20.))
            .rounded_full()
            .border_1()
            .with_animation(
                // The state is part of the name, so this runs again every
                // time the state changes rather than once when it appears.
                SharedString::from(format!("switch-{name}-{on}")),
                Animation::new(MOTION).with_easing(ease_in_out),
                move |switch, delta| {
                    let along = if on { delta } else { 1. - delta };
                    switch
                        .bg(paint::mix(raised, lit, along))
                        .border_color(paint::mix(border, lit, along))
                        .child(
                            div()
                                .absolute()
                                .top(px(2.))
                                .left(px(2. + along * 14.))
                                .size(px(14.))
                                .rounded_full()
                                .bg(base),
                        )
                },
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
            true,
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

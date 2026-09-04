//! The Projects page: what skep runs for you, and where it serves it.

use super::paint::faded;
use gpui::{AppContext as _, Entity, Focusable as _};

use crate::field::{self, Field};

use super::*;

/// A project being described, after its folder has been chosen. The folder is
/// already decided by the time this exists, which is why it is not a field.
pub(super) struct Naming {
    pub(super) directory: std::path::PathBuf,
    pub(super) command: Entity<Field>,
    pub(super) site: Entity<Field>,
    pub(super) complaint: Option<String>,
}

impl Naming {
    pub(super) fn new(
        directory: std::path::PathBuf,
        look: field::Look,
        cx: &mut Context<Skep>,
    ) -> Self {
        // The name a project is served at is nearly always its folder plus
        // .test, so it is offered rather than asked for.
        let suggested = directory
            .file_name()
            .map(|name| {
                let cleaned: String = name
                    .to_string_lossy()
                    .to_ascii_lowercase()
                    .chars()
                    .map(|c| if c.is_ascii_alphanumeric() { c } else { '-' })
                    .collect();
                format!("{}.test", cleaned.trim_matches('-'))
            })
            .unwrap_or_default();
        let site = cx.new(|cx| Field::new("myapp.test", look, cx));
        site.update(cx, |field, cx| field.set(suggested, cx));
        Self {
            directory,
            command: cx.new(|cx| Field::new("npm run dev -- --port {port}", look, cx)),
            site,
            complaint: None,
        }
    }
}

impl Skep {
    /// Every project this machine knows about. A project is remembered the
    /// first time `skep up` runs in its directory, so this list is what you
    /// have actually worked on rather than a scan of the disk.
    pub(super) fn projects_page(&self, cx: &mut Context<Self>) -> AnyElement {
        let running = self
            .projects
            .iter()
            .filter(|project| self.project_status(&project.name).is_some())
            .count();
        let rows: Vec<_> = self
            .projects
            .iter()
            .enumerate()
            .map(|(index, project)| self.project_row(index, project, cx))
            .collect();
        let empty = rows.is_empty();

        div()
            .flex()
            .flex_col()
            .flex_1()
            .min_w_0()
            .h_full()
            .child(
                self.page_header(
                    "Projects",
                    Some(
                        div()
                            .flex()
                            .items_center()
                            .gap_3()
                            .child(div().label().text_color(self.theme.muted).child(
                                SharedString::from(format!(
                                    "{running} of {} running",
                                    self.projects.len()
                                )),
                            ))
                            .child(self.add_project(cx))
                            .into_any_element(),
                    ),
                    cx,
                ),
            )
            .child(
                div()
                    .id("projects")
                    .flex()
                    .flex_col()
                    .flex_1()
                    .min_w_0()
                    .overflow_y_scroll()
                    .children(rows)
                    .children(
                        empty.then(|| self.nothing("run skep up in a project to see it here")),
                    ),
            )
            .into_any_element()
    }

    /// The way in. A folder chooser rather than a form: a project is a
    /// directory that already exists, so the only question is which one, and
    /// the machine's own chooser answers that better than anything this app
    /// could draw.
    fn add_project(&self, cx: &mut Context<Self>) -> impl IntoElement {
        self.chip("add-project", "Add a project")
            .on_click(cx.listener(|_, _, window, cx| {
                let picked = cx.prompt_for_paths(gpui::PathPromptOptions {
                    files: false,
                    directories: true,
                    multiple: false,
                    prompt: Some(SharedString::from("Choose")),
                });
                // Choosing the folder is the first half. What to run in it is
                // the second, and it is asked for rather than left in a file
                // for somebody to find.
                let handle = window.window_handle();
                cx.spawn(async move |skep, cx| {
                    let Ok(Ok(Some(paths))) = picked.await else {
                        return;
                    };
                    let Some(directory) = paths.first().cloned() else {
                        return;
                    };
                    let _ = cx.update_window(handle, |_, window, cx| {
                        let _ = skep.update(cx, |skep, cx| {
                            let naming = Naming::new(directory, skep.writing(), cx);
                            naming.command.focus_handle(cx).focus(window, cx);
                            skep.naming = Some(naming);
                            cx.notify();
                        });
                    });
                })
                .detach();
            }))
    }

    /// The second half of adding a project: what to run in the folder that
    /// was just chosen, and the name to serve it at.
    ///
    /// Asked rather than left in a file. A template a person has to find and
    /// edit is a worse first five minutes than two fields with the usual
    /// answers already in them.
    pub(super) fn naming_form(&self, cx: &mut Context<Self>) -> Option<AnyElement> {
        let naming = self.naming.as_ref()?;
        let theme = &self.theme;

        let labelled = |name: &'static str, about: &'static str, held: &Entity<Field>| {
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
                        .px_3()
                        .py_2()
                        .rounded(px(CHIP))
                        .bg(theme.base)
                        .border_1()
                        .border_color(theme.border)
                        .body()
                        .font_family(MONO)
                        .child(held.clone()),
                )
                .child(
                    div()
                        .caption()
                        .text_color(theme.idle)
                        .child(SharedString::from(about)),
                )
        };

        Some(
            div()
                .id("naming")
                .on_key_down(cx.listener(Self::naming_keys))
                .absolute()
                .inset_0()
                .flex()
                .items_center()
                .justify_center()
                .bg(faded(theme.base, 0.72))
                .on_click(cx.listener(|skep, _, _, cx| {
                    skep.naming = None;
                    cx.notify();
                }))
                .child(
                    div()
                        .id("naming-card")
                        .flex()
                        .flex_col()
                        .gap_4()
                        .w(px(460.))
                        .p(px(MARGIN))
                        .rounded(px(PANEL_RADIUS))
                        .bg(theme.raised)
                        .border_1()
                        .border_color(theme.border)
                        .on_click(|_, _, cx| cx.stop_propagation())
                        .child(
                            div()
                                .flex()
                                .flex_col()
                                .gap_1()
                                .child(div().title().child(SharedString::from("Add a project")))
                                .child(
                                    div()
                                        .caption()
                                        .font_family(MONO)
                                        .text_color(theme.muted)
                                        .truncate()
                                        .child(SharedString::from(
                                            naming.directory.display().to_string(),
                                        )),
                                ),
                        )
                        .child(labelled(
                            "Command",
                            "Run from that folder. PORT is set for you, and {port} is filled in \
                             wherever you put it.",
                            &naming.command,
                        ))
                        .child(labelled(
                            "Site",
                            "The name to serve it at. Leave it empty to run it without one.",
                            &naming.site,
                        ))
                        .children(naming.complaint.as_ref().map(|complaint| {
                            div()
                                .caption()
                                .text_color(theme.failed)
                                .child(SharedString::from(complaint.clone()))
                        }))
                        .child(
                            div()
                                .flex()
                                .items_center()
                                .justify_end()
                                .gap_2()
                                .child(self.quiet("naming-cancel", "Cancel").on_click(cx.listener(
                                    |skep, _, _, cx| {
                                        skep.naming = None;
                                        cx.notify();
                                    },
                                )))
                                .child(self.chip("naming-add", "Add project").on_click(
                                    cx.listener(|skep, _, _, cx| {
                                        skep.submit_project(cx);
                                        cx.notify();
                                    }),
                                )),
                        ),
                )
                .into_any_element(),
        )
    }

    /// Only the keys about the form. The fields own everything else.
    pub(super) fn naming_keys(
        &mut self,
        event: &gpui::KeyDownEvent,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let Some(naming) = self.naming.as_ref() else {
            return;
        };
        match event.keystroke.key.as_str() {
            "escape" => {
                self.naming = None;
                cx.notify();
            }
            "tab" => {
                let on_command = naming.command.focus_handle(cx).contains_focused(window, cx);
                let next = if on_command {
                    &naming.site
                } else {
                    &naming.command
                };
                next.focus_handle(cx).focus(window, cx);
                cx.notify();
            }
            "enter" => {
                self.submit_project(cx);
                cx.notify();
            }
            _ => {}
        }
    }

    /// Writes the project down, or says what is wrong with it.
    pub(super) fn submit_project(&mut self, cx: &mut Context<Self>) {
        let Some(naming) = self.naming.as_mut() else {
            return;
        };
        let command = naming.command.read(cx).text().trim().to_string();
        let site = naming.site.read(cx).text().trim().to_string();
        if command.is_empty() {
            naming.complaint = Some("a command is needed: what starts this project".to_string());
            return;
        }
        if !site.is_empty() && comb::valid_hostname(&site).is_err() {
            naming.complaint = Some(format!(
                "{site:?} is not a hostname a certificate can cover"
            ));
            return;
        }
        let directory = naming.directory.display().to_string();
        let _ = self.commands.send(Command::AddProject {
            directory,
            command,
            site: (!site.is_empty()).then_some(site),
        });
        self.naming = None;
    }

    /// The instance a project runs as, if it is registered at all. A project
    /// is an ordinary instance once it starts, so its state comes from the
    /// same replica everything else does rather than from a second account
    /// of what is happening.
    fn project_status(&self, name: &str) -> Option<&ServiceStatus> {
        self.mirror
            .services()
            .find(|status| status.id.service.as_str() == name && status.state.is_running())
    }

    fn project_row(
        &self,
        index: usize,
        project: &crate::bridge::Project,
        cx: &mut Context<Self>,
    ) -> AnyElement {
        let theme = &self.theme;
        let status = self.project_status(&project.name);
        let port = status.and_then(|status| status.ports.values().next().copied());
        let directory = project.directory.clone();
        let colour = match status {
            Some(_) => theme.running,
            None => theme.idle,
        };

        div()
            .id(("project", index))
            .group("project")
            .relative()
            .flex()
            .flex_col()
            .w_full()
            .min_w_0()
            .px(px(MARGIN))
            .py(px(14.))
            .border_b_1()
            .border_color(theme.border)
            .children(status.map(|_| {
                div().absolute().inset_0().bg(gpui::linear_gradient(
                    90.,
                    gpui::linear_color_stop(faded(colour, theme.wash), 0.),
                    gpui::linear_color_stop(faded(colour, 0.), 0.3),
                ))
            }))
            .child(
                div()
                    .flex()
                    .items_center()
                    .gap_3()
                    .w_full()
                    .min_w_0()
                    .child(div().size(px(7.)).rounded_full().flex_shrink_0().bg(colour))
                    .child(
                        div()
                            .flex_shrink_0()
                            .label()
                            .child(SharedString::from(project.name.clone())),
                    )
                    // The name it is served at is the thing you actually type,
                    // so it goes where the eye is already looking.
                    .children(project.site.as_ref().map(|host| {
                        div()
                            .flex_shrink_0()
                            .caption()
                            .text_color(theme.muted)
                            .child(SharedString::from(comb::site_url(host, self.site_port)))
                    }))
                    .children(port.map(|port| self.tag(port.to_string().into())))
                    .child(div().flex_1())
                    .child({
                        let mut actions = div().flex().items_center().gap_2().flex_shrink_0();
                        if let Some(status) = status {
                            let id = status.id.clone();
                            actions = actions.child(self.button("Stop", Command::Stop(id)));
                        } else if project.command.is_some() {
                            actions = actions.child(
                                self.button("Start", Command::StartProject(directory.clone())),
                            );
                        }
                        actions
                    }),
            )
            .child(
                div()
                    .flex()
                    .items_baseline()
                    .gap_3()
                    .w_full()
                    .min_w_0()
                    .pt_1()
                    .pl(px(19.))
                    .child(
                        div()
                            .flex_1()
                            .min_w_0()
                            .truncate()
                            .caption()
                            .text_color(theme.idle)
                            .child(SharedString::from(project.directory.clone())),
                    )
                    .child(
                        div()
                            .flex_shrink_0()
                            .caption()
                            .font_family(MONO)
                            .text_color(theme.idle)
                            .child(SharedString::from(match &project.command {
                                Some(command) => command.clone(),
                                // What a project looks like the moment it is
                                // added, so the row says what to do next
                                // rather than looking broken.
                                None => "add a [run] command to skep.toml".to_string(),
                            })),
                    )
                    .child(
                        // Forgetting is not deleting, so it lives quietly and
                        // only appears when the row is under the pointer.
                        div()
                            .id(("forget", index))
                            .flex_shrink_0()
                            .caption()
                            .text_color(theme.idle)
                            .cursor_pointer()
                            .opacity(0.)
                            .group_hover("project", |style| style.opacity(1.))
                            .hover(|style| style.text_color(theme.text))
                            .on_click(cx.listener(move |skep, _, _, cx| {
                                let _ = skep
                                    .commands
                                    .send(Command::ForgetProject(directory.clone()));
                                cx.notify();
                            }))
                            .child(SharedString::from("forget")),
                    ),
            )
            .into_any_element()
    }
}

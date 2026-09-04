//! The Projects page: what skep runs for you, and where it serves it.

use super::paint::faded;
use super::*;

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
        div()
            .id("add-project")
            .flex_shrink_0()
            .px_3()
            .py_1p5()
            .rounded(px(CHIP))
            .border_1()
            .border_color(self.theme.border)
            .label()
            .text_color(self.theme.accent)
            .cursor_pointer()
            .hover(|style| style.border_color(self.theme.accent))
            .on_click(cx.listener(|skep, _, _, cx| {
                let picked = cx.prompt_for_paths(gpui::PathPromptOptions {
                    files: false,
                    directories: true,
                    multiple: false,
                    prompt: Some(SharedString::from("Add")),
                });
                let commands = skep.commands.clone();
                cx.spawn(async move |skep, cx| {
                    let Ok(Ok(Some(paths))) = picked.await else {
                        return;
                    };
                    let Some(directory) = paths.first() else {
                        return;
                    };
                    let _ = commands.send(Command::AddProject(directory.display().to_string()));
                    let _ = skep.update(cx, |_, cx| cx.notify());
                })
                .detach();
            }))
            .child(SharedString::from("Add a project"))
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
                    gpui::linear_color_stop(faded(colour, 0.06), 0.),
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

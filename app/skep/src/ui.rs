//! The window. Everything rendered here derives from the replica, which
//! derives from the event stream, so the interface cannot show a state the
//! engine never reported.

use std::time::Duration;

use comb::{Applied, InstanceId, Mirror, ServiceState, ServiceStatus};
use gpui::{
    Context, InteractiveElement, IntoElement, ParentElement, Render, SharedString,
    StatefulInteractiveElement, Styled, Window, div, px,
};
use tokio::sync::mpsc::{UnboundedReceiver, UnboundedSender};

use crate::bridge::{Bridge, Command, Update};
use crate::platform::Menubar;
use crate::theme::Theme;

const RAIL: &[(&str, bool)] = &[
    ("Services", true),
    ("Projects", false),
    ("Logs", false),
    ("Mail", false),
    ("Agent", false),
];

enum Connection {
    Waiting,
    Hosting,
    Blocked { pid: Option<u32> },
    Stopping,
}

pub struct Skep {
    menubar: Option<Menubar>,
    theme: Theme,
    mirror: Mirror,
    connection: Connection,
    problem: Option<SharedString>,
    commands: UnboundedSender<Command>,
    updates: UnboundedReceiver<Update>,
}

impl Skep {
    pub fn new(bridge: Bridge, menubar: Option<Menubar>, cx: &mut Context<Self>) -> Self {
        // The engine speaks on a tokio runtime, so its messages are collected
        // here rather than delivered. Draining is cheap and keeps every update
        // arriving on GPUI's own thread.
        cx.spawn(async move |this, cx| {
            loop {
                cx.background_executor()
                    .timer(Duration::from_millis(50))
                    .await;
                if this.update(cx, |skep, cx| skep.drain(cx)).is_err() {
                    return;
                }
            }
        })
        .detach();

        let mut skep = Self {
            menubar,
            theme: Theme::dark(),
            mirror: Mirror::new(),
            connection: Connection::Waiting,
            problem: None,
            commands: bridge.commands,
            updates: bridge.updates,
        };
        skep.reflect();
        skep
    }

    /// The menubar says the same thing the window does, from the same replica.
    fn reflect(&mut self) {
        if let Some(menubar) = &self.menubar {
            let services: Vec<_> = self.mirror.services().cloned().collect();
            menubar.show(self.mirror.summary().glyph(), &services);
        }
    }

    pub fn stopping(&mut self, cx: &mut Context<Self>) {
        self.connection = Connection::Stopping;
        cx.notify();
    }

    fn drain(&mut self, cx: &mut Context<Self>) {
        let mut moved = false;
        while let Ok(update) = self.updates.try_recv() {
            moved = true;
            match update {
                Update::Snapshot(snapshot) => self.mirror.reset(*snapshot),
                Update::Event(event) => {
                    // Falling behind is answered by asking, never by guessing.
                    if self.mirror.apply(&event) == Applied::Resync {
                        let _ = self.commands.send(Command::Resync);
                    }
                }
                Update::Hosting => {
                    self.connection = Connection::Hosting;
                    self.problem = None;
                }
                Update::Blocked { pid } => self.connection = Connection::Blocked { pid },
                Update::Failed(message) => self.problem = Some(message.into()),
            }
        }
        if moved {
            self.reflect();
            cx.notify();
        }
    }
}

/// What the row says it is doing. A failure says whatever the engine said,
/// word for word, because that sentence usually contains the fix.
fn line(status: &ServiceStatus) -> SharedString {
    if let Some(activity) = &status.activity {
        return activity.clone().into();
    }
    match &status.state {
        ServiceState::Ready => "running".into(),
        ServiceState::Failed { reason } => reason.clone().into(),
        ServiceState::Restarting { attempt } => format!("restarting, attempt {attempt}").into(),
        other => other.name().to_string().into(),
    }
}

fn ports(status: &ServiceStatus) -> SharedString {
    status
        .ports
        .values()
        .map(ToString::to_string)
        .collect::<Vec<_>>()
        .join(", ")
        .into()
}

impl Render for Skep {
    fn render(&mut self, _window: &mut Window, _cx: &mut Context<Self>) -> impl IntoElement {
        div()
            .flex()
            .size_full()
            .bg(self.theme.base)
            .text_color(self.theme.text)
            .text_sm()
            .child(self.rail())
            .child(self.content())
    }
}

impl Skep {
    fn rail(&self) -> impl IntoElement {
        let theme = &self.theme;
        div()
            .flex()
            .flex_col()
            .w(px(168.))
            .h_full()
            .border_r_1()
            .border_color(theme.border)
            .py_3()
            .children(RAIL.iter().map(|(name, built)| {
                div()
                    .px_4()
                    .py_1p5()
                    .text_color(if *built { theme.text } else { theme.idle })
                    .child(SharedString::from(*name))
            }))
    }

    fn content(&self) -> impl IntoElement {
        let services: Vec<_> = self.mirror.services().cloned().collect();
        div()
            .flex()
            .flex_col()
            .flex_1()
            .h_full()
            .child(self.header())
            .children(self.banner())
            .child(
                div()
                    .id("services")
                    .flex()
                    .flex_col()
                    .flex_1()
                    .overflow_y_scroll()
                    .children(services.into_iter().map(|status| self.row(status))),
            )
    }

    fn header(&self) -> impl IntoElement {
        let summary = self.mirror.summary();
        div()
            .flex()
            .items_center()
            .justify_between()
            .px_5()
            .py_3()
            .border_b_1()
            .border_color(self.theme.border)
            .child(SharedString::from("Services"))
            .child(
                div()
                    .text_xs()
                    .text_color(self.theme.muted)
                    .child(SharedString::from(format!(
                        "{} of {} running",
                        summary.running, summary.total
                    ))),
            )
    }

    /// The only place anything speaks above a row: who holds the machine, and
    /// what this window is doing about it.
    fn banner(&self) -> Option<impl IntoElement> {
        let message: SharedString = match (&self.connection, &self.problem) {
            (Connection::Blocked { pid }, _) => match pid {
                Some(pid) => format!("another skep is running this machine (pid {pid})").into(),
                None => "another skep is running this machine".into(),
            },
            (Connection::Stopping, _) => "stopping services".into(),
            (Connection::Waiting, _) => "connecting".into(),
            (_, Some(problem)) => problem.clone(),
            _ => return None,
        };

        let blocked = matches!(self.connection, Connection::Blocked { .. });
        Some(
            div()
                .flex()
                .items_center()
                .justify_between()
                .px_5()
                .py_2()
                .bg(self.theme.raised)
                .border_b_1()
                .border_color(self.theme.border)
                .text_xs()
                .text_color(self.theme.muted)
                .child(message)
                .children(blocked.then(|| self.button("Take over", Command::TakeOver))),
        )
    }

    fn row(&self, status: ServiceStatus) -> impl IntoElement {
        let theme = &self.theme;
        let working = status.activity.is_some();
        let failed = matches!(status.state, ServiceState::Failed { .. });
        let id = status.id.clone();

        div()
            .flex()
            .items_center()
            .gap_3()
            .px_5()
            .py_3()
            .border_b_1()
            .border_color(theme.border)
            .child(
                div()
                    .w(px(7.))
                    .h(px(7.))
                    .rounded_full()
                    .bg(theme.dot(&status.state, working)),
            )
            .child(
                div()
                    .w(px(190.))
                    .child(SharedString::from(status.id.to_string())),
            )
            .child(
                div()
                    .w(px(110.))
                    .text_xs()
                    .text_color(theme.muted)
                    .child(ports(&status)),
            )
            .child(
                div()
                    .flex_1()
                    .text_xs()
                    .text_color(if failed { theme.failed } else { theme.muted })
                    .child(line(&status)),
            )
            .child(self.actions(&status, id))
    }

    fn actions(&self, status: &ServiceStatus, id: InstanceId) -> impl IntoElement {
        let live = status.state.is_running() || status.state.is_transitional();
        let mut row = div().flex().items_center().gap_2();
        if live {
            row = row
                .child(self.button("Stop", Command::Stop(id.clone())))
                .child(self.button("Restart", Command::Restart(id)));
        } else {
            row = row.child(self.button("Start", Command::Start(id)));
        }
        row
    }

    /// Orange lives here and on transient dots. Nowhere else.
    fn button(&self, label: &'static str, command: Command) -> impl IntoElement {
        let commands = self.commands.clone();
        div()
            .id(label)
            .px_2p5()
            .py_1()
            .text_xs()
            .text_color(self.theme.accent)
            .border_1()
            .border_color(self.theme.border)
            .rounded_sm()
            .cursor_pointer()
            .hover(|style| style.border_color(self.theme.accent))
            .on_click(move |_, _, _| {
                let _ = commands.send(command.clone());
            })
            .child(SharedString::from(label))
    }
}

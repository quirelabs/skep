//! The window. Everything rendered here derives from the replica, which
//! derives from the event stream, so the interface cannot show a state the
//! engine never reported.

use std::collections::{BTreeMap, VecDeque};
use std::time::{Duration, Instant};

use comb::{Applied, InstanceId, Label, LogLine, Mirror, ServiceState, ServiceStatus, Snapshot};
use gpui::{
    Animation, AnimationExt, AnyElement, ClipboardItem, Context, FontWeight, Hsla,
    InteractiveElement, IntoElement, ParentElement, Render, SharedString,
    StatefulInteractiveElement, Styled, Subscription, Window, div, ease_in_out, pulsating_between,
    px,
};
use tokio::sync::mpsc::{UnboundedReceiver, UnboundedSender};

use crate::bridge::{Bridge, Command, Update};
use crate::platform::Menubar;
use crate::theme::{Scale, Theme};

#[derive(Clone, Copy, PartialEq)]
enum Page {
    Services,
    Sites,
    Settings,
}

/// The rail shows what skep is designed to have, dimmed where it does not have
/// it yet. Settings sits apart at the bottom, where settings go.
const RAIL: &[(&str, Option<Page>)] = &[
    ("Services", Some(Page::Services)),
    ("Sites", Some(Page::Sites)),
    ("Projects", None),
    ("Logs", None),
    ("Mail", None),
    ("Agent", None),
];

/// Every motion in the interface is shorter than this. Anything slower reads
/// as the machine being slow rather than as the interface being alive.
const MOTION: Duration = Duration::from_millis(180);
const BREATH: Duration = Duration::from_millis(1100);
const LOG_HEIGHT: f32 = 208.;
/// Ships with every macOS, unlike the generic family names, which font-kit
/// will not resolve.
const MONO: &str = "Menlo";
const LOG_LIMIT: usize = 400;
/// How long a copied line stays marked.
const ACKNOWLEDGED: Duration = Duration::from_millis(900);
/// Wide enough for four digits, which is more lines than are kept.
const GUTTER: f32 = 34.;

#[derive(Clone, Copy, PartialEq, Eq)]
enum Copied {
    Line(u64),
    Everything,
}

enum Connection {
    Waiting,
    Hosting,
    Blocked { pid: Option<u32> },
    Stopping,
}

pub struct Skep {
    page: Page,
    menubar: Option<Menubar>,
    theme: Theme,
    mirror: Mirror,
    connection: Connection,
    problem: Option<SharedString>,
    expanded: Option<InstanceId>,
    /// Numbered so a line keeps its identity as older ones fall off the end,
    /// which is what stops finished fades from replaying.
    logs: VecDeque<(u64, LogLine)>,
    next_line: u64,
    /// What was just copied, and when, so the acknowledgement clears itself.
    copied: Option<(Copied, Instant)>,
    /// The copies kept for whichever service is open.
    kept: Vec<Snapshot>,
    /// Every hostname this machine serves, and anything in the way of it.
    sites: BTreeMap<String, u16>,
    site_trouble: Vec<String>,
    authority_trusted: bool,
    commands: UnboundedSender<Command>,
    updates: UnboundedReceiver<Update>,
    /// Dropping this stops the appearance following the system.
    _following: Subscription,
}

impl Skep {
    pub fn new(
        bridge: Bridge,
        menubar: Option<Menubar>,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> Self {
        // Light and dark are one system, and the machine already knows which
        // one a person is in. Following it beats asking them twice.
        let following = {
            let this = cx.entity().downgrade();
            window.observe_window_appearance(move |window, cx| {
                let appearance = window.appearance();
                if let Some(this) = this.upgrade() {
                    this.update(cx, |skep, cx| {
                        skep.theme = Theme::for_appearance(appearance);
                        cx.notify();
                    });
                }
            })
        };

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
            page: Page::Services,
            menubar,
            theme: Theme::for_appearance(window.appearance()),
            mirror: Mirror::new(),
            connection: Connection::Waiting,
            problem: None,
            expanded: None,
            logs: VecDeque::new(),
            next_line: 0,
            copied: None,
            kept: Vec::new(),
            sites: BTreeMap::new(),
            site_trouble: Vec::new(),
            authority_trusted: false,
            commands: bridge.commands,
            updates: bridge.updates,
            _following: following,
        };
        skep.reflect();
        skep
    }

    pub fn stopping(&mut self, cx: &mut Context<Self>) {
        self.connection = Connection::Stopping;
        cx.notify();
    }

    /// The menubar says the same thing the window does, from the same replica.
    fn reflect(&mut self) {
        if let Some(menubar) = &self.menubar {
            let services: Vec<_> = self.mirror.services().cloned().collect();
            menubar.show(self.mirror.summary().glyph(), &services);
        }
    }

    fn drain(&mut self, cx: &mut Context<Self>) {
        let mut moved = false;
        while let Ok(update) = self.updates.try_recv() {
            moved = true;
            match update {
                Update::Overview(overview) => self.mirror.reset(*overview),
                Update::Event(event) => {
                    // Falling behind is answered by asking, never by guessing.
                    if self.mirror.apply(&event) == Applied::Resync {
                        let _ = self.commands.send(Command::Resync);
                    }
                }
                Update::Sites {
                    sites,
                    trouble,
                    trusted,
                } => {
                    self.sites = sites;
                    self.site_trouble = trouble;
                    self.authority_trusted = trusted;
                }
                Update::Hosting => {
                    self.connection = Connection::Hosting;
                    self.problem = None;
                }
                Update::Blocked { pid } => self.connection = Connection::Blocked { pid },
                Update::Failed(message) => self.problem = Some(message.into()),
                Update::Logs(tail) => {
                    self.logs.clear();
                    for line in tail {
                        self.remember(line);
                    }
                }
                Update::Log(line) => self.remember(*line),
                Update::Kept(kept) => self.kept = kept,
            }
        }
        // The acknowledgement clears itself on the same beat as everything else.
        if self
            .copied
            .is_some_and(|(_, at)| at.elapsed() > ACKNOWLEDGED)
        {
            self.copied = None;
            moved = true;
        }
        if moved {
            self.reflect();
            cx.notify();
        }
    }

    fn remember(&mut self, line: LogLine) {
        // Subscribing before reading the tail can repeat one line. Dropping a
        // repeat is cheaper than dropping a line.
        if self
            .logs
            .back()
            .is_some_and(|(_, last)| last.at == line.at && last.text == line.text)
        {
            return;
        }
        self.logs.push_back((self.next_line, line));
        self.next_line += 1;
        while self.logs.len() > LOG_LIMIT {
            self.logs.pop_front();
        }
    }

    fn toggle(&mut self, id: InstanceId, cx: &mut Context<Self>) {
        self.logs.clear();
        // Numbering restarts with each watch, so a line's number means its
        // place in what is on screen rather than a running total nobody asked
        // for.
        self.next_line = 0;
        self.copied = None;
        self.kept.clear();
        self.expanded = if self.expanded.as_ref() == Some(&id) {
            let _ = self.commands.send(Command::Watch(None));
            None
        } else {
            let _ = self.commands.send(Command::Watch(Some(id.clone())));
            let _ = self.commands.send(Command::Snapshots(id.clone()));
            Some(id)
        };
        cx.notify();
    }
}

fn faded(color: Hsla, alpha: f32) -> Hsla {
    Hsla { a: alpha, ..color }
}

/// What the row says it is doing. The row gets one line of it; the whole
/// sentence, which usually contains the fix, is in the expansion.
fn line(status: &ServiceStatus) -> SharedString {
    if let Some(activity) = &status.activity {
        return activity.clone().into();
    }
    match &status.state {
        ServiceState::Ready => "running".into(),
        ServiceState::Failed { reason } => reason.clone().into(),
        ServiceState::Restarting { attempt } => format!("restarting, attempt {attempt}").into(),
        // Worth saying before anyone clicks: this one cannot start.
        _ if status.blocked.is_some() => "stopped, port in use".into(),
        other => other.name().to_string().into(),
    }
}

/// The whole of whatever is wrong, if anything is.
fn note(status: &ServiceStatus) -> Option<SharedString> {
    match &status.state {
        ServiceState::Failed { reason } => Some(reason.clone().into()),
        _ => status.blocked.clone().map(Into::into),
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
    fn render(&mut self, _window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        div()
            .flex()
            .size_full()
            .bg(self.theme.base)
            .text_color(self.theme.text)
            .body()
            .child(self.rail(cx))
            .child(match self.page {
                Page::Services => self.content(cx),
                Page::Sites => self.sites_page(cx),
                Page::Settings => self.settings(cx),
            })
    }
}

impl Skep {
    fn rail(&self, cx: &mut Context<Self>) -> AnyElement {
        let mut items = Vec::with_capacity(RAIL.len());
        for (index, (name, page)) in RAIL.iter().enumerate() {
            items.push(self.rail_item(index, name, *page, cx));
        }

        div()
            .flex()
            .flex_col()
            .w(px(164.))
            .h_full()
            .flex_shrink_0()
            .border_r_1()
            .border_color(self.theme.border)
            .pt_5()
            .pb_4()
            .gap_0p5()
            .children(items)
            .child(div().flex_1())
            .child(self.rail_item(RAIL.len(), "Settings", Some(Page::Settings), cx))
            .into_any_element()
    }

    fn rail_item(
        &self,
        index: usize,
        name: &'static str,
        page: Option<Page>,
        cx: &mut Context<Self>,
    ) -> AnyElement {
        let built = page.is_some();
        let here = page == Some(self.page);
        let item = div()
            .id(("rail", index))
            .px_6()
            .py_1()
            .text_color(if here {
                self.theme.text
            } else if built {
                self.theme.muted
            } else {
                self.theme.idle
            })
            .font_weight(if here {
                FontWeight::MEDIUM
            } else {
                FontWeight::NORMAL
            })
            .child(SharedString::from(name));

        match page {
            Some(page) => item
                .cursor_pointer()
                .on_click(cx.listener(move |skep, _, _, cx| {
                    skep.page = page;
                    cx.notify();
                }))
                .into_any_element(),
            None => item.into_any_element(),
        }
    }

    /// Read only on purpose. The file is the interface; this explains what it
    /// currently means and opens it.
    /// Sites are what skep puts in front of an app you already run, so the
    /// page says the url and the port and nothing else. Anything in the way
    /// is said plainly rather than left for a person to work out from a
    /// browser warning.
    fn sites_page(&self, _cx: &mut Context<Self>) -> AnyElement {
        let theme = &self.theme;

        let mut rows = div().flex().flex_col().w_full();
        if self.sites.is_empty() {
            rows = rows.child(div().px_6().py_4().label().text_color(theme.muted).child(
                SharedString::from(
                    "No sites yet. Put one in skep.toml or config.toml, then run skep up:\n\n\
                         [sites]\n\"myapp.test\" = 3000",
                ),
            ));
        }
        for (host, port) in &self.sites {
            rows = rows.child(
                div()
                    .flex()
                    .items_center()
                    .justify_between()
                    .w_full()
                    .min_w_0()
                    .px_6()
                    .py_3()
                    .border_b_1()
                    .border_color(theme.border)
                    .child(div().min_w_0().child(SharedString::from(format!(
                        "https://{host}:{}",
                        comb::HTTPS_PORT
                    ))))
                    .child(
                        div()
                            .caption()
                            .flex_shrink_0()
                            .text_color(theme.muted)
                            .child(SharedString::from(format!("port {port}"))),
                    ),
            );
        }

        let mut notes = div().flex().flex_col().w_full();
        if !self.authority_trusted && !self.sites.is_empty() {
            notes = notes.child(
                self.note("Certificates are not trusted on this machine yet. Run skep trust."),
            );
        }
        for trouble in &self.site_trouble {
            notes = notes.child(self.note(trouble));
        }

        div()
            .flex()
            .flex_col()
            .flex_1()
            .h_full()
            .min_w_0()
            .overflow_hidden()
            .child(
                div()
                    .flex()
                    .items_center()
                    .w_full()
                    .px_6()
                    .py_4()
                    .border_b_1()
                    .border_color(theme.border)
                    .child(div().title().child(SharedString::from("Sites"))),
            )
            .child(notes)
            .child(rows)
            .into_any_element()
    }

    fn note(&self, text: &str) -> impl IntoElement {
        div()
            .w_full()
            .min_w_0()
            .px_6()
            .py_3()
            .body()
            .text_color(self.theme.muted)
            .child(SharedString::from(text.to_string()))
    }

    fn settings(&self, cx: &mut Context<Self>) -> AnyElement {
        let theme = &self.theme;
        let services: Vec<_> = self.mirror.services().cloned().collect();

        div()
            .flex()
            .flex_col()
            .flex_1()
            .h_full()
            .overflow_hidden()
            .child(
                div()
                    .flex()
                    .items_center()
                    .justify_between()
                    .w_full()
                    .px_6()
                    .py_4()
                    .border_b_1()
                    .border_color(theme.border)
                    .child(div().title().child(SharedString::from("Settings")))
                    .child(self.open_settings(cx)),
            )
            .child(
                div()
                    .w_full()
                    .px_6()
                    .py_3()
                    .body()
                    .text_color(theme.muted)
                    .child(SharedString::from(
                        "Ports and versions live in config.toml. A project's skep.toml wins \
                         wherever both speak, so a repository always gets what it asks for.",
                    )),
            )
            .child(
                div()
                    .id("settings-list")
                    .flex()
                    .flex_col()
                    .flex_1()
                    .w_full()
                    .min_w_0()
                    .overflow_y_scroll()
                    .children(services.into_iter().map(|status| {
                        let chosen = !status.ports_from.is_empty();
                        let ports: Vec<String> = status
                            .ports
                            .iter()
                            .map(|(name, number)| match status.ports_from.get(name) {
                                Some(source) => format!("{name} {number} ({source})"),
                                None => format!("{name} {number}"),
                            })
                            .collect();

                        div()
                            .flex()
                            .w_full()
                            .items_center()
                            .gap_3()
                            .px_6()
                            .py_3()
                            .border_b_1()
                            .border_color(theme.border)
                            .child(
                                div()
                                    .w(px(186.))
                                    .truncate()
                                    .child(SharedString::from(status.id.to_string())),
                            )
                            .child(
                                div()
                                    .flex_1()
                                    .truncate()
                                    .label()
                                    .text_color(if chosen { theme.text } else { theme.muted })
                                    .child(SharedString::from(ports.join(",  "))),
                            )
                    })),
            )
            .into_any_element()
    }

    fn open_settings(&self, cx: &mut Context<Self>) -> AnyElement {
        div()
            .id("open-settings")
            .px_2p5()
            .py_1()
            .label()
            .text_color(self.theme.accent)
            .border_1()
            .border_color(self.theme.border)
            .rounded_sm()
            .cursor_pointer()
            .hover(|style| style.border_color(self.theme.accent))
            .on_click(cx.listener(|skep, _, _, cx| {
                skep.reveal_settings();
                cx.notify();
            }))
            .child(SharedString::from("Open config.toml"))
            .into_any_element()
    }

    /// Writes a commented starting point if there is nothing there, then hands
    /// the file to whatever the machine opens .toml with.
    fn reveal_settings(&mut self) {
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

    fn content(&self, cx: &mut Context<Self>) -> AnyElement {
        let services: Vec<_> = self.mirror.services().cloned().collect();
        let empty = services.is_empty();
        let mut rows = Vec::with_capacity(services.len());
        for (index, status) in services.into_iter().enumerate() {
            rows.push(self.row(index, status, cx));
        }

        div()
            .flex()
            .flex_col()
            .flex_1()
            .min_w_0()
            .h_full()
            .child(self.header())
            .children(self.banner())
            .child(
                div()
                    .id("services")
                    .flex()
                    .flex_col()
                    .flex_1()
                    .min_w_0()
                    .overflow_y_scroll()
                    .children(rows)
                    .children(empty.then(|| self.nothing("no services yet"))),
            )
            .into_any_element()
    }

    /// Empty states say what would be here, quietly, and never fill the space
    /// with something to look at.
    fn nothing(&self, what: &'static str) -> impl IntoElement {
        div()
            .flex()
            .flex_1()
            .items_center()
            .justify_center()
            .py_10()
            .label()
            .text_color(self.theme.idle)
            .child(SharedString::from(what))
    }

    fn header(&self) -> impl IntoElement {
        let summary = self.mirror.summary();
        div()
            .flex()
            .items_center()
            .justify_between()
            .px_6()
            .py_4()
            .border_b_1()
            .border_color(self.theme.border)
            .child(div().title().child(SharedString::from("Services")))
            .child(
                div()
                    .label()
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
                .px_6()
                .py_2p5()
                .bg(self.theme.raised)
                .border_b_1()
                .border_color(self.theme.border)
                .label()
                .text_color(self.theme.muted)
                .child(message)
                .children(blocked.then(|| self.button("Take over", Command::TakeOver))),
        )
    }

    fn row(&self, index: usize, status: ServiceStatus, cx: &mut Context<Self>) -> AnyElement {
        let theme = &self.theme;
        let working = status.activity.is_some() || status.state.is_transitional();
        let failed = matches!(status.state, ServiceState::Failed { .. });
        let id = status.id.clone();
        let open = self.expanded.as_ref() == Some(&id);
        let key = SharedString::from(id.to_string());

        let head = div()
            .id(("row", index))
            .flex()
            .w_full()
            .min_w_0()
            .items_center()
            .gap_3()
            .pl(px(if status.id.is_branch() { 44. } else { 24. }))
            .pr_6()
            .py_3()
            .cursor_pointer()
            .hover(|style| style.bg(theme.raised))
            .on_click(cx.listener({
                let id = id.clone();
                move |skep, _, _, cx| skep.toggle(id.clone(), cx)
            }))
            .child(self.dot(&status, working, index))
            .child(div().w(px(186.)).truncate().child(key.clone()))
            .child(
                div()
                    .w(px(104.))
                    .label()
                    .text_color(theme.muted)
                    .child(ports(&status)),
            )
            .child(
                // Truncated on purpose: a port conflict's remedy is a whole
                // sentence, and it belongs in the expansion rather than
                // pushing the buttons off the window.
                div()
                    .flex_1()
                    .truncate()
                    .label()
                    .text_color(if failed { theme.failed } else { theme.muted })
                    .child(line(&status)),
            )
            .child(self.actions(&status, id));

        div()
            .flex()
            .flex_col()
            .w_full()
            .overflow_hidden()
            .border_b_1()
            .border_color(theme.border)
            .child(head)
            .children(open.then(|| self.output(index, note(&status), cx)))
            .into_any_element()
    }

    /// Status colour lives here and nowhere else. Orange means motion: while a
    /// service is working the dot breathes, which is the only thing in the
    /// interface that repeats.
    fn dot(&self, status: &ServiceStatus, working: bool, index: usize) -> AnyElement {
        let colour = self.theme.dot(&status.state, working);
        let dot = div()
            .w(px(7.))
            .h(px(7.))
            .flex_shrink_0()
            .rounded_full()
            .bg(colour);

        if !working {
            return dot.into_any_element();
        }
        dot.with_animation(
            ("breath", index),
            Animation::new(BREATH)
                .repeat()
                .with_easing(pulsating_between(0.35, 1.0)),
            move |dot, delta| dot.bg(faded(colour, delta)),
        )
        .into_any_element()
    }

    /// The row grows in place. Nothing that needs attention appears anywhere
    /// the eye was not already looking.
    fn output(
        &self,
        index: usize,
        note: Option<SharedString>,
        cx: &mut Context<Self>,
    ) -> AnyElement {
        let theme = &self.theme;
        let muted = theme.muted;
        let text = theme.text;
        let accent = theme.accent;
        let idle = theme.idle;
        let lines: Vec<_> = self.logs.iter().cloned().collect();
        let empty = lines.is_empty();

        let mut rendered = Vec::with_capacity(lines.len());
        for (seq, line) in lines {
            let marked = self
                .copied
                .is_some_and(|(what, _)| what == Copied::Line(seq));
            rendered.push(
                div()
                    .id(("line", seq as usize))
                    .flex()
                    .w_full()
                    .min_w_0()
                    .flex_shrink_0()
                    .gap_3()
                    .px_6()
                    .py_0p5()
                    .cursor_pointer()
                    .hover(|style| style.bg(theme.base))
                    .on_click(cx.listener(move |skep, _, _, cx| skep.copy_line(seq, cx)))
                    // The number is the quiet part: it exists to be referred
                    // to, not to be read.
                    .child(
                        div()
                            .w(px(GUTTER))
                            .flex_shrink_0()
                            .text_right()
                            .body()
                            .font_family(MONO)
                            .text_color(if marked { accent } else { idle })
                            .child(SharedString::from((seq + 1).to_string())),
                    )
                    .child(
                        div()
                            .flex_1()
                            .min_w_0()
                            .body()
                            .font_family(MONO)
                            .text_color(if marked { text } else { muted })
                            .child(SharedString::from(line.text))
                            .with_animation(
                                ("fade", seq as usize),
                                Animation::new(MOTION).with_easing(ease_in_out),
                                move |line, delta| line.text_color(faded(muted, delta)),
                            ),
                    )
                    .into_any_element(),
            );
        }

        div()
            .id(("output", index))
            .flex()
            .flex_col()
            .w_full()
            .min_w_0()
            .overflow_x_hidden()
            .bg(self.theme.raised)
            .border_t_1()
            .border_color(theme.border)
            .overflow_y_scroll()
            .child(self.keeping(cx))
            .children(note.map(|note| {
                div()
                    .w_full()
                    .flex_shrink_0()
                    .px_6()
                    .py_3()
                    .body()
                    .text_color(theme.failed)
                    .child(note)
            }))
            .children((!empty).then(|| {
                div()
                    .flex()
                    .w_full()
                    .flex_shrink_0()
                    .justify_end()
                    .px_6()
                    .pt_2()
                    .child(self.copy_all(cx))
            }))
            .children(rendered)
            .children(empty.then(|| self.nothing("no output yet")))
            .with_animation(
                ("open", index),
                Animation::new(MOTION).with_easing(ease_in_out),
                |body, delta| body.h(px(LOG_HEIGHT * delta)),
            )
            .into_any_element()
    }

    /// Snapshots and branches, where stopping and restarting already are:
    /// more of the same list rather than a mode to enter.
    fn keeping(&self, cx: &mut Context<Self>) -> AnyElement {
        let Some(id) = self.expanded.clone() else {
            return div().into_any_element();
        };
        let theme = &self.theme;
        let taking = id.clone();
        let sprouting = id.clone();

        let mut strip = div()
            .flex()
            .w_full()
            .flex_shrink_0()
            .items_center()
            .gap_2()
            .px_6()
            .py_3()
            .border_b_1()
            .border_color(theme.border)
            .child(
                div()
                    .flex_1()
                    .min_w_0()
                    .label()
                    .text_color(theme.muted)
                    .child(SharedString::from(if self.kept.is_empty() {
                        "no copies kept".to_string()
                    } else {
                        format!("{} kept", self.kept.len())
                    })),
            )
            .child(self.act(
                "Snapshot",
                move |skep, cx| {
                    let name = skep.next_name("snapshot", |kept| kept.name.clone());
                    let _ = skep.commands.send(Command::Snapshot(taking.clone(), name));
                    cx.notify();
                },
                cx,
            ))
            .child(self.act(
                "Branch",
                move |skep, cx| {
                    let label = skep.next_name("branch", |kept| kept.name.clone());
                    if let Ok(label) = Label::new(label) {
                        let _ = skep
                            .commands
                            .send(Command::Branch(sprouting.clone(), label, None));
                    }
                    cx.notify();
                },
                cx,
            ));

        if id.is_branch() {
            let doomed = id.clone();
            strip = strip.child(self.act(
                "Delete",
                move |skep, cx| {
                    let _ = skep.commands.send(Command::RemoveBranch(doomed.clone()));
                    cx.notify();
                },
                cx,
            ));
        }

        let kept: Vec<AnyElement> = self
            .kept
            .iter()
            .enumerate()
            .map(|(index, snapshot)| {
                let (from, name) = (id.clone(), snapshot.name.clone());
                let (dropping, dropped) = (id.clone(), snapshot.name.clone());
                div()
                    .flex()
                    .w_full()
                    .flex_shrink_0()
                    .items_center()
                    .gap_2()
                    .px_6()
                    .py_1p5()
                    .child(
                        div()
                            .flex_1()
                            .min_w_0()
                            .label()
                            .text_color(theme.muted)
                            .child(SharedString::from(snapshot.name.clone())),
                    )
                    .child(self.act_at(
                        ("branch-from", index),
                        "Branch from",
                        move |skep, cx| {
                            let label = skep.next_name("branch", |kept| kept.name.clone());
                            if let Ok(label) = Label::new(label) {
                                let _ = skep.commands.send(Command::Branch(
                                    from.clone(),
                                    label,
                                    Some(name.clone()),
                                ));
                            }
                            cx.notify();
                        },
                        cx,
                    ))
                    .child(self.act_at(
                        ("drop-snapshot", index),
                        "Delete",
                        move |skep, cx| {
                            let _ = skep
                                .commands
                                .send(Command::RemoveSnapshot(dropping.clone(), dropped.clone()));
                            cx.notify();
                        },
                        cx,
                    ))
                    .into_any_element()
            })
            .collect();

        div()
            .flex()
            .flex_col()
            .w_full()
            .flex_shrink_0()
            .child(strip)
            .children(kept)
            .into_any_element()
    }

    /// A name nobody is using yet, since there is nowhere to type one.
    fn next_name(&self, stem: &str, of: impl Fn(&Snapshot) -> String) -> String {
        let taken: Vec<String> = self
            .kept
            .iter()
            .map(&of)
            .chain(
                self.mirror
                    .services()
                    .filter_map(|service| service.id.label.as_ref().map(ToString::to_string)),
            )
            .collect();
        (1..)
            .map(|n| format!("{stem}-{n}"))
            .find(|name| !taken.contains(name))
            .unwrap_or_else(|| format!("{stem}-1"))
    }

    fn act(
        &self,
        label: &'static str,
        run: impl Fn(&mut Self, &mut Context<Self>) + 'static,
        cx: &mut Context<Self>,
    ) -> AnyElement {
        self.act_at((label, 0), label, run, cx)
    }

    fn act_at(
        &self,
        id: (&'static str, usize),
        label: &'static str,
        run: impl Fn(&mut Self, &mut Context<Self>) + 'static,
        cx: &mut Context<Self>,
    ) -> AnyElement {
        div()
            .id(id)
            .flex_shrink_0()
            .px_2p5()
            .py_1()
            .label()
            .text_color(self.theme.accent)
            .border_1()
            .border_color(self.theme.border)
            .rounded_sm()
            .cursor_pointer()
            .hover(|style| style.border_color(self.theme.accent))
            .on_click(cx.listener(move |skep, _, _, cx| run(skep, cx)))
            .child(SharedString::from(label))
            .into_any_element()
    }

    fn copy_all(&self, cx: &mut Context<Self>) -> AnyElement {
        let done = self
            .copied
            .is_some_and(|(what, _)| what == Copied::Everything);
        div()
            .id("copy-all")
            .label()
            .text_color(if done {
                self.theme.text
            } else {
                self.theme.accent
            })
            .cursor_pointer()
            .on_click(cx.listener(|skep, _, _, cx| {
                let text: Vec<&str> = skep
                    .logs
                    .iter()
                    .map(|(_, line)| line.text.as_str())
                    .collect();
                cx.write_to_clipboard(ClipboardItem::new_string(text.join("\n")));
                skep.copied = Some((Copied::Everything, Instant::now()));
                cx.notify();
            }))
            .child(SharedString::from(if done { "copied" } else { "copy all" }))
            .into_any_element()
    }

    /// One line, because that is usually the one being pasted into a search.
    fn copy_line(&mut self, seq: u64, cx: &mut Context<Self>) {
        let Some((_, line)) = self.logs.iter().find(|(number, _)| *number == seq) else {
            return;
        };
        cx.write_to_clipboard(ClipboardItem::new_string(line.text.clone()));
        self.copied = Some((Copied::Line(seq), Instant::now()));
        cx.notify();
    }

    fn actions(&self, status: &ServiceStatus, id: InstanceId) -> impl IntoElement {
        let live = status.state.is_running() || status.state.is_transitional();
        // Everything else in the row compresses before the buttons do: a
        // control you cannot reach is worse than a name you cannot finish.
        let mut row = div().flex().flex_shrink_0().items_center().gap_2();
        if live {
            row = row
                .child(self.button("Stop", Command::Stop(id.clone())))
                .child(self.button("Restart", Command::Restart(id)));
        } else {
            row = row.child(self.button("Start", Command::Start(id)));
        }
        row
    }

    /// Orange lives here and on a breathing dot. Nowhere else.
    fn button(&self, label: &'static str, command: Command) -> impl IntoElement {
        let commands = self.commands.clone();
        div()
            .id(label)
            .px_2p5()
            .py_1()
            .label()
            .text_color(self.theme.accent)
            .border_1()
            .border_color(self.theme.border)
            .rounded_sm()
            .cursor_pointer()
            .hover(|style| style.border_color(self.theme.accent))
            .on_click(move |_, _, cx| {
                cx.stop_propagation();
                let _ = commands.send(command.clone());
            })
            .child(SharedString::from(label))
    }
}

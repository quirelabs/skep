//! The Services page: the list, the expanded row with its log, and the
//! snapshots kept for it.

use comb::EventKind;

use super::paint::{dither, faded};
use super::*;

pub(super) const LOG_HEIGHT: f32 = 248.;
/// What the same panel is worth when there is nothing to show in it.
const QUIET_HEIGHT: f32 = 108.;
pub(super) const LOG_LIMIT: usize = 400;
/// Wide enough for four digits, which is more lines than are kept.
pub(super) const GUTTER: f32 = 34.;

/// What the row says it is doing. The row gets one line of it; the whole
/// sentence, which usually contains the fix, is in the expansion.
/// What is worth saying beside a row, if anything is.
///
/// The dot already carries the state, so repeating it in words next to it
/// says one thing twice and leaves five rows all reading "stopped". Only what
/// the dot cannot say gets words: a phase, an announcement, a reason, or the
/// fact that starting would fail.
pub(super) fn line(status: &ServiceStatus) -> Option<SharedString> {
    if let Some(activity) = &status.activity {
        return Some(activity.clone().into());
    }
    // What the service announced is the one thing to know about it while
    // it runs: for a tunnel, the public url.
    if let Some(notice) = &status.notice {
        return Some(notice.clone().into());
    }
    match &status.state {
        ServiceState::Failed { reason } => Some(reason.clone().into()),
        ServiceState::Restarting { attempt } => {
            Some(format!("restarting, attempt {attempt}").into())
        }
        _ if status.blocked.is_some() => Some("port in use".into()),
        _ => None,
    }
}

/// The whole of whatever is wrong, if anything is.
pub(super) fn note(status: &ServiceStatus) -> Option<SharedString> {
    match &status.state {
        ServiceState::Failed { reason } => Some(reason.clone().into()),
        _ => status.blocked.clone().map(Into::into),
    }
}

/// Every port a service listens on, each its own tag. A comma separated run
/// of numbers reads as one long number; a tag apiece reads as a count you can
/// take in without counting.
pub(super) fn ports(status: &ServiceStatus) -> Vec<SharedString> {
    status
        .ports
        .values()
        .map(|port| port.to_string().into())
        .collect()
}

/// How many of the most recent happenings are kept. Enough to answer "what
/// just went wrong" without becoming a log nobody reads.
pub(super) const HAPPENINGS: usize = 40;

/// The list gives way to the feed rather than the other way round: past this
/// the services scroll among themselves, which keeps what just happened on
/// screen however many services are registered.
const LIST_CEILING: f32 = 320.;

/// Something worth saying happened to a service. Built from the same event
/// stream the replica is built from, so the feed cannot disagree with the
/// list above it.
pub(super) struct Happening {
    pub(super) at: comb::Timestamp,
    pub(super) who: SharedString,
    pub(super) said: SharedString,
}

impl Happening {
    /// Most events are machinery. Only the ones a person would want to know
    /// about become a line, which is what keeps a download's progress from
    /// burying a crash.
    pub(super) fn of(event: &comb::Event) -> Option<Self> {
        let who = event.instance.as_ref()?;
        let said: SharedString = match &event.kind {
            EventKind::StateChanged { to, .. } => match to {
                ServiceState::Ready => "ready".into(),
                ServiceState::Stopped => "stopped".into(),
                ServiceState::Starting => "starting".into(),
                ServiceState::Failed { reason } => format!("failed: {reason}").into(),
                _ => return None,
            },
            EventKind::PortConflict { port, process, .. } => match process {
                Some(process) => format!("port {port} held by {process}").into(),
                None => format!("port {port} is in use").into(),
            },
            EventKind::RestartScheduled { attempt, .. } => {
                format!("restarting, attempt {attempt}").into()
            }
            EventKind::Notice { text: Some(text) } => format!("on {text}").into(),
            _ => return None,
        };
        Some(Self {
            at: event.at,
            who: who.service.as_str().to_string().into(),
            said,
        })
    }
}

/// How long ago, in the shortest true form. Relative rather than a clock
/// time, because a feed of things that just happened is read as distances
/// from now, and because the machine's offset from utc is not something this
/// app has any business working out.
pub(super) fn ago(at: comb::Timestamp) -> SharedString {
    let now = comb::Timestamp::now().as_millis();
    let seconds = now.saturating_sub(at.as_millis()) / 1_000;
    match seconds {
        0..=4 => "just now".into(),
        5..=59 => format!("{seconds}s ago").into(),
        60..=3599 => format!("{}m ago", seconds / 60).into(),
        3600..=86_399 => format!("{}h ago", seconds / 3600).into(),
        _ => format!("{}d ago", seconds / 86_400).into(),
    }
}

impl Skep {
    /// A service, named. The service and the version it is are one string in
    /// the protocol and two different things to read: the name is what you are
    /// looking for, the version is what you check afterwards. A branch says so
    /// in words, because indentation alone does not survive a glance.
    pub(super) fn instance(&self, status: &ServiceStatus) -> AnyElement {
        let theme = &self.theme;
        let id = &status.id;

        let mut named = div()
            .flex()
            .items_baseline()
            .gap_1p5()
            .w(px(200.))
            .min_w_0()
            .flex_shrink_0()
            .child(
                div()
                    .flex_shrink_0()
                    .child(SharedString::from(id.service.as_str().to_string())),
            )
            .child(
                div()
                    .caption()
                    .min_w_0()
                    .truncate()
                    .text_color(theme.muted)
                    .child(SharedString::from(id.version.as_str().to_string())),
            );

        if let Some(label) = id.tag.as_ref().map(comb::Tag::name) {
            named = named.child(
                div()
                    .flex_shrink_0()
                    .caption()
                    .px_1()
                    .rounded(px(CHIP))
                    .bg(theme.base)
                    .text_color(theme.muted)
                    .child(SharedString::from(label.as_str().to_string())),
            );
        }
        named.into_any_element()
    }

    pub(super) fn note(&self, text: &str) -> impl IntoElement {
        div()
            .w_full()
            .min_w_0()
            .px_6()
            .py_3()
            .body()
            .text_color(self.theme.muted)
            .child(SharedString::from(text.to_string()))
    }

    pub(super) fn content(&self, cx: &mut Context<Self>) -> AnyElement {
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
            .child(self.header(cx))
            .children(self.banner())
            .child(self.hero())
            .child(
                div()
                    .id("services")
                    .flex()
                    .flex_col()
                    .flex_shrink_0()
                    .min_w_0()
                    .max_h(px(LIST_CEILING))
                    .overflow_y_scroll()
                    .children(rows)
                    .children(empty.then(|| self.nothing("no services yet"))),
            )
            .child(self.stream())
            .into_any_element()
    }

    /// Empty states say what would be here, quietly, and never fill the space
    /// with something to look at.
    /// Where there is nothing, with the texture that only ever appears here.
    ///
    /// An ordered dither rather than a random one: the pattern comes from the
    /// cell's own coordinates, so it does not crawl when the window resizes
    /// and it draws the same way twice. It fades out towards the words so it
    /// never competes with them.
    pub(super) fn nothing(&self, what: &'static str) -> impl IntoElement {
        let ink = self.theme.idle;
        div()
            .relative()
            .flex()
            .flex_1()
            .items_center()
            .justify_center()
            .py_10()
            .child(
                gpui::canvas(
                    |_, _, _| (),
                    move |bounds, _, window, _| dither(bounds, ink, window),
                )
                .absolute()
                .size_full(),
            )
            .child(
                div()
                    .label()
                    .text_color(self.theme.idle)
                    .child(SharedString::from(what)),
            )
    }

    pub(super) fn header(&self, cx: &mut Context<Self>) -> impl IntoElement {
        let summary = self.mirror.summary();
        self.page_header(
            "Services",
            Some(
                div()
                    .label()
                    .text_color(self.theme.muted)
                    .child(SharedString::from(format!(
                        "{} of {} running",
                        summary.running, summary.total
                    )))
                    .into_any_element(),
            ),
            cx,
        )
    }

    /// The only place anything speaks above a row: who holds the machine, and
    /// what this window is doing about it.
    pub(super) fn banner(&self) -> Option<impl IntoElement> {
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

    pub(super) fn row(
        &self,
        index: usize,
        status: ServiceStatus,
        cx: &mut Context<Self>,
    ) -> AnyElement {
        let theme = &self.theme;
        let working = status.activity.is_some() || status.state.is_transitional();
        let failed = matches!(status.state, ServiceState::Failed { .. });
        let id = status.id.clone();
        let open = self.expanded.as_ref() == Some(&id);

        // A row that has something to say says it across itself: the state's
        // own colour, strongest at the edge it starts from and gone by the
        // time it reaches the words. Only for the states worth noticing, so
        // a list of stopped services stays quiet.
        let wash = match &status.state {
            ServiceState::Ready => Some(theme.running),
            ServiceState::Failed { .. } => Some(theme.failed),
            _ if status.blocked.is_some() => Some(theme.failed),
            _ => None,
        };

        let head = div()
            .id(("row", index))
            .group("row")
            .relative()
            // The row you are pointing at is marked at its edge rather than
            // lit across its middle: an edge is a thing the eye finds without
            // being asked to look at it.
            .child(
                div()
                    .absolute()
                    .left_0()
                    .top_0()
                    .bottom_0()
                    .w(px(2.))
                    .bg(theme.accent)
                    .opacity(0.)
                    .group_hover("row", |style| style.opacity(1.)),
            )
            .children(wash.map(|colour| {
                div().absolute().inset_0().bg(gpui::linear_gradient(
                    90.,
                    gpui::linear_color_stop(faded(colour, 0.06), 0.),
                    gpui::linear_color_stop(faded(colour, 0.), 0.3),
                ))
            }))
            .flex()
            .w_full()
            .min_w_0()
            .items_center()
            .gap_3()
            .pl(px(if status.id.is_branch() {
                MARGIN + INDENT
            } else {
                MARGIN
            }))
            .pr(px(MARGIN))
            .py(px(14.))
            .cursor_pointer()
            .hover(|style| style.bg(theme.raised))
            .on_click(cx.listener({
                let id = id.clone();
                move |skep, _, _, cx| skep.toggle(id.clone(), cx)
            }))
            .child(self.dot(&status, working, index))
            .child(self.instance(&status))
            .child(
                div()
                    .flex()
                    .items_center()
                    .gap_1p5()
                    .w(px(132.))
                    .flex_shrink_0()
                    .children(ports(&status).into_iter().map(|port| self.tag(port))),
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
                    .children(line(&status)),
            )
            .child(
                // Five rows of lit buttons is five things shouting. The
                // action belongs to the row you are pointing at, and to the
                // row you have opened, which is the one you are working on.
                {
                    let mut holder = div().flex_shrink_0();
                    if !open {
                        holder = holder
                            .opacity(0.)
                            .group_hover("row", |style| style.opacity(1.));
                    }
                    holder.child(self.actions(&status, id))
                },
            );

        div()
            .flex()
            .flex_col()
            .w_full()
            .overflow_hidden()
            .border_b_1()
            .border_color(theme.border)
            .child(head)
            .children(open.then(|| self.output(index, &status, note(&status), cx)))
            .into_any_element()
    }

    /// A number that is a thing rather than a word: mono, boxed, quiet.
    pub(super) fn tag(&self, text: SharedString) -> impl IntoElement {
        div()
            .flex_shrink_0()
            .px_1p5()
            .rounded(px(CHIP))
            .bg(self.theme.raised)
            .border_1()
            .border_color(self.theme.border)
            .caption()
            .font_family(MONO)
            .text_color(self.theme.muted)
            .child(text)
    }

    /// The page's face. One thing larger than everything else, so a screen
    /// reads as a place rather than as a table with a title over it.
    pub(super) fn hero(&self) -> impl IntoElement {
        let summary = self.mirror.summary();
        let blocked = self
            .mirror
            .services()
            .filter(|status| status.blocked.is_some())
            .count();
        let said = match (summary.total, summary.failed, blocked) {
            (0, _, _) => "nothing registered yet".to_string(),
            (_, 0, 0) => "everything is where you left it".to_string(),
            (_, failed, 0) => format!("{failed} failed"),
            (_, 0, 1) => "one port is held by something else".to_string(),
            (_, 0, held) => format!("{held} ports are held by something else"),
            (_, failed, held) => format!("{failed} failed, {held} ports held elsewhere"),
        };
        div()
            .flex()
            .items_center()
            .gap_4()
            .px_6()
            .pt_6()
            .pb_5()
            .child(
                svg()
                    .path("icons/hexagon.svg")
                    .size(px(32.))
                    .flex_shrink_0()
                    .text_color(if summary.running > 0 {
                        self.theme.accent
                    } else {
                        self.theme.idle
                    }),
            )
            .child(
                div()
                    .flex()
                    .flex_col()
                    .gap_0p5()
                    .min_w_0()
                    .child(div().display().child(SharedString::from(format!(
                        "{} of {} running",
                        summary.running, summary.total
                    ))))
                    .child(
                        div()
                            .label()
                            .text_color(self.theme.muted)
                            .child(SharedString::from(said)),
                    ),
            )
    }

    /// What just happened, across everything rather than one service. The
    /// window is worth leaving open because of this: a crash at 20:51 is
    /// still on screen at 21:10.
    pub(super) fn stream(&self) -> impl IntoElement {
        let lines = self.happenings.iter().map(|happening| {
            div()
                .flex()
                .items_baseline()
                .gap_3()
                .px_6()
                .py_1p5()
                .child(
                    div()
                        .w(px(64.))
                        .flex_shrink_0()
                        .caption()
                        .text_color(self.theme.idle)
                        .child(ago(happening.at)),
                )
                .child(
                    div()
                        .w(px(96.))
                        .flex_shrink_0()
                        .truncate()
                        .label()
                        .text_color(self.theme.muted)
                        .child(happening.who.clone()),
                )
                .child(
                    div()
                        .flex_1()
                        .truncate()
                        .label()
                        .text_color(self.theme.text)
                        .child(happening.said.clone()),
                )
        });

        div()
            .id("stream")
            .flex()
            .flex_col()
            .flex_1()
            .min_h(px(0.))
            .overflow_y_scroll()
            .border_t_1()
            .border_color(self.theme.border)
            .child(
                div()
                    .px_6()
                    .pt_4()
                    .pb_2()
                    .caption()
                    .text_color(self.theme.idle)
                    .child(SharedString::from("Lately")),
            )
            .children(lines)
    }

    /// Status colour lives here and nowhere else. While a service is working
    /// the dot breathes, which is the only thing in the interface that
    /// repeats.
    pub(super) fn dot(&self, status: &ServiceStatus, working: bool, index: usize) -> AnyElement {
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
    pub(super) fn output(
        &self,
        index: usize,
        status: &ServiceStatus,
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
        // Only as tall as it has something to hold. Reserving the full height
        // for nothing was a large empty box with a word in the middle of it.
        let tall = if empty { QUIET_HEIGHT } else { LOG_HEIGHT };

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
            .children(empty.then(|| self.waiting(status)))
            .with_animation(
                ("open", index),
                Animation::new(MOTION).with_easing(ease_in_out),
                move |body, delta| body.h(px(tall * delta)),
            )
            .into_any_element()
    }

    /// The log with nothing in it yet, which is a sentence rather than a
    /// place.
    ///
    /// Two designs were wrong before this one. A field of grain filled the
    /// band with noise and read as damage; a bordered card was a large empty
    /// box with a word in the middle of it, which is worse, because a
    /// container with nothing in it is a promise that something failed to
    /// arrive. There is no container now. The sentence says why the log is
    /// empty, which for a stopped service is not "no output yet" but that it
    /// is not running.
    pub(super) fn waiting(&self, status: &ServiceStatus) -> impl IntoElement {
        let said = match &status.state {
            ServiceState::Ready => "running quietly, nothing said yet",
            ServiceState::Failed { .. } => "it stopped before it said anything",
            _ if status.blocked.is_some() => "not running, its port is held by something else",
            _ => "not running, so there is nothing to hear",
        };
        div()
            .flex()
            .flex_1()
            .min_h(px(0.))
            .w_full()
            .items_center()
            .justify_center()
            .caption()
            .text_color(self.theme.idle)
            .child(SharedString::from(said))
    }

    /// Snapshots and branches, where stopping and restarting already are:
    /// more of the same list rather than a mode to enter.
    pub(super) fn keeping(&self, cx: &mut Context<Self>) -> AnyElement {
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
                    // Nothing may stay pointed at an instance that is gone.
                    if skep.expanded.as_ref() == Some(&doomed) {
                        skep.toggle(doomed.clone(), cx);
                    }
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
    pub(super) fn next_name(&self, stem: &str, of: impl Fn(&Snapshot) -> String) -> String {
        let taken: Vec<String> =
            self.kept
                .iter()
                .map(&of)
                .chain(self.mirror.services().filter_map(|service| {
                    service.id.tag.as_ref().map(|tag| tag.name().to_string())
                }))
                .collect();
        (1..)
            .map(|n| format!("{stem}-{n}"))
            .find(|name| !taken.contains(name))
            .unwrap_or_else(|| format!("{stem}-1"))
    }

    pub(super) fn act(
        &self,
        label: &'static str,
        run: impl Fn(&mut Self, &mut Context<Self>) + 'static,
        cx: &mut Context<Self>,
    ) -> AnyElement {
        self.act_at((label, 0), label, run, cx)
    }

    pub(super) fn act_at(
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
            .rounded(px(CHIP))
            .cursor_pointer()
            .hover(|style| style.border_color(self.theme.accent))
            .on_click(cx.listener(move |skep, _, _, cx| run(skep, cx)))
            .child(SharedString::from(label))
            .into_any_element()
    }

    pub(super) fn copy_all(&self, cx: &mut Context<Self>) -> AnyElement {
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
    pub(super) fn copy_line(&mut self, seq: u64, cx: &mut Context<Self>) {
        let Some((_, line)) = self.logs.iter().find(|(number, _)| *number == seq) else {
            return;
        };
        cx.write_to_clipboard(ClipboardItem::new_string(line.text.clone()));
        self.copied = Some((Copied::Line(seq), Instant::now()));
        cx.notify();
    }

    pub(super) fn actions(&self, status: &ServiceStatus, id: InstanceId) -> impl IntoElement {
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

    /// The accent lives here and on a breathing dot. Nowhere else.
    pub(super) fn button(&self, label: &'static str, command: Command) -> impl IntoElement {
        let commands = self.commands.clone();
        div()
            .id(label)
            .px_2p5()
            .py_1()
            .label()
            .text_color(self.theme.accent)
            .border_1()
            .border_color(self.theme.border)
            .rounded(px(CHIP))
            .cursor_pointer()
            .hover(|style| style.border_color(self.theme.accent))
            .on_click(move |_, _, cx| {
                cx.stop_propagation();
                let _ = commands.send(command.clone());
            })
            .child(SharedString::from(label))
    }
}

//! The window. Everything rendered here derives from the replica, which
//! derives from the event stream, so the interface cannot show a state the
//! engine never reported.

use std::cell::RefCell;
use std::collections::{BTreeMap, VecDeque};
use std::rc::Rc;
use std::time::{Duration, Instant};

use comb::{Applied, InstanceId, Label, LogLine, Mirror, ServiceState, ServiceStatus, Snapshot};
use gpui::{
    Animation, AnimationExt, AnyElement, ClipboardItem, Context, FontWeight, Hsla,
    InteractiveElement, IntoElement, ParentElement, Render, ScrollHandle, SharedString,
    StatefulInteractiveElement, Styled, Subscription, Window, div, ease_in_out, pulsating_between,
    px, svg,
};
use tokio::sync::mpsc::{UnboundedReceiver, UnboundedSender};

use crate::bridge::{Bridge, Command, Update};
use crate::platform::Menubar;
use crate::preview::Preview;
use crate::theme::{Scale, Theme};

/// The three ways to look at one message: as it renders, as it arrived, and as
/// it would fare elsewhere.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum MailView {
    Rendered,
    Source,
    Checks,
}

#[derive(Clone, Copy, PartialEq)]
enum Page {
    Services,
    Sites,
    Mail,
    Settings,
}

/// The rail shows what skep is designed to have, dimmed where it does not have
/// it yet. Settings sits apart at the bottom, where settings go.
const RAIL: &[(&str, &str, Option<Page>)] = &[
    // A skep is a straw beehive and the engine inside it is called comb, so
    // the cell is the app's own shape rather than a borrowed one.
    ("Services", "hexagon", Some(Page::Services)),
    ("Sites", "globe-simple", Some(Page::Sites)),
    ("Projects", "squares-four", None),
    ("Logs", "list-dashes", None),
    ("Mail", "envelope-simple", Some(Page::Mail)),
    ("Agent", "sparkle", None),
];

/// The thin weight is eight units in a 256 unit box, so an icon lands on whole
/// device pixels only when its size divides by sixteen. At two times that
/// means 16 or 32 and nothing between, and 16 is the rail's size. Growing it
/// to 18 or 20 would soften every glyph, so extra room comes from padding.
/// The thin weight is eight units in a 256 unit box, so a glyph sits on whole
/// device pixels only at sizes that divide by sixteen: at two times that is 16
/// or 32 and nothing between. 20 is a quarter pixel off and softens very
/// slightly, which is the price of the presence it buys.
const GLYPH: f32 = 20.;

const SETTINGS_GLYPH: &str = "sliders-horizontal";
const COLLAPSE_GLYPH: &str = "sidebar-simple";

const RAIL_WIDE: f32 = 208.;

/// How far a page heading must stand clear of the traffic lights: whatever
/// width the rail is not currently covering for it.
fn clearance(rail: f32) -> f32 {
    (LIGHTS - rail).max(0.)
}

/// The band across the top of the window. The traffic lights sit in its left,
/// in the rail while the rail is open and over the page header once it is not,
/// so both have to stand this tall and leave that corner alone.
const TITLEBAR: f32 = 44.;

const LIGHTS: f32 = 84.;

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
    /// One line of a message, by its place in it. gpui has no text selection
    /// at this revision, so copying a piece has to be something you click.
    Piece(usize),
    Message,
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
    /// What the mail catcher caught, which message is open, and anything in
    /// the way of asking.
    mail: Vec<comb_services::mail::Summary>,
    unread: usize,
    opened: Option<comb_services::mail::Body>,
    mail_trouble: Option<SharedString>,
    /// The webview that shows a message as it was written. Shared, because the
    /// element that knows where the reading pane is has to tell it on every
    /// frame and cannot borrow the view to do so.
    preview: Rc<RefCell<Option<Preview>>>,
    /// Which way the open message is being looked at, and what has been asked
    /// for about it. All of it clears when a different message is opened,
    /// because none of it is true of any other one.
    mail_view: MailView,
    source: Option<String>,
    /// Whether this one message was asked to load its images.
    images_shown: bool,
    checks: Option<
        Box<(
            comb_services::mail::Compatibility,
            comb_services::mail::Links,
        )>,
    >,
    /// Where the list is scrolled to, so a bar can be drawn showing it.
    mail_scroll: ScrollHandle,
    /// Every hostname this machine serves, and anything in the way of it.
    sites: BTreeMap<String, u16>,
    site_trouble: Vec<String>,
    authority_trusted: bool,
    commands: UnboundedSender<Command>,
    updates: UnboundedReceiver<Update>,
    /// Where the rail is sliding from and to, and when it started. Keeping
    /// the start rather than a flag is what lets a toggle mid slide begin
    /// from where the rail actually is instead of jumping.
    rail_from: f32,
    rail_to: f32,
    rail_since: Instant,
    /// Changing this restarts the slide, which is how the animation is told
    /// to run again rather than only on first appearance.
    rail_moves: usize,
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
            mail: Vec::new(),
            unread: 0,
            opened: None,
            mail_trouble: None,
            preview: Rc::new(RefCell::new(None)),
            mail_view: MailView::Rendered,
            source: None,
            images_shown: false,
            checks: None,
            mail_scroll: ScrollHandle::new(),
            sites: BTreeMap::new(),
            site_trouble: Vec::new(),
            authority_trusted: false,
            commands: bridge.commands,
            updates: bridge.updates,
            rail_from: RAIL_WIDE,
            rail_to: RAIL_WIDE,
            rail_since: Instant::now(),
            rail_moves: 0,
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
                Update::Mail { messages, unread } => {
                    if self.opened.is_none()
                        && let Some(preview) = self.preview.borrow_mut().as_mut()
                    {
                        preview.hide();
                    }
                    self.mail = messages;
                    self.unread = unread;
                    self.mail_trouble = None;
                }
                Update::MailSource(source) => self.source = Some(source),
                Update::MailChecks(checks) => self.checks = Some(checks),
                Update::MailBody(body) => {
                    if let Some(preview) = self.preview.borrow_mut().as_mut() {
                        if body.html.is_empty() {
                            preview.hide();
                        } else {
                            preview.show(&body.html);
                        }
                    }
                    // A different message means everything asked about the
                    // last one is no longer about anything.
                    if self.opened.as_ref().is_none_or(|open| open.id != body.id) {
                        self.mail_view = MailView::Rendered;
                        self.source = None;
                        self.checks = None;
                        self.images_shown = false;
                    }
                    self.opened = Some(*body);
                    self.mail_trouble = None;
                }
                Update::MailTrouble(why) => {
                    self.mail = Vec::new();
                    self.opened = None;
                    self.mail_trouble = Some(SharedString::from(why));
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

/// Every url in a message, in the order they appear and without repeats. The
/// converter writes a link as its words followed by its target in brackets, so
/// finding them is a matter of reading to the next space or bracket.
fn links(text: &str) -> Vec<String> {
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

/// A small stable number from a string. Not a hash anybody should rely on,
/// just enough to give a sender the same mark every time.
fn fingerprint(text: &str) -> u32 {
    let mut sum: u32 = 2_166_136_261;
    for byte in text.trim().to_ascii_lowercase().bytes() {
        sum ^= u32::from(byte);
        sum = sum.wrapping_mul(16_777_619);
    }
    sum
}

/// The time of day out of an iso timestamp. A message caught minutes ago does
/// not need its date spelled out.
fn clock(at: &str) -> String {
    at.split('T')
        .nth(1)
        .and_then(|rest| rest.get(..5))
        .unwrap_or(at)
        .to_string()
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
    fn render(&mut self, window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        // The webview belongs to the window, so it cannot exist before one.
        if self.preview.borrow().is_none() {
            *self.preview.borrow_mut() = Preview::attach(window);
        }
        // It draws above everything gpui draws, so anywhere but the mail page
        // it has to be gone rather than merely behind.
        // It draws above everything gpui draws, so anywhere it is not wanted
        // it has to be gone rather than merely behind: covered by the source
        // or the checks is not a thing it can be.
        let wanted = self.page == Page::Mail
            && self.mail_view == MailView::Rendered
            && self
                .opened
                .as_ref()
                .is_some_and(|body| !body.html.is_empty());
        if !wanted && let Some(preview) = self.preview.borrow_mut().as_mut() {
            preview.hide();
        }

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
                Page::Mail => self.mail_page(cx),
                Page::Settings => self.settings(cx),
            })
    }
}

impl Skep {
    fn rail(&self, cx: &mut Context<Self>) -> AnyElement {
        let mut items = Vec::with_capacity(RAIL.len());
        for (index, (name, glyph, page)) in RAIL.iter().enumerate() {
            items.push(self.rail_item(index, name, glyph, *page, cx));
        }

        let (from, to, moves) = (self.rail_from, self.rail_to, self.rail_moves);
        let edge = self.theme.border;
        div()
            .flex()
            .flex_col()
            .h_full()
            .flex_shrink_0()
            // Clipped, so the words hold their shape on the way out instead of
            // rewrapping into a narrower and narrower column.
            .overflow_hidden()
            .border_r_1()
            .pb_3()
            .child(self.rail_top())
            .gap_0p5()
            .children(items)
            .child(div().flex_1())
            .child(self.rail_item(
                RAIL.len(),
                "Settings",
                SETTINGS_GLYPH,
                Some(Page::Settings),
                cx,
            ))
            .with_animation(
                ("rail", moves),
                Animation::new(MOTION).with_easing(ease_in_out),
                move |rail, delta| {
                    let width = from + (to - from) * delta;
                    // A line with nothing behind it is not an edge.
                    rail.w(px(width)).border_color(if width < 1. {
                        gpui::transparent_black()
                    } else {
                        edge
                    })
                },
            )
            .into_any_element()
    }

    /// The rail's own top band. Empty on purpose: the traffic lights are
    /// drawn over it by the window, and it drags the way a titlebar would.
    fn toggle_rail(&mut self) {
        let current = self.rail_width();
        let opening = self.rail_to == 0.;
        self.rail_from = current;
        self.rail_to = if opening { RAIL_WIDE } else { 0. };
        self.rail_since = Instant::now();
        self.rail_moves += 1;
    }

    /// Where the rail is at this instant, which is not where it is going. A
    /// toggle part way through a slide starts from this rather than from the
    /// width the last slide began at, so reversing does not jump.
    fn rail_width(&self) -> f32 {
        let progress =
            (self.rail_since.elapsed().as_secs_f32() / MOTION.as_secs_f32()).clamp(0., 1.);
        self.rail_from + (self.rail_to - self.rail_from) * ease_in_out(progress)
    }

    fn rail_top(&self) -> AnyElement {
        div()
            .id("rail-top")
            .w_full()
            .h(px(TITLEBAR))
            .flex_shrink_0()
            .on_mouse_down(gpui::MouseButton::Left, |_, window, _| {
                window.start_window_move();
            })
            .into_any_element()
    }

    /// A page's heading, with the control that shuts the rail beside it. It
    /// lives here rather than in the rail because a control that goes away
    /// when you use it cannot bring back what it took.
    ///
    /// The padding grows as the rail shrinks, by exactly as much as the rail
    /// gives up, so the traffic lights never land on the words.
    fn page_title(&self, title: &'static str, cx: &mut Context<Self>) -> AnyElement {
        let (from, to, moves) = (self.rail_from, self.rail_to, self.rail_moves);
        div()
            .flex()
            .items_center()
            .gap_2()
            .flex_shrink_0()
            .child(
                div()
                    .id("collapse")
                    .flex()
                    .items_center()
                    .justify_center()
                    .size(px(28.))
                    .flex_shrink_0()
                    .rounded_md()
                    .cursor_pointer()
                    .hover(|style| style.bg(self.theme.raised))
                    .child(
                        svg()
                            .path(format!("icons/{COLLAPSE_GLYPH}.svg"))
                            .size(px(GLYPH))
                            .text_color(self.theme.muted),
                    )
                    .on_click(cx.listener(|skep, _, _, cx| {
                        skep.toggle_rail();
                        cx.notify();
                    })),
            )
            .child(div().title().child(SharedString::from(title)))
            .with_animation(
                ("title", moves),
                Animation::new(MOTION).with_easing(ease_in_out),
                move |title, delta| title.pl(px(clearance(from + (to - from) * delta))),
            )
            .into_any_element()
    }

    fn rail_item(
        &self,
        index: usize,
        name: &'static str,
        glyph: &'static str,
        page: Option<Page>,
        cx: &mut Context<Self>,
    ) -> AnyElement {
        let built = page.is_some();
        let here = page == Some(self.page);
        let colour = if here {
            self.theme.text
        } else if built {
            self.theme.muted
        } else {
            self.theme.idle
        };

        let mut item = div()
            .id(("rail", index))
            .flex()
            .items_center()
            .w(px(RAIL_WIDE - 16.))
            .flex_shrink_0()
            .gap_3()
            .mx_2()
            .px_3()
            .py_2()
            .rounded_md()
            .text_color(colour);

        // The selected row is a surface rather than an accent fill: orange is
        // reserved for what you press and what is moving, and a whole row of
        // it would drown both.
        if here {
            item = item.bg(self.theme.raised);
        }

        item = item.child(
            svg()
                .path(format!("icons/{glyph}.svg"))
                .size(px(GLYPH))
                .flex_shrink_0()
                .text_color(if here { self.theme.accent } else { colour }),
        );

        item = item.child(
            div()
                .body()
                .min_w_0()
                .font_weight(if here {
                    FontWeight::MEDIUM
                } else {
                    FontWeight::NORMAL
                })
                .child(SharedString::from(name)),
        );

        match page {
            Some(page) => item
                .cursor_pointer()
                .hover(|style| style.bg(self.theme.raised))
                .on_click(cx.listener(move |skep, _, _, cx| {
                    skep.page = page;
                    // A page that shows something fetched asks for it on the
                    // way in rather than showing yesterday's answer.
                    if page == Page::Mail {
                        let _ = skep.commands.send(Command::Mail);
                    }
                    cx.notify();
                }))
                .into_any_element(),
            None => item.into_any_element(),
        }
    }

    /// What the mail catcher caught. The same shape as everything else here:
    /// a list of rows that open in place, because a message is one more thing
    /// to look inside rather than somewhere else to go.
    fn mail_page(&self, cx: &mut Context<Self>) -> AnyElement {
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
                .flex()
                .items_center()
                .gap_3()
                .w_full()
                .min_w_0()
                .px_6()
                .py_3()
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
                    div()
                        .w(px(220.))
                        .truncate()
                        .child(SharedString::from(message.subject.clone())),
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
                div()
                    .flex()
                    .items_center()
                    .justify_between()
                    .w_full()
                    .pl_4()
                    .pr_6()
                    .h(px(TITLEBAR))
                    .flex_shrink_0()
                    .border_b_1()
                    .border_color(theme.border)
                    .child(self.page_title("Mail", cx))
                    .child(
                        div()
                            .flex()
                            .items_center()
                            .gap_3()
                            .child(div().caption().text_color(theme.muted).child(
                                SharedString::from(if self.unread == 0 {
                                    format!("{} caught", self.mail.len())
                                } else {
                                    format!("{} unread of {}", self.unread, self.mail.len())
                                }),
                            ))
                            .children((!self.mail.is_empty()).then(|| {
                                div()
                                    .id("clear-mail")
                                    .px_2()
                                    .py_1()
                                    .rounded_md()
                                    .label()
                                    .cursor_pointer()
                                    .text_color(theme.muted)
                                    .hover(|style| style.bg(theme.raised).text_color(theme.text))
                                    .child(SharedString::from("Clear"))
                                    .on_click(cx.listener(|skep, _, _, cx| {
                                        skep.opened = None;
                                        let _ = skep.commands.send(Command::ClearMail);
                                        cx.notify();
                                    }))
                            })),
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

    /// How widely the message is supported, as a bar rather than three
    /// numbers in a row.
    ///
    /// The three parts are ordered rather than merely different, so they are
    /// drawn as one ink getting fainter rather than as three colours. That is
    /// also what keeps status colour where it belongs, which is in a row's
    /// dot and nowhere else.
    fn support(&self, clients: &comb_services::mail::Compatibility) -> AnyElement {
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
    fn sender_mark(&self, from: &str) -> AnyElement {
        let bits = fingerprint(from);
        let ink = self.theme.muted;

        let mut comb = div().flex().flex_col().gap_px().flex_shrink_0();
        for row in 0..3 {
            let mut across = div().flex().gap_px();
            for column in 0..3 {
                // The outer columns are the same, so the mark has an axis.
                let bit = row * 2 + column.min(2 - column);
                let filled = bits >> bit & 1 == 1;
                across = across.child(div().size(px(4.)).rounded_sm().bg(if filled {
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
    fn mail_columns(&self) -> AnyElement {
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
            .px_6()
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
    fn scrollbar(&self) -> Option<AnyElement> {
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
    fn reading(&self, body: &comb_services::mail::Body, cx: &mut Context<Self>) -> AnyElement {
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
                    .px_6()
                    .py_2()
                    .flex_shrink_0()
                    .child(
                        div()
                            .min_w_0()
                            .truncate()
                            .label()
                            .child(SharedString::from(body.subject.clone())),
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

    fn mail_tabs(&self, body: &comb_services::mail::Body, cx: &mut Context<Self>) -> AnyElement {
        let theme = &self.theme;
        let mut tabs = div().flex().items_center().gap_1();

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
                    .px_2()
                    .py_0p5()
                    .rounded_md()
                    .caption()
                    .cursor_pointer()
                    .text_color(if here { theme.text } else { theme.muted })
                    .bg(if here {
                        theme.base
                    } else {
                        gpui::transparent_black()
                    })
                    .hover(|style| style.text_color(theme.text))
                    .on_click(cx.listener(move |skep, _, _, cx| {
                        skep.mail_view = which;
                        // Asked for on the way in rather than kept fresh: the
                        // source never changes and the checks reach out over
                        // the network.
                        match which {
                            MailView::Source if skep.source.is_none() => {
                                let _ = skep.commands.send(Command::MailSource(id.clone()));
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
    fn rendered(&self, body: &comb_services::mail::Body, cx: &mut Context<Self>) -> AnyElement {
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
    fn held_back(&self, body: &comb_services::mail::Body, cx: &mut Context<Self>) -> AnyElement {
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
            .px_6()
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
    fn source_view(&self) -> AnyElement {
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
            .px_6()
            .py_2()
            .overflow_y_scroll()
            .child(lines)
            .into_any_element()
    }

    /// How the message would fare elsewhere. Nothing here runs until it is
    /// asked for: following the links means reaching out over the network, and
    /// a viewer that did that on its own would contradict everything the
    /// rendered view promises.
    fn checks_view(&self, cx: &mut Context<Self>) -> AnyElement {
        let theme = &self.theme;
        let Some(open) = &self.opened else {
            return self.nothing("no message").into_any_element();
        };

        let Some(found) = &self.checks else {
            let id = open.id.clone();
            return div()
                .flex()
                .flex_col()
                .flex_1()
                .min_h_0()
                .w_full()
                .px_6()
                .py_3()
                .gap_2()
                .child(div().caption().text_color(theme.muted).child(
                    SharedString::from(
                        "Checks how this message renders in real mail clients, and follows every                          link in it to see whether it answers. Following the links reaches out                          over the network, so it happens only when you ask.",
                    ),
                ))
                .child(
                    div()
                        .id("run-checks")
                        .caption()
                        .cursor_pointer()
                        .text_color(theme.accent)
                        .on_click(cx.listener(move |skep, _, _, cx| {
                            let _ = skep.commands.send(Command::MailChecks(id.clone()));
                            cx.notify();
                        }))
                        .child(SharedString::from("run the checks")),
                )
                .into_any_element();
        };

        let (clients, links) = found.as_ref();
        let mut out = div().flex().flex_col().w_full().gap_3();

        out = out.child(self.support(clients));

        for warning in clients.warnings.iter().take(8) {
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
                            .min_w_0()
                            .truncate()
                            .child(SharedString::from(warning.what.clone())),
                    )
                    .child(
                        div()
                            .caption()
                            .flex_shrink_0()
                            .text_color(theme.muted)
                            .child(SharedString::from(format!("{:.0}%", warning.supported))),
                    ),
            );
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
            .px_6()
            .py_3()
            .overflow_y_scroll()
            .child(out)
            .into_any_element()
    }

    fn close_message(&self, cx: &mut Context<Self>) -> AnyElement {
        div()
            .id("close-message")
            .caption()
            .cursor_pointer()
            .text_color(self.theme.muted)
            .on_click(cx.listener(|skep, _, _, cx| {
                skep.opened = None;
                if let Some(preview) = skep.preview.borrow_mut().as_mut() {
                    preview.hide();
                }
                cx.notify();
            }))
            .child(SharedString::from("close"))
            .into_any_element()
    }

    /// The message itself, in the same monospaced treatment the logs get:
    /// what was sent is closer to output than to prose.
    ///
    /// gpui has no text selection at this revision, so nothing here can be
    /// dragged over. Every line is a click instead, and any link in the
    /// message is pulled out and made one of its own, because a link or a code
    /// is what anyone is actually after.
    fn message(&self, body: &comb_services::mail::Body, cx: &mut Context<Self>) -> AnyElement {
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
                .rounded_sm()
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
                    .rounded_md()
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
            .px_6()
            .pb_3()
            .gap_2()
            .overflow_y_scroll()
            .child(aside)
            .child(lines)
            .into_any_element()
    }

    fn copy_message(&self, body: &comb_services::mail::Body, cx: &mut Context<Self>) -> AnyElement {
        let done = self.copied.is_some_and(|(what, _)| what == Copied::Message);
        let text = body.text.clone();
        div()
            .id("copy-message")
            .caption()
            .cursor_pointer()
            .text_color(if done {
                self.theme.text
            } else {
                self.theme.accent
            })
            .on_click(cx.listener(move |skep, _, _, cx| {
                cx.write_to_clipboard(ClipboardItem::new_string(text.clone()));
                skep.copied = Some((Copied::Message, Instant::now()));
                cx.notify();
            }))
            .child(SharedString::from(if done {
                "copied"
            } else {
                "copy message"
            }))
            .into_any_element()
    }

    fn sites_page(&self, cx: &mut Context<Self>) -> AnyElement {
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
                    .pl_4()
                    .pr_6()
                    .h(px(TITLEBAR))
                    .flex_shrink_0()
                    .border_b_1()
                    .border_color(theme.border)
                    .child(self.page_title("Sites", cx)),
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
                    .pl_4()
                    .pr_6()
                    .h(px(TITLEBAR))
                    .flex_shrink_0()
                    .border_b_1()
                    .border_color(theme.border)
                    .child(self.page_title("Settings", cx))
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
            .child(self.header(cx))
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

    fn header(&self, cx: &mut Context<Self>) -> impl IntoElement {
        let summary = self.mirror.summary();
        div()
            .flex()
            .items_center()
            .justify_between()
            .pl_4()
            .pr_6()
            .h(px(TITLEBAR))
            .flex_shrink_0()
            .border_b_1()
            .border_color(self.theme.border)
            .child(self.page_title("Services", cx))
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

#[cfg(test)]
mod tests {
    use gpui::AssetSource;

    use super::{COLLAPSE_GLYPH, RAIL, SETTINGS_GLYPH};
    use crate::icons::Icons;

    /// A glyph that does not resolve fails silently at runtime: gpui draws
    /// nothing and the element stays clickable, so the control is invisible
    /// but still works. Checking only the rail missed exactly that, because
    /// the collapse control is not in it.
    #[test]
    fn every_icon_on_disk_is_registered() {
        let directory = concat!(env!("CARGO_MANIFEST_DIR"), "/assets/icons");
        let mut checked = 0;
        for entry in std::fs::read_dir(directory).expect("the icons directory exists") {
            let path = entry.expect("a readable entry").path();
            if path.extension().is_none_or(|kind| kind != "svg") {
                continue;
            }
            let name = path.file_stem().unwrap().to_string_lossy().into_owned();
            assert!(
                Icons.load(&format!("icons/{name}.svg")).unwrap().is_some(),
                "{name}.svg is in the assets directory but not in the icons table, \
                 so anything asking for it draws nothing"
            );
            checked += 1;
        }
        assert!(checked > 0, "no icons were found to check");
    }

    #[test]
    fn every_glyph_the_rail_asks_for_exists() {
        for (name, glyph, _) in RAIL.iter().chain([&("Settings", SETTINGS_GLYPH, None)]) {
            let found = Icons
                .load(&format!("icons/{glyph}.svg"))
                .expect("looking one up cannot fail");
            let bytes = found.unwrap_or_else(|| panic!("{name} wants {glyph}, which is not here"));
            assert!(
                bytes.starts_with(b"<svg"),
                "{glyph} does not look like an svg"
            );
        }
    }

    #[test]
    fn the_collapse_control_has_a_glyph() {
        assert!(
            Icons
                .load(&format!("icons/{COLLAPSE_GLYPH}.svg"))
                .unwrap()
                .is_some()
        );
    }

    #[test]
    fn a_glyph_that_is_not_there_is_absent_rather_than_wrong() {
        assert!(Icons.load("icons/nothing-like-this.svg").unwrap().is_none());
    }
}

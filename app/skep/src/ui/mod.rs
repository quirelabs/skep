//! The window. Everything rendered here derives from the replica, which
//! derives from the event stream, so the interface cannot show a state the
//! engine never reported.

use std::cell::RefCell;
use std::collections::{BTreeMap, VecDeque};
use std::rc::Rc;
use std::time::{Duration, Instant};

use comb::{Applied, InstanceId, Label, LogLine, Mirror, ServiceState, ServiceStatus, Snapshot};
use gpui::{
    Animation, AnimationExt, AnyElement, Bounds, ClipboardItem, Context, FontWeight, Hsla,
    InteractiveElement, IntoElement, ParentElement, Pixels, Render, ScrollHandle, SharedString,
    StatefulInteractiveElement, Styled, Subscription, Window, div, ease_in_out, pulsating_between,
    px, svg,
};
use tokio::sync::mpsc::{UnboundedReceiver, UnboundedSender};

use crate::bridge::{Bridge, Command, Update};
use crate::platform::Menubar;
use crate::preview::{Held, Preview, Scout};
use crate::theme::{Scale, Theme};

/// What the screen needs to say about certificates. Not "trusted" or not, but
/// which authority, out of which home, with which fingerprint: two homes make
/// two authorities carrying the same name, and only the last of those tells
/// them apart.
mod mail;
mod paint;
mod rail;
mod services;
mod settings;
mod sites;

use mail::MailView;

/// The curve where the page meets the rail. Only that edge is rounded, so the
/// rail appears to wrap around the page rather than sit beside it. No line
/// along it: the curve and the change of surface are the edge, and a rule as
/// well made the two halves read as two documents.
const PANEL_RADIUS: f32 = 12.;
/// How much of the page's tooth shows. Enough to feel under a fingertip,
/// not enough to see as a texture.
const TOOTH: f32 = 0.11;
use paint::fingerprint;
use rail::{RAIL_WIDE, TITLEBAR};
use services::LOG_LIMIT;
use settings::Trust;
use sites::Draft;

/// Ships with every macOS, unlike the generic family names, which font-kit
/// will not resolve.
const MONO: &str = "Menlo";

#[derive(Clone, Copy, PartialEq)]
enum Page {
    Services,
    Sites,
    Mail,
    Settings,
}

/// Every motion in the interface is shorter than this. Anything slower reads
/// as the machine being slow rather than as the interface being alive.
const MOTION: Duration = Duration::from_millis(180);
const BREATH: Duration = Duration::from_millis(1100);
/// How long a copied line stays marked.
const ACKNOWLEDGED: Duration = Duration::from_millis(900);
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
    /// What has just happened, across every service rather than one.
    happenings: VecDeque<services::Happening>,
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
    /// Which authority this process holds, where it lives, and whether the
    /// machine accepts it.
    trust: Option<Trust>,
    /// Whether the window is full screen, where macOS takes the traffic
    /// lights away and the space kept clear for them is just a gap.
    fullscreen: bool,
    /// Which client-support warning is open, if any.
    open_warning: Option<usize>,
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
    /// Which sites had something answering the last time anyone looked. A site
    /// is only config until something is behind it.
    answering: BTreeMap<String, bool>,
    /// A site being written, if one is. Two fields and which of them is
    /// taking the keys.
    draft: Option<Draft>,
    entry: gpui::FocusHandle,
    /// Which site is being looked at, if any.
    site: Option<String>,
    /// A picture of each site, taken off screen. Held here rather than in the
    /// scout so a photograph outlives the site going quiet.
    shots: BTreeMap<String, std::sync::Arc<gpui::Image>>,
    scout: Rc<RefCell<Option<Scout>>>,
    /// Whether the window had focus last time it drew, so coming back to it
    /// can look again. The same deliberate act as opening the tab, rather than
    /// a timer nobody asked for.
    was_active: bool,
    /// Whether a site opens in the browser rather than in the pane.
    sites_in_browser: bool,
    /// The light in the window. One picture, stretched to fit.
    sky: Option<std::sync::Arc<gpui::Image>>,
    /// The page's own tooth, so both halves of the window are one material.
    tooth: Option<std::sync::Arc<gpui::Image>>,
    authority_trusted: bool,
    /// The port sites are reachable on, 8443 until the helper forwards 443.
    site_port: u16,
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
                        // Drawn from the palette, so they are drawn again.
                        skep.sky = None;
                        skep.tooth = None;
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
            happenings: VecDeque::new(),
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
            trust: None,
            fullscreen: false,
            open_warning: None,
            images_shown: false,
            checks: None,
            mail_scroll: ScrollHandle::new(),
            sites: BTreeMap::new(),
            site_trouble: Vec::new(),
            answering: BTreeMap::new(),
            draft: None,
            entry: cx.focus_handle(),
            site: None,
            shots: BTreeMap::new(),
            scout: Rc::new(RefCell::new(None)),
            was_active: true,
            sites_in_browser: false,
            sky: None,
            tooth: None,
            authority_trusted: false,
            site_port: comb::HTTPS_PORT,
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
                    if let Some(happening) = services::Happening::of(&event) {
                        self.happenings.push_front(happening);
                        self.happenings.truncate(services::HAPPENINGS);
                    }
                    // Falling behind is answered by asking, never by guessing.
                    if self.mirror.apply(&event) == Applied::Resync {
                        let _ = self.commands.send(Command::Resync);
                    }
                }
                Update::Trust {
                    home,
                    root,
                    fingerprint,
                    trusted,
                } => {
                    self.trust = Some(Trust {
                        home,
                        root,
                        fingerprint,
                        trusted,
                    });
                    self.authority_trusted = trusted;
                }
                Update::SiteHealth(answering) => {
                    // A site that stopped answering leaves a page behind that
                    // must not come back the moment it answers again.
                    if let Some(host) = &self.site
                        && answering.get(host) != Some(&true)
                        && let Some(preview) = self.preview.borrow_mut().as_mut()
                        && preview.holds(&Held::Site(host.clone()))
                    {
                        preview.forget();
                    }
                    // A site that has come back is not what it was when the
                    // last picture was taken.
                    for (host, alive) in &answering {
                        if *alive && self.answering.get(host) == Some(&false) {
                            self.shots.remove(host);
                        }
                    }
                    self.answering = answering;
                }
                Update::Mail { messages, unread } => {
                    // A message that is no longer in the inbox was cleared
                    // elsewhere, and the reading pane must not outlive it.
                    if let Some(open) = &self.opened
                        && !messages.iter().any(|message| message.id == open.id)
                    {
                        self.opened = None;
                    }
                    self.mail = messages;
                    self.unread = unread;
                    self.mail_trouble = None;
                }
                Update::MailSource { id, source } => {
                    if self.opened.as_ref().is_some_and(|open| open.id == id) {
                        self.source = Some(source);
                    }
                }
                Update::MailChecks { id, checks } => {
                    if self.opened.as_ref().is_some_and(|open| open.id == id) {
                        self.checks = Some(checks);
                    }
                }
                Update::MailBody(body) => {
                    // A different message means everything asked about the
                    // last one is no longer about anything.
                    if self.opened.as_ref().is_none_or(|open| open.id != body.id) {
                        self.mail_view = MailView::Rendered;
                        self.source = None;
                        self.checks = None;
                        self.images_shown = false;
                        self.open_warning = None;
                    }
                    self.opened = Some(*body);
                    self.mail_trouble = None;
                }
                // What was last known stays on screen under the sentence:
                // one failed fetch is not a reason to show an empty inbox.
                Update::MailTrouble(why) => self.mail_trouble = Some(SharedString::from(why)),
                Update::SiteList(sites) => {
                    self.sites = sites;
                    self.draft = None;
                }
                Update::SiteRefused(why) => match &mut self.draft {
                    Some(draft) => draft.complaint = Some(why),
                    None => self.site_trouble.push(why),
                },
                Update::Preferences { sites_in_browser } => {
                    self.sites_in_browser = sites_in_browser;
                }
                Update::Sites {
                    sites,
                    trouble,
                    trusted,
                    public_https,
                } => {
                    self.shots.retain(|host, _| sites.contains_key(host));
                    self.sites = sites;
                    self.site_trouble = trouble;
                    self.authority_trusted = trusted;
                    self.site_port = public_https;
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

    /// Keeps the contact sheet supplied. Asks for a picture of any site that
    /// has none, and collects whichever one the scout has finished with.
    ///
    /// Driven from the frame rather than from a timer, so the waiting for a
    /// page to settle is ordinary rust rather than something scheduled inside
    /// appkit that nothing here could cancel.
    fn photograph(&mut self, cx: &mut Context<Self>) {
        // The handle is cloned so the borrow is of the scout rather than of
        // this view, which leaves the shots free to be written below.
        let held = self.scout.clone();
        let mut held = held.borrow_mut();
        let Some(scout) = held.as_mut() else {
            return;
        };

        let wanted: Vec<(String, String)> = self
            .sites
            .keys()
            .filter(|host| !self.shots.contains_key(*host))
            .map(|host| (host.clone(), comb::site_url(host, self.site_port)))
            .collect();
        if !wanted.is_empty() {
            scout.want(wanted);
        }
        if let Some((host, png)) = scout.tick() {
            self.shots.insert(
                host,
                std::sync::Arc::new(gpui::Image::from_bytes(gpui::ImageFormat::Png, png)),
            );
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

    pub(super) fn toggle(&mut self, id: InstanceId, cx: &mut Context<Self>) {
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

impl Render for Skep {
    fn render(&mut self, window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        self.fullscreen = window.is_fullscreen();
        // Coming back to the window is as deliberate as opening the tab, so
        // the count is never older than the glance being taken at it.
        let active = window.is_window_active();
        if active && !self.was_active && self.page == Page::Sites {
            let _ = self.commands.send(Command::CheckSites);
        }
        self.was_active = active;
        // The webviews belong to the window, so they cannot exist before one.
        if self.preview.borrow().is_none() {
            *self.preview.borrow_mut() = Preview::attach(window);
        }
        if self.scout.borrow().is_none() {
            *self.scout.borrow_mut() = Scout::attach(window);
        }
        self.photograph(cx);
        // It draws above everything gpui draws, so anywhere it is not wanted
        // it has to be gone rather than merely behind: covered by the source
        // or the checks is not a thing it can be. This is the one place that
        // decides what it holds, so two pages sharing it cannot disagree.
        let wanted = match self.page {
            Page::Mail if self.mail_view == MailView::Rendered => self
                .opened
                .as_ref()
                .filter(|body| !body.html.is_empty())
                .map(|body| Held::Message {
                    id: body.id.clone(),
                    body: fingerprint(&body.html),
                }),
            // A site only shows itself when there is something behind it to
            // show. Otherwise the pane has its own answer.
            Page::Sites => self
                .site
                .as_ref()
                .filter(|host| self.answering.get(*host) == Some(&true))
                .map(|host| Held::Site(host.clone())),
            _ => None,
        };
        if let Some(preview) = self.preview.borrow_mut().as_mut() {
            match wanted {
                Some(held) if preview.holds(&held) => preview.reveal(),
                Some(held @ Held::Message { .. }) => {
                    if let Some(body) = &self.opened {
                        preview.show_message(held, &body.html);
                    }
                }
                Some(Held::Site(host)) => {
                    let url = comb::site_url(&host, self.site_port);
                    preview.show_site(host, &url);
                }
                None => preview.hide(),
            }
        }

        if self.sky.is_none() {
            self.sky = paint::sky(&self.theme);
        }
        if self.tooth.is_none() {
            self.tooth = paint::tooth(&self.theme);
        }

        div()
            .relative()
            .flex()
            .size_full()
            .bg(self.theme.backdrop())
            .text_color(self.theme.text)
            .body()
            .children(self.sky.as_ref().map(|image| {
                use gpui::StyledImage as _;
                gpui::img(image.clone())
                    .object_fit(gpui::ObjectFit::Fill)
                    .absolute()
                    .size_full()
            }))
            .child(self.rail(cx))
            .child(
                // Full height and width, so a screen's title sits on the same
                // line as the traffic lights rather than a margin below them.
                // The panel is rounded only where it meets the rail, which is
                // what makes the rail read as wrapping around it.
                div()
                    .relative()
                    .flex()
                    .flex_1()
                    .min_w_0()
                    .h_full()
                    .overflow_hidden()
                    .rounded_tl(px(PANEL_RADIUS))
                    .rounded_bl(px(PANEL_RADIUS))
                    .bg(self.theme.surface)
                    // First, so it lies under everything the page draws and
                    // nothing has to be read through it.
                    .children(self.tooth.as_ref().map(|image| {
                        use gpui::StyledImage as _;
                        gpui::img(image.clone())
                            .object_fit(gpui::ObjectFit::Fill)
                            .absolute()
                            .size_full()
                            .opacity(TOOTH)
                    }))
                    .child(match self.page {
                        Page::Services => self.content(cx),
                        Page::Sites => self.sites_page(cx),
                        Page::Mail => self.mail_page(cx),
                        Page::Settings => self.settings(cx),
                    }),
            )
    }
}

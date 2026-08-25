//! The menubar item. GPUI has no status item API at this revision, so this is
//! AppKit directly.

use std::cell::RefCell;

use comb::{Glyph, ServiceState, ServiceStatus};
use objc2::rc::Retained;
use objc2::runtime::{NSObject, NSObjectProtocol};
use objc2::{DefinedClass, MainThreadOnly, define_class, msg_send, sel};
use objc2_app_kit::{
    NSApplication, NSColor, NSMenu, NSMenuItem, NSStatusBar, NSStatusItem,
    NSVariableStatusItemLength,
};
use objc2_foundation::{MainThreadMarker, NSString};
use tokio::sync::mpsc::UnboundedSender;

use crate::bridge::Command;

struct Ivars {
    commands: UnboundedSender<Command>,
    /// One per menu item, found by the item's tag. Rebuilt with the menu.
    actions: RefCell<Vec<Command>>,
}

define_class!(
    // SAFETY: NSObject has no subclassing requirements, and this does not
    // implement Drop.
    #[unsafe(super(NSObject))]
    #[thread_kind = MainThreadOnly]
    #[name = "SkepMenubarTarget"]
    #[ivars = Ivars]
    struct Target;

    impl Target {
        #[unsafe(method(perform:))]
        fn perform(&self, sender: &NSMenuItem) {
            let tag = sender.tag() as usize;
            let command = self.ivars().actions.borrow().get(tag).cloned();
            if let Some(command) = command {
                let _ = self.ivars().commands.send(command);
            }
        }
    }

    unsafe impl NSObjectProtocol for Target {}
);

impl Target {
    fn new(mtm: MainThreadMarker, commands: UnboundedSender<Command>) -> Retained<Self> {
        let this = Self::alloc(mtm).set_ivars(Ivars {
            commands,
            actions: RefCell::new(Vec::new()),
        });
        unsafe { msg_send![super(this), init] }
    }
}

pub struct Menubar {
    item: Retained<NSStatusItem>,
    target: Retained<Target>,
    mtm: MainThreadMarker,
}

impl Menubar {
    pub fn new(mtm: MainThreadMarker, commands: UnboundedSender<Command>) -> Self {
        let bar = NSStatusBar::systemStatusBar();
        let item = bar.statusItemWithLength(NSVariableStatusItemLength);
        Self {
            item,
            target: Target::new(mtm, commands),
            mtm,
        }
    }

    /// Grey when nothing runs, green with a count when everything is healthy,
    /// orange while anything is in motion, and a different shape entirely when
    /// something has failed, because that is the one you must not miss.
    pub fn show(&self, glyph: Glyph, services: &[ServiceStatus]) {
        let (title, tint) = match glyph {
            Glyph::Idle => ("○".to_string(), NSColor::secondaryLabelColor()),
            Glyph::Running(count) => (format!("● {count}"), NSColor::systemGreenColor()),
            Glyph::Working => ("◐".to_string(), NSColor::systemOrangeColor()),
            Glyph::Failed => ("▲".to_string(), NSColor::systemRedColor()),
        };

        if let Some(button) = self.item.button(self.mtm) {
            button.setTitle(&NSString::from_str(&title));
            button.setContentTintColor(Some(&tint));
        }
        self.item.setMenu(Some(&self.menu(services)));
    }

    fn menu(&self, services: &[ServiceStatus]) -> Retained<NSMenu> {
        let menu = NSMenu::new(self.mtm);
        let mut actions = Vec::new();

        for status in services {
            let live = status.state.is_running() || status.state.is_transitional();
            let verb = if live { "Stop" } else { "Start" };
            let title = format!("{verb} {}  ({})", status.id, describe(status));

            let item = self.entry(&title, actions.len());
            actions.push(if live {
                Command::Stop(status.id.clone())
            } else {
                Command::Start(status.id.clone())
            });
            menu.addItem(&item);
        }

        menu.addItem(&NSMenuItem::separatorItem(self.mtm));
        menu.addItem(&self.quit());
        *self.target.ivars().actions.borrow_mut() = actions;
        menu
    }

    fn entry(&self, title: &str, tag: usize) -> Retained<NSMenuItem> {
        let item = unsafe {
            NSMenuItem::initWithTitle_action_keyEquivalent(
                NSMenuItem::alloc(self.mtm),
                &NSString::from_str(title),
                Some(sel!(perform:)),
                &NSString::from_str(""),
            )
        };
        unsafe { item.setTarget(Some(&*self.target)) };
        item.setTag(tag as isize);
        item
    }

    /// Quitting goes through NSApplication, so it runs the same shutdown a
    /// window quit does and services stop with their host.
    fn quit(&self) -> Retained<NSMenuItem> {
        let item = unsafe {
            NSMenuItem::initWithTitle_action_keyEquivalent(
                NSMenuItem::alloc(self.mtm),
                &NSString::from_str("Quit Skep"),
                Some(sel!(terminate:)),
                &NSString::from_str("q"),
            )
        };
        let app = NSApplication::sharedApplication(self.mtm);
        unsafe { item.setTarget(Some(&app)) };
        item
    }
}

fn describe(status: &ServiceStatus) -> String {
    if let Some(activity) = &status.activity {
        return activity.clone();
    }
    match &status.state {
        ServiceState::Ready => "running".to_string(),
        ServiceState::Failed { .. } => "failed".to_string(),
        other => other.name().to_string(),
    }
}

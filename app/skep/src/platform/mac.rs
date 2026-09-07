//! The menubar item. GPUI has no status item API at this revision, so this is
//! AppKit directly.

use std::cell::RefCell;

use comb::{Glyph, ServiceState, ServiceStatus};

/// What the shape means, for anyone reading the menu bar aloud.
/// The cell in its three states, drawn at two times eighteen points by
/// scripts/icon.py. Compiled in for the same reason the interface icons are:
/// the app is one file, and a mark cannot go missing between a build and a
/// machine.
const IDLE: &[u8] = include_bytes!("../../assets/menu/idle.png");
const RUNNING: &[u8] = include_bytes!("../../assets/menu/running.png");
const WORKING: &[u8] = include_bytes!("../../assets/menu/working.png");

/// One of those, as something the menu bar will draw.
///
/// Sized in points rather than left at its pixel size: the bytes are 36 wide
/// so a retina display has something to work with, and saying 18 is what tells
/// AppKit that is two times rather than an image twice too big.
fn template(bytes: &[u8]) -> Option<Retained<NSImage>> {
    let data = NSData::with_bytes(bytes);
    let image = NSImage::initWithData(NSImage::alloc(), &data)?;
    image.setSize(NSSize::new(18., 18.));
    Some(image)
}

fn describe_glyph(glyph: Glyph) -> String {
    match glyph {
        Glyph::Idle => "skep, nothing running".to_string(),
        Glyph::Running(count) => format!("skep, {count} running"),
        Glyph::Working => "skep, working".to_string(),
        Glyph::Failed => "skep, something failed".to_string(),
    }
}
use objc2::rc::Retained;
use objc2::runtime::{NSObject, NSObjectProtocol};
use objc2::{AnyThread, DefinedClass, MainThreadOnly, define_class, msg_send, sel};
use objc2_app_kit::{
    NSApplication, NSColor, NSImage, NSMenu, NSMenuItem, NSStatusBar, NSStatusItem,
    NSVariableStatusItemLength,
};
use objc2_foundation::{MainThreadMarker, NSData, NSSize, NSString};
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

    /// A hollow cell when nothing runs, a solid one with a count when
    /// something does, half filled while anything is in motion, and a
    /// different shape entirely when something has failed, because that is
    /// the one you must not miss.
    ///
    /// The state is carried by the shape rather than by colour, which is the
    /// menu bar's own convention and not a compromise: everything up there is
    /// a silhouette, and a coloured one reads as a notification badge.
    ///
    /// The shape is the app's own: a cell, hollow when nothing runs and solid
    /// when something does, so the state reads without colour as well as with
    /// it. It is drawn by the system rather than typed as a character, which
    /// is what makes it sit on the menu bar's baseline at the size the menu
    /// bar happens to be, on every display.
    pub fn show(&self, glyph: Glyph, services: &[ServiceStatus]) {
        // No tint on the ordinary states. A tint overrides what a template
        // image does for itself, and what it does for itself is the only
        // thing that gets this right: the menu bar has its own appearance,
        // which is dark over a dark wallpaper even while the system is light,
        // so a label colour resolved against the app drew the cell in near
        // black on a near black bar. Left alone, the system draws it in
        // whatever the bar wants, in both appearances and while a menu is
        // open over it.
        //
        // Failure keeps its colour, because that is the one worth spending an
        // exception on, and red carries on a bar of either appearance.
        // The cell, drawn by us rather than borrowed from SF Symbols. The
        // symbol set has a hexagon and it is not this hexagon: different
        // proportions, a thinner stroke, and no relation to the one on the
        // icon. The mark in the menu bar is the app's face, so it is the same
        // mark, drawn from the same geometry by scripts/icon.py.
        let (drawn, title, tint) = match glyph {
            Glyph::Idle => (Some(IDLE), String::new(), None),
            Glyph::Running(count) => (Some(RUNNING), format!(" {count}"), None),
            Glyph::Working => (Some(WORKING), String::new(), None),
            // Not a cell at all. A shape you have not been staring past all
            // day is the point of it, and the one place a colour up here is
            // worth spending: red carries on a bar of either appearance.
            Glyph::Failed => (None, String::new(), Some(NSColor::systemRedColor())),
        };

        if let Some(button) = self.item.button(self.mtm) {
            let image = match drawn {
                Some(bytes) => template(bytes),
                None => NSImage::imageWithSystemSymbolName_accessibilityDescription(
                    &NSString::from_str("exclamationmark.triangle.fill"),
                    Some(&NSString::from_str(&describe_glyph(glyph))),
                ),
            };
            match image {
                Some(image) => {
                    image.setTemplate(true);
                    button.setImage(Some(&image));
                    button.setTitle(&NSString::from_str(&title));
                }
                // Nothing to draw with still has to say something, so it says
                // it the way it always did.
                None => button.setTitle(&NSString::from_str(match glyph {
                    Glyph::Idle => "\u{25cb}",
                    Glyph::Running(_) => "\u{25cf}",
                    Glyph::Working => "\u{25d0}",
                    Glyph::Failed => "\u{25b2}",
                })),
            }
            button.setContentTintColor(tint.as_deref());
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

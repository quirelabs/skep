//! Showing a message as it was written.
//!
//! A WKWebView is a native view sitting on top of the one gpui draws into, so
//! it obeys none of gpui's layout: it cannot be clipped by a scrolling list and
//! nothing can be drawn over it. That is why a message is read in a pane of its
//! own rather than opened inside the list, and why this is told where to be on
//! every frame rather than laid out once.
//!
//! Nothing it shows may reach the network. That is not a preference: an
//! unguarded message fetches its tracking pixel the moment it opens, which
//! tells whoever sent it that you read it. The guard is in comb_services::mail.

use std::cell::{Cell, RefCell};
use std::collections::VecDeque;
use std::ptr::NonNull;
use std::rc::Rc;
use std::time::{Duration, Instant};

use block2::Block;
use gpui::{Bounds, Pixels, Window};
use objc2::rc::Retained;
use objc2::runtime::{NSObjectProtocol, ProtocolObject};
use objc2::{DefinedClass, MainThreadMarker, MainThreadOnly, define_class, msg_send};
use objc2_app_kit::{
    NSBitmapImageFileType, NSBitmapImageRep, NSColor, NSImage, NSView, NSWorkspace,
};
use objc2_foundation::{
    NSDictionary, NSError, NSObject, NSPoint, NSRect, NSSize, NSString, NSURL, NSURLRequest,
};
use objc2_web_kit::{
    WKNavigation, WKNavigationAction, WKNavigationActionPolicy, WKNavigationDelegate, WKWebView,
    WKWebViewConfiguration,
};
use raw_window_handle::{HasWindowHandle, RawWindowHandle};

define_class!(
    /// Decides what a click is allowed to do. A message is a document to read,
    /// not a place to browse: following a link inside the pane would replace
    /// the message with a web page and there would be no way back to it. The
    /// browser already does browsing, so links go there.
    #[unsafe(super(NSObject))]
    #[name = "SkepMailNavigation"]
    #[thread_kind = MainThreadOnly]
    struct Navigation;

    unsafe impl NSObjectProtocol for Navigation {}

    unsafe impl WKNavigationDelegate for Navigation {
        #[unsafe(method(webView:decidePolicyForNavigationAction:decisionHandler:))]
        fn decide(
            &self,
            _webview: &WKWebView,
            action: &WKNavigationAction,
            handler: &Block<dyn Fn(WKNavigationActionPolicy)>,
        ) {
            let target = unsafe { action.request().URL() };
            let scheme = target
                .as_ref()
                .and_then(|url| url.scheme())
                .map(|scheme| scheme.to_string())
                .unwrap_or_default();

            // A link somebody pressed leaves for the browser. The load itself
            // is allowed, and nothing else is: a form in a message must not be
            // able to send anywhere, whatever the sanitiser let through.
            let kind = unsafe { action.navigationType() };
            let pressed = kind == objc2_web_kit::WKNavigationType::LinkActivated;
            if let Some(url) = target
                && pressed
                && (scheme == "http" || scheme == "https")
            {
                NSWorkspace::sharedWorkspace().openURL(&url);
            }
            let policy = if kind == objc2_web_kit::WKNavigationType::Other {
                WKNavigationActionPolicy::Allow
            } else {
                WKNavigationActionPolicy::Cancel
            };
            (*handler).call((policy,));
        }
    }
);

impl Navigation {
    fn new(marker: MainThreadMarker) -> Retained<Self> {
        unsafe { msg_send![Self::alloc(marker), init] }
    }
}

/// What the pane holds. Two pages share one pane, so each has to be able to
/// tell whether coming back to it means a reveal or a load.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Held {
    /// A message, and a digest of the html it was given, since the same
    /// message comes back different once its images are allowed.
    Message {
        id: String,
        body: u32,
    },
    Site(String),
}

pub struct Preview {
    view: Retained<WKWebView>,
    parent: Retained<NSView>,
    /// Held because the webview does not keep its delegate alive.
    _navigation: Retained<Navigation>,
    /// Hiding does not empty it, so coming back to what it holds is a matter
    /// of showing it again rather than loading it again.
    holding: Option<Held>,
    showing: bool,
}

impl Preview {
    /// Adds a webview to the window, hidden until there is something to show.
    pub fn attach(window: &Window) -> Option<Self> {
        let marker = MainThreadMarker::new()?;
        // gpui has an inherent window_handle of its own, so the trait's is
        // named outright rather than reached through the value.
        let handle = HasWindowHandle::window_handle(window).ok()?;
        let RawWindowHandle::AppKit(appkit) = handle.as_raw() else {
            return None;
        };

        // Safety: the window owns this view and outlives anything added to it.
        let parent: Retained<NSView> = unsafe {
            let pointer: NonNull<NSView> = appkit.ns_view.cast();
            Retained::retain(pointer.as_ptr())?
        };

        let configuration = unsafe { WKWebViewConfiguration::new(marker) };
        // A message is input from outside. It has no business running anything.
        unsafe {
            configuration
                .defaultWebpagePreferences()
                .setAllowsContentJavaScript(false);
        }

        let view = unsafe {
            WKWebView::initWithFrame_configuration(
                WKWebView::alloc(marker),
                NSRect::new(NSPoint::new(0., 0.), NSSize::new(0., 0.)),
                &configuration,
            )
        };
        // Safari's inspector, on the message. Right click, inspect element,
        // on mail the app under development just sent: the cheapest useful
        // thing in the whole viewer.
        unsafe { view.setInspectable(true) };
        // So a light message does not flash against a dark window while it
        // loads. The message itself is left as it was written.
        unsafe { view.setUnderPageBackgroundColor(Some(&NSColor::whiteColor())) };

        let navigation = Navigation::new(marker);
        unsafe { view.setNavigationDelegate(Some(ProtocolObject::from_ref(&*navigation))) };

        view.setHidden(true);
        parent.addSubview(&view);

        Some(Self {
            view,
            parent,
            _navigation: navigation,
            holding: None,
            showing: false,
        })
    }

    pub fn holds(&self, held: &Held) -> bool {
        self.holding.as_ref() == Some(held)
    }

    /// Points it at a site. Only ever something on this machine that the
    /// person running skep put there themselves.
    pub fn show_site(&mut self, host: String, url: &str) {
        let Some(target) = NSURL::URLWithString(&NSString::from_str(url)) else {
            return;
        };
        let request = NSURLRequest::requestWithURL(&target);
        unsafe { self.view.loadRequest(&request) };
        self.view.setHidden(false);
        self.holding = Some(Held::Site(host));
        self.showing = true;
    }

    /// Puts a message in it. The html is guarded before it gets here.
    pub fn show_message(&mut self, held: Held, html: &str) {
        unsafe {
            self.view
                .loadHTMLString_baseURL(&NSString::from_str(html), None);
        }
        self.view.setHidden(false);
        self.holding = Some(held);
        self.showing = true;
    }

    /// Shows what is already in it. Switching to the source and back should
    /// not cost a reload, and a message that reloaded every time would flicker
    /// and refetch on every glance.
    pub fn reveal(&mut self) {
        if self.holding.is_some() && !self.showing {
            self.view.setHidden(false);
            self.showing = true;
        }
    }

    /// Drops what it holds without loading anything else, so the next look
    /// at the same thing is a fresh load rather than whatever page WebKit
    /// was left showing.
    pub fn forget(&mut self) {
        self.hide();
        self.holding = None;
    }

    pub fn hide(&mut self) {
        if self.showing {
            self.view.setHidden(true);
            self.showing = false;
        }
    }

    /// Told where to be, every frame, because gpui will not do it. The window
    /// counts down from the top and appkit counts up from the bottom, so the
    /// pane's place has to be turned over.
    pub fn place(&self, bounds: Bounds<Pixels>) {
        let height = self.parent.frame().size.height;
        let top = f64::from(bounds.origin.y);
        let tall = f64::from(bounds.size.height);
        let frame = NSRect::new(
            NSPoint::new(f64::from(bounds.origin.x), height - top - tall),
            NSSize::new(f64::from(bounds.size.width), tall),
        );
        self.view.setFrame(frame);
    }
}

impl Drop for Preview {
    fn drop(&mut self) {
        self.view.removeFromSuperview();
    }
}

/// Off-screen webviews that photograph a site so the list can show what is
/// behind each name.
///
/// A live webview cannot be tiled: it draws above everything gpui draws and
/// obeys none of its layout, so a grid of them could not scroll or clip. A
/// photograph is an ordinary image and behaves. One scout loads each site in
/// turn, off to the side of the window where nothing can see it, and hands
/// back a png.
pub struct Scout {
    view: Retained<WKWebView>,
    _delegate: Retained<Loading>,
    arrived: Rc<Cell<bool>>,
    /// Where the shutter leaves its picture. Written by appkit, on this
    /// thread, some turns of the run loop after it was asked.
    developed: Rc<RefCell<Option<Vec<u8>>>>,
    queue: VecDeque<(String, String)>,
    doing: Option<Job>,
}

struct Job {
    host: String,
    since: Instant,
    loaded: bool,
    shooting: bool,
}

/// A page keeps painting after it says it has finished, so the shutter waits.
const SETTLE: Duration = Duration::from_millis(900);
/// And gives up rather than holding the queue for a site that never answers.
const PATIENCE: Duration = Duration::from_secs(12);
/// Large enough that text in the photograph survives being shown small.
const SHOT: (f64, f64) = (1000., 625.);

impl Scout {
    pub fn attach(window: &Window) -> Option<Self> {
        let marker = MainThreadMarker::new()?;
        let handle = HasWindowHandle::window_handle(window).ok()?;
        let RawWindowHandle::AppKit(appkit) = handle.as_raw() else {
            return None;
        };
        // Safety: the window owns this view and outlives anything added to it.
        let parent: Retained<NSView> = unsafe {
            let pointer: NonNull<NSView> = appkit.ns_view.cast();
            Retained::retain(pointer.as_ptr())?
        };

        let configuration = unsafe { WKWebViewConfiguration::new(marker) };
        let view = unsafe {
            WKWebView::initWithFrame_configuration(
                WKWebView::alloc(marker),
                // Beside the window rather than hidden: a hidden view is not
                // asked to draw, and photographs of it come back empty.
                NSRect::new(
                    NSPoint::new(-(SHOT.0 + 200.), 0.),
                    NSSize::new(SHOT.0, SHOT.1),
                ),
                &configuration,
            )
        };
        let arrived = Rc::new(Cell::new(false));
        let delegate = Loading::new(marker, arrived.clone());
        unsafe { view.setNavigationDelegate(Some(ProtocolObject::from_ref(&*delegate))) };
        parent.addSubview(&view);

        Some(Self {
            view,
            _delegate: delegate,
            arrived,
            developed: Rc::new(RefCell::new(None)),
            queue: VecDeque::new(),
            doing: None,
        })
    }

    /// Asks for photographs of these, in this order. Anything already queued
    /// or in hand is left alone.
    pub fn want(&mut self, wanted: Vec<(String, String)>) {
        for (host, url) in wanted {
            let queued = self.queue.iter().any(|(known, _)| known == &host);
            let doing = self.doing.as_ref().is_some_and(|job| job.host == host);
            if !queued && !doing {
                self.queue.push_back((host, url));
            }
        }
    }

    /// Moves the queue along by one beat. Called from the same tick everything
    /// else in the window is driven by, which is what keeps the waiting in
    /// rust rather than in a timer somewhere inside appkit.
    pub fn tick(&mut self) -> Option<(String, Vec<u8>)> {
        match &mut self.doing {
            None => {
                let (host, url) = self.queue.pop_front()?;
                let target = NSURL::URLWithString(&NSString::from_str(&url))?;
                self.arrived.set(false);
                unsafe {
                    self.view
                        .loadRequest(&NSURLRequest::requestWithURL(&target))
                };
                self.doing = Some(Job {
                    host,
                    since: Instant::now(),
                    loaded: false,
                    shooting: false,
                });
                None
            }
            Some(job) => {
                if self.arrived.get() && !job.loaded {
                    job.loaded = true;
                    job.since = Instant::now();
                }
                let waited = job.since.elapsed();

                if job.shooting {
                    // The shutter answers some turns of the run loop later,
                    // so this is where the picture is collected.
                    if let Some(png) = self.developed.borrow_mut().take() {
                        let host = job.host.clone();
                        self.doing = None;
                        return Some((host, png));
                    }
                    if waited > PATIENCE {
                        self.doing = None;
                    }
                    return None;
                }
                if !job.loaded {
                    // A site that never answers must not hold the queue.
                    if waited > PATIENCE {
                        self.doing = None;
                    }
                    return None;
                }
                if waited < SETTLE {
                    return None;
                }
                job.shooting = true;
                job.since = Instant::now();
                self.shutter();
                None
            }
        }
    }

    /// Presses the shutter. The answer does not come back here: appkit calls
    /// the block later, on this thread, and the picture is collected by a
    /// later tick. Reading it straight after asking was the first version of
    /// this, and it returned nothing every time.
    fn shutter(&self) {
        let developed = self.developed.clone();
        let handler = block2::RcBlock::new(move |image: *mut NSImage, _: *mut NSError| {
            if image.is_null() {
                return;
            }
            // Safety: the block is handed a borrowed image, so it is retained
            // for as long as it is read.
            let Some(image) = (unsafe { Retained::retain(image) }) else {
                return;
            };
            *developed.borrow_mut() = png(&image);
        });
        unsafe {
            self.view
                .takeSnapshotWithConfiguration_completionHandler(None, &handler);
        }
    }
}

/// Turns an appkit image into png bytes, which is the one image format
/// everything downstream already knows how to read.
fn png(image: &NSImage) -> Option<Vec<u8>> {
    let tiff = image.TIFFRepresentation()?;
    let rep = NSBitmapImageRep::imageRepWithData(&tiff)?;
    let data = unsafe {
        rep.representationUsingType_properties(NSBitmapImageFileType::PNG, &NSDictionary::new())
    }?;
    Some(data.to_vec())
}

define_class!(
    /// Reports only that a page finished arriving. The waiting afterwards is
    /// done in the window's own tick.
    #[unsafe(super(NSObject))]
    #[name = "SkepSiteLoading"]
    #[thread_kind = MainThreadOnly]
    #[ivars = Rc<Cell<bool>>]
    struct Loading;

    unsafe impl NSObjectProtocol for Loading {}

    unsafe impl WKNavigationDelegate for Loading {
        #[unsafe(method(webView:didFinishNavigation:))]
        fn finished(&self, _webview: &WKWebView, _navigation: Option<&WKNavigation>) {
            self.ivars().set(true);
        }
    }
);

impl Loading {
    fn new(marker: MainThreadMarker, arrived: Rc<Cell<bool>>) -> Retained<Self> {
        let this = Self::alloc(marker).set_ivars(arrived);
        unsafe { msg_send![super(this), init] }
    }
}

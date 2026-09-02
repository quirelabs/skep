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

use std::ptr::NonNull;

use block2::Block;
use gpui::{Bounds, Pixels, Window};
use objc2::rc::Retained;
use objc2::runtime::{NSObjectProtocol, ProtocolObject};
use objc2::{MainThreadMarker, MainThreadOnly, define_class, msg_send};
use objc2_app_kit::{NSColor, NSView, NSWorkspace};
use objc2_foundation::{NSObject, NSPoint, NSRect, NSSize, NSString, NSURL, NSURLRequest};
use objc2_web_kit::{
    WKNavigationAction, WKNavigationActionPolicy, WKNavigationDelegate, WKWebView,
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

pub struct Preview {
    view: Retained<WKWebView>,
    parent: Retained<NSView>,
    /// Held because the webview does not keep its delegate alive.
    _navigation: Retained<Navigation>,
    /// Whether a message has been put in it. Hiding does not empty it, so
    /// coming back to it is a matter of showing it again rather than loading
    /// it again.
    loaded: bool,
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
            loaded: false,
            showing: false,
        })
    }

    /// Points it at a site. Only ever something on this machine that the
    /// person running skep put there themselves.
    pub fn show_url(&mut self, url: &str) {
        let Some(target) = NSURL::URLWithString(&NSString::from_str(url)) else {
            return;
        };
        let request = NSURLRequest::requestWithURL(&target);
        unsafe { self.view.loadRequest(&request) };
        self.view.setHidden(false);
        self.loaded = true;
        self.showing = true;
    }

    /// Puts a message in it. The html is guarded before it gets here.
    pub fn show(&mut self, html: &str) {
        unsafe {
            self.view
                .loadHTMLString_baseURL(&NSString::from_str(html), None);
        }
        self.view.setHidden(false);
        self.loaded = true;
        self.showing = true;
    }

    /// Shows what is already in it. Switching to the source and back should
    /// not cost a reload, and a message that reloaded every time would flicker
    /// and refetch on every glance.
    pub fn reveal(&mut self) {
        if self.loaded && !self.showing {
            self.view.setHidden(false);
            self.showing = true;
        }
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

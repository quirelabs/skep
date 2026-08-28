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

use gpui::{Bounds, Pixels, Window};
use objc2::MainThreadMarker;
use objc2::MainThreadOnly;
use objc2::rc::Retained;
use objc2_app_kit::NSView;
use objc2_foundation::{NSPoint, NSRect, NSSize, NSString};
use objc2_web_kit::{WKWebView, WKWebViewConfiguration};
use raw_window_handle::{HasWindowHandle, RawWindowHandle};

pub struct Preview {
    view: Retained<WKWebView>,
    parent: Retained<NSView>,
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
        view.setHidden(true);
        parent.addSubview(&view);

        Some(Self {
            view,
            parent,
            showing: false,
        })
    }

    /// Puts a message in it. The html is guarded before it gets here.
    pub fn show(&mut self, html: &str) {
        unsafe {
            self.view
                .loadHTMLString_baseURL(&NSString::from_str(html), None);
        }
        self.view.setHidden(false);
        self.showing = true;
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

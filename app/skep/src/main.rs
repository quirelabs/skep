//! Skep. The app is the host: it owns the machine's services for as long as it
//! is open, and takes them down with it when it closes.

mod bridge;
mod theme;
mod ui;

use std::sync::Arc;

use anyhow::{Context as _, Result};
use comb::{Engine, Paths};
use comb_services::{Request, catalog};
use gpui::{App, AppContext, Bounds, WindowBounds, WindowOptions, px, size};
use tokio::runtime::Runtime;
use tokio::signal::unix::{SignalKind, signal};

fn main() -> Result<()> {
    let runtime = Arc::new(Runtime::new().context("starting the runtime")?);
    let engine = Engine::with_paths(Paths::from_env());

    // Every known service, registered and stopped. Starting one is then a
    // question for the engine rather than a registration dance per click.
    runtime.block_on(async {
        for adapter in catalog() {
            let spec = adapter
                .spec(&Request::new(), engine.paths())
                .with_context(|| format!("building the {} service", adapter.name()))?;
            engine.register(spec).await?;
        }
        anyhow::Ok(())
    })?;

    let bridge = bridge::start(&runtime, engine.clone());

    // A window quit runs through on_app_quit below, but a terminated process
    // never gets there, and children left holding ports outlive the thing that
    // owned them. A crash or a SIGKILL still orphans them; nothing inside the
    // process can prevent that.
    {
        let engine = engine.clone();
        runtime.spawn(async move {
            let mut terminated = match signal(SignalKind::terminate()) {
                Ok(signal) => signal,
                Err(_) => return,
            };
            let mut interrupted = match signal(SignalKind::interrupt()) {
                Ok(signal) => signal,
                Err(_) => return,
            };
            tokio::select! {
                _ = terminated.recv() => {}
                _ = interrupted.recv() => {}
            }
            let _ = engine.stop_everything().await;
            std::process::exit(0);
        });
    }

    gpui_platform::application().run(move |cx: &mut App| {
        let bounds = Bounds::centered(None, size(px(860.), px(560.)), cx);
        let window = cx
            .open_window(
                WindowOptions {
                    window_bounds: Some(WindowBounds::Windowed(bounds)),
                    ..Default::default()
                },
                |_, cx| cx.new(|cx| ui::Skep::new(bridge, cx)),
            )
            .expect("a window");

        // Services live and die with their host, so quitting stops them in
        // dependency order and waits for that to finish.
        let stopping = engine.clone();
        let handle = runtime.clone();
        cx.on_app_quit(move |cx| {
            let _ = window.update(cx, |skep, _, cx| skep.stopping(cx));
            let engine = stopping.clone();
            let handle = handle.clone();
            async move {
                let (finished, waiting) = tokio::sync::oneshot::channel();
                handle.spawn(async move {
                    let _ = engine.stop_everything().await;
                    let _ = finished.send(());
                });
                let _ = waiting.await;
            }
        })
        .detach();

        cx.activate(true);
    });

    Ok(())
}

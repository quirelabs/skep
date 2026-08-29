//! The icon set, compiled in. Phosphor thin, which is filled geometry rather
//! than strokes: eight units thick in a 256 unit box, so a 16px icon draws a
//! half point edge that lands on one device pixel on a retina display.
//!
//! Names are what a thing is in skep, not what the file is called, so swapping
//! a glyph is one line here rather than a hunt through the interface.

use std::borrow::Cow;

use gpui::{AssetSource, Result, SharedString};

macro_rules! icons {
    ($($name:literal => $file:literal),* $(,)?) => {
        const FILES: &[(&str, &[u8])] = &[
            $(($name, include_bytes!(concat!("../assets/icons/", $file, ".svg")))),*
        ];
    };
}

icons! {
    "hexagon" => "hexagon",
    "globe-simple" => "globe-simple",
    "squares-four" => "squares-four",
    "list-dashes" => "list-dashes",
    "envelope-simple" => "envelope-simple",
    "sparkle" => "sparkle",
    "sliders-horizontal" => "sliders-horizontal",
    "circles-three" => "circles-three",
    "sidebar-simple" => "sidebar-simple",
    "paperclip" => "paperclip",
}

/// Compiled in rather than read from disk, so the app is one file and an icon
/// cannot go missing between a build and a machine.
pub struct Icons;

impl AssetSource for Icons {
    fn load(&self, path: &str) -> Result<Option<Cow<'static, [u8]>>> {
        let wanted = path.trim_start_matches("icons/").trim_end_matches(".svg");
        Ok(FILES
            .iter()
            .find(|(name, _)| *name == wanted)
            .map(|(_, bytes)| Cow::Borrowed(*bytes)))
    }

    fn list(&self, _path: &str) -> Result<Vec<SharedString>> {
        Ok(FILES
            .iter()
            .map(|(name, _)| SharedString::from(format!("icons/{name}.svg")))
            .collect())
    }
}

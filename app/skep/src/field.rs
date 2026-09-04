//! A line of text somebody can actually type into.
//!
//! Adapted from gpui's own input example, which is the only way to get the
//! behaviour a person expects without writing a text engine: a cursor you can
//! place with the mouse, a selection you can drag, copy and paste, the arrow
//! keys, and input methods. The first version of the site form drew a caret
//! into a label and read key events, which cannot do any of that, and it felt
//! exactly as far from the platform as it was.
//!
//! Boundaries are grapheme clusters rather than Rust chars, because a
//! character is what a person sees, not what the encoding counts. Backspace
//! over a flag removes the flag, and an accent stays on its letter.

use std::ops::Range;

use unicode_segmentation::UnicodeSegmentation;

use gpui::{
    App, Bounds, ClipboardItem, Context, CursorStyle, Element, ElementId, ElementInputHandler,
    Entity, EntityInputHandler, FocusHandle, Focusable, GlobalElementId, Hsla, InteractiveElement,
    IntoElement, KeyBinding, LayoutId, MouseButton, MouseDownEvent, MouseMoveEvent, MouseUpEvent,
    PaintQuad, ParentElement, Pixels, Point, Render, ShapedLine, SharedString, Style, Styled,
    TextRun, UTF16Selection, UnderlineStyle, Window, actions, div, fill, point, px, relative,
};

actions!(
    field,
    [
        Backspace,
        Delete,
        Left,
        Right,
        SelectLeft,
        SelectRight,
        SelectAll,
        Home,
        End,
        Paste,
        Cut,
        Copy,
        ShowCharacterPalette,
    ]
);

/// Bound once, against this field's own key context, so nothing here reaches
/// a window that is not typing.
pub fn bind(cx: &mut App) {
    cx.bind_keys([
        KeyBinding::new("backspace", Backspace, Some("Field")),
        KeyBinding::new("delete", Delete, Some("Field")),
        KeyBinding::new("left", Left, Some("Field")),
        KeyBinding::new("right", Right, Some("Field")),
        KeyBinding::new("shift-left", SelectLeft, Some("Field")),
        KeyBinding::new("shift-right", SelectRight, Some("Field")),
        KeyBinding::new("cmd-a", SelectAll, Some("Field")),
        KeyBinding::new("cmd-v", Paste, Some("Field")),
        KeyBinding::new("cmd-c", Copy, Some("Field")),
        KeyBinding::new("cmd-x", Cut, Some("Field")),
        KeyBinding::new("home", Home, Some("Field")),
        KeyBinding::new("end", End, Some("Field")),
        KeyBinding::new("ctrl-cmd-space", ShowCharacterPalette, Some("Field")),
    ]);
}

/// How a field is painted. Passed in rather than reached for, so the field
/// knows nothing about the app's palette.
#[derive(Clone, Copy)]
pub struct Look {
    pub text: Hsla,
    pub hint: Hsla,
    pub cursor: Hsla,
    pub selection: Hsla,
}

pub struct Field {
    focus: FocusHandle,
    content: SharedString,
    placeholder: SharedString,
    look: Look,
    selected: Range<usize>,
    reversed: bool,
    marked: Option<Range<usize>>,
    line: Option<ShapedLine>,
    bounds: Option<Bounds<Pixels>>,
    selecting: bool,
}

impl Field {
    pub fn new(placeholder: impl Into<SharedString>, look: Look, cx: &mut Context<Self>) -> Self {
        Self {
            focus: cx.focus_handle(),
            content: SharedString::default(),
            placeholder: placeholder.into(),
            look,
            selected: 0..0,
            reversed: false,
            marked: None,
            line: None,
            bounds: None,
            selecting: false,
        }
    }

    pub fn text(&self) -> &str {
        &self.content
    }

    pub fn look(&mut self, look: Look) {
        self.look = look;
    }

    /// Puts text in and leaves the cursor after it, which is what a field
    /// filled in for somebody should feel like.
    pub fn set(&mut self, text: impl Into<SharedString>, cx: &mut Context<Self>) {
        self.content = text.into();
        let end = self.content.len();
        self.selected = end..end;
        self.marked = None;
        cx.notify();
    }

    fn left(&mut self, _: &Left, _: &mut Window, cx: &mut Context<Self>) {
        if self.selected.is_empty() {
            self.move_to(self.before(self.cursor()), cx);
        } else {
            self.move_to(self.selected.start, cx)
        }
    }

    fn right(&mut self, _: &Right, _: &mut Window, cx: &mut Context<Self>) {
        if self.selected.is_empty() {
            self.move_to(self.after(self.selected.end), cx);
        } else {
            self.move_to(self.selected.end, cx)
        }
    }

    fn select_left(&mut self, _: &SelectLeft, _: &mut Window, cx: &mut Context<Self>) {
        self.select_to(self.before(self.cursor()), cx);
    }

    fn select_right(&mut self, _: &SelectRight, _: &mut Window, cx: &mut Context<Self>) {
        self.select_to(self.after(self.cursor()), cx);
    }

    fn select_all(&mut self, _: &SelectAll, _: &mut Window, cx: &mut Context<Self>) {
        self.move_to(0, cx);
        self.select_to(self.content.len(), cx)
    }

    fn home(&mut self, _: &Home, _: &mut Window, cx: &mut Context<Self>) {
        self.move_to(0, cx);
    }

    fn end(&mut self, _: &End, _: &mut Window, cx: &mut Context<Self>) {
        self.move_to(self.content.len(), cx);
    }

    fn backspace(&mut self, _: &Backspace, window: &mut Window, cx: &mut Context<Self>) {
        if self.selected.is_empty() {
            self.select_to(self.before(self.cursor()), cx)
        }
        self.replace_text_in_range(None, "", window, cx)
    }

    fn delete(&mut self, _: &Delete, window: &mut Window, cx: &mut Context<Self>) {
        if self.selected.is_empty() {
            self.select_to(self.after(self.cursor()), cx)
        }
        self.replace_text_in_range(None, "", window, cx)
    }

    fn paste(&mut self, _: &Paste, window: &mut Window, cx: &mut Context<Self>) {
        if let Some(text) = cx.read_from_clipboard().and_then(|item| item.text()) {
            // One line, whatever was on the clipboard.
            self.replace_text_in_range(None, &text.replace('\n', " "), window, cx);
        }
    }

    fn copy(&mut self, _: &Copy, _: &mut Window, cx: &mut Context<Self>) {
        if !self.selected.is_empty() {
            cx.write_to_clipboard(ClipboardItem::new_string(
                self.content[self.selected.clone()].to_string(),
            ));
        }
    }

    fn cut(&mut self, _: &Cut, window: &mut Window, cx: &mut Context<Self>) {
        if !self.selected.is_empty() {
            cx.write_to_clipboard(ClipboardItem::new_string(
                self.content[self.selected.clone()].to_string(),
            ));
            self.replace_text_in_range(None, "", window, cx)
        }
    }

    fn palette(&mut self, _: &ShowCharacterPalette, window: &mut Window, _: &mut Context<Self>) {
        window.show_character_palette();
    }

    fn pressed(&mut self, event: &MouseDownEvent, _: &mut Window, cx: &mut Context<Self>) {
        self.selecting = true;
        if event.modifiers.shift {
            self.select_to(self.index_at(event.position), cx);
        } else {
            self.move_to(self.index_at(event.position), cx)
        }
    }

    fn released(&mut self, _: &MouseUpEvent, _: &mut Window, _: &mut Context<Self>) {
        self.selecting = false;
    }

    fn dragged(&mut self, event: &MouseMoveEvent, _: &mut Window, cx: &mut Context<Self>) {
        if self.selecting {
            self.select_to(self.index_at(event.position), cx);
        }
    }

    fn move_to(&mut self, offset: usize, cx: &mut Context<Self>) {
        self.selected = offset..offset;
        cx.notify()
    }

    fn cursor(&self) -> usize {
        if self.reversed {
            self.selected.start
        } else {
            self.selected.end
        }
    }

    fn index_at(&self, at: Point<Pixels>) -> usize {
        if self.content.is_empty() {
            return 0;
        }
        let (Some(bounds), Some(line)) = (self.bounds.as_ref(), self.line.as_ref()) else {
            return 0;
        };
        boundary(
            &self.content,
            line.closest_index_for_x(at.x - bounds.left()),
        )
    }

    fn select_to(&mut self, offset: usize, cx: &mut Context<Self>) {
        if self.reversed {
            self.selected.start = offset
        } else {
            self.selected.end = offset
        };
        if self.selected.end < self.selected.start {
            self.reversed = !self.reversed;
            self.selected = self.selected.end..self.selected.start;
        }
        cx.notify()
    }

    fn before(&self, offset: usize) -> usize {
        before(&self.content, offset)
    }

    fn after(&self, offset: usize) -> usize {
        after(&self.content, offset)
    }

    fn utf8_at(&self, offset: usize) -> usize {
        let mut bytes = 0;
        let mut units = 0;
        for character in self.content.chars() {
            if units >= offset {
                break;
            }
            units += character.len_utf16();
            bytes += character.len_utf8();
        }
        bytes
    }

    fn utf16_at(&self, offset: usize) -> usize {
        let mut units = 0;
        let mut bytes = 0;
        for character in self.content.chars() {
            if bytes >= offset {
                break;
            }
            bytes += character.len_utf8();
            units += character.len_utf16();
        }
        units
    }

    fn range_to_utf16(&self, range: &Range<usize>) -> Range<usize> {
        self.utf16_at(range.start)..self.utf16_at(range.end)
    }

    fn range_from_utf16(&self, range: &Range<usize>) -> Range<usize> {
        self.utf8_at(range.start)..self.utf8_at(range.end)
    }
}

/// The boundary before an offset. Free functions rather than methods so the
/// rules about where a character starts can be tested without a window.
fn before(text: &str, offset: usize) -> usize {
    text.grapheme_indices(true)
        .map(|(at, _)| at)
        .rev()
        .find(|at| *at < offset)
        .unwrap_or(0)
}

/// The boundary after an offset, which is the end of whatever cluster the
/// offset falls inside.
fn after(text: &str, offset: usize) -> usize {
    text.grapheme_indices(true)
        .map(|(at, cluster)| at + cluster.len())
        .find(|end| *end > offset)
        .unwrap_or(text.len())
}

/// Snaps an offset back to the start of the cluster it lands in, so a click
/// never puts the cursor halfway through something drawn as one mark.
fn boundary(text: &str, offset: usize) -> usize {
    if offset >= text.len() {
        return text.len();
    }
    text.grapheme_indices(true)
        .map(|(at, _)| at)
        .rev()
        .find(|at| *at <= offset)
        .unwrap_or(0)
}

impl EntityInputHandler for Field {
    fn text_for_range(
        &mut self,
        range: Range<usize>,
        actual: &mut Option<Range<usize>>,
        _: &mut Window,
        _: &mut Context<Self>,
    ) -> Option<String> {
        let range = self.range_from_utf16(&range);
        actual.replace(self.range_to_utf16(&range));
        Some(self.content[range].to_string())
    }

    fn selected_text_range(
        &mut self,
        _: bool,
        _: &mut Window,
        _: &mut Context<Self>,
    ) -> Option<UTF16Selection> {
        Some(UTF16Selection {
            range: self.range_to_utf16(&self.selected),
            reversed: self.reversed,
        })
    }

    fn marked_text_range(&self, _: &mut Window, _: &mut Context<Self>) -> Option<Range<usize>> {
        self.marked.as_ref().map(|range| self.range_to_utf16(range))
    }

    fn unmark_text(&mut self, _: &mut Window, _: &mut Context<Self>) {
        self.marked = None;
    }

    fn replace_text_in_range(
        &mut self,
        range: Option<Range<usize>>,
        text: &str,
        _: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let range = range
            .as_ref()
            .map(|range| self.range_from_utf16(range))
            .or(self.marked.clone())
            .unwrap_or(self.selected.clone());

        self.content =
            (self.content[0..range.start].to_owned() + text + &self.content[range.end..]).into();
        self.selected = range.start + text.len()..range.start + text.len();
        self.marked.take();
        cx.notify();
    }

    fn replace_and_mark_text_in_range(
        &mut self,
        range: Option<Range<usize>>,
        text: &str,
        selected: Option<Range<usize>>,
        _: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let range = range
            .as_ref()
            .map(|range| self.range_from_utf16(range))
            .or(self.marked.clone())
            .unwrap_or(self.selected.clone());

        self.content =
            (self.content[0..range.start].to_owned() + text + &self.content[range.end..]).into();
        self.marked = (!text.is_empty()).then(|| range.start..range.start + text.len());
        self.selected = selected
            .as_ref()
            .map(|selected| self.range_from_utf16(selected))
            .map(|new| new.start + range.start..new.end + range.end)
            .unwrap_or_else(|| range.start + text.len()..range.start + text.len());
        cx.notify();
    }

    fn bounds_for_range(
        &mut self,
        range: Range<usize>,
        bounds: Bounds<Pixels>,
        _: &mut Window,
        _: &mut Context<Self>,
    ) -> Option<Bounds<Pixels>> {
        let line = self.line.as_ref()?;
        let range = self.range_from_utf16(&range);
        Some(Bounds::from_corners(
            point(bounds.left() + line.x_for_index(range.start), bounds.top()),
            point(bounds.left() + line.x_for_index(range.end), bounds.bottom()),
        ))
    }

    fn character_index_for_point(
        &mut self,
        at: Point<Pixels>,
        _: &mut Window,
        _: &mut Context<Self>,
    ) -> Option<usize> {
        let inside = self.bounds?.localize(&at)?;
        let line = self.line.as_ref()?;
        let index = boundary(&self.content, line.index_for_x(at.x - inside.x)?);
        Some(self.utf16_at(index))
    }
}

impl Focusable for Field {
    fn focus_handle(&self, _: &App) -> FocusHandle {
        self.focus.clone()
    }
}

/// The text itself, drawn by hand because a selection and a cursor are not
/// things a div can hold.
struct Written {
    field: Entity<Field>,
}

struct Drawn {
    line: Option<ShapedLine>,
    cursor: Option<PaintQuad>,
    selection: Option<PaintQuad>,
}

impl IntoElement for Written {
    type Element = Self;

    fn into_element(self) -> Self::Element {
        self
    }
}

impl Element for Written {
    type RequestLayoutState = ();
    type PrepaintState = Drawn;

    fn id(&self) -> Option<ElementId> {
        None
    }

    fn source_location(&self) -> Option<&'static core::panic::Location<'static>> {
        None
    }

    fn request_layout(
        &mut self,
        _: Option<&GlobalElementId>,
        _: Option<&gpui::InspectorElementId>,
        window: &mut Window,
        cx: &mut App,
    ) -> (LayoutId, Self::RequestLayoutState) {
        let mut style = Style::default();
        style.size.width = relative(1.).into();
        style.size.height = window.line_height().into();
        (window.request_layout(style, [], cx), ())
    }

    fn prepaint(
        &mut self,
        _: Option<&GlobalElementId>,
        _: Option<&gpui::InspectorElementId>,
        bounds: Bounds<Pixels>,
        _: &mut Self::RequestLayoutState,
        window: &mut Window,
        cx: &mut App,
    ) -> Self::PrepaintState {
        let field = self.field.read(cx);
        let content = field.content.clone();
        let selected = field.selected.clone();
        let cursor = field.cursor();
        let look = field.look;
        let style = window.text_style();

        let (shown, colour) = if content.is_empty() {
            (field.placeholder.clone(), look.hint)
        } else {
            (content, look.text)
        };

        let run = TextRun {
            len: shown.len(),
            font: style.font(),
            color: colour,
            background_color: None,
            underline: None,
            strikethrough: None,
        };
        // What an input method is still deciding about is underlined, which
        // is the convention every other field on the machine follows.
        let runs = match field.marked.as_ref() {
            Some(marked) => [
                TextRun {
                    len: marked.start,
                    ..run.clone()
                },
                TextRun {
                    len: marked.end - marked.start,
                    underline: Some(UnderlineStyle {
                        color: Some(run.color),
                        thickness: px(1.),
                        wavy: false,
                    }),
                    ..run.clone()
                },
                TextRun {
                    len: shown.len() - marked.end,
                    ..run
                },
            ]
            .into_iter()
            .filter(|run| run.len > 0)
            .collect(),
            None => vec![run],
        };

        let size = style.font_size.to_pixels(window.rem_size());
        let line = window.text_system().shape_line(shown, size, &runs, None);

        let (selection, cursor) = if selected.is_empty() {
            let at = line.x_for_index(cursor);
            (
                None,
                Some(fill(
                    Bounds::new(
                        point(bounds.left() + at, bounds.top()),
                        gpui::size(px(1.5), bounds.bottom() - bounds.top()),
                    ),
                    look.cursor,
                )),
            )
        } else {
            (
                Some(fill(
                    Bounds::from_corners(
                        point(
                            bounds.left() + line.x_for_index(selected.start),
                            bounds.top(),
                        ),
                        point(
                            bounds.left() + line.x_for_index(selected.end),
                            bounds.bottom(),
                        ),
                    ),
                    look.selection,
                )),
                None,
            )
        };
        Drawn {
            line: Some(line),
            cursor,
            selection,
        }
    }

    fn paint(
        &mut self,
        _: Option<&GlobalElementId>,
        _: Option<&gpui::InspectorElementId>,
        bounds: Bounds<Pixels>,
        _: &mut Self::RequestLayoutState,
        drawn: &mut Self::PrepaintState,
        window: &mut Window,
        cx: &mut App,
    ) {
        let focus = self.field.read(cx).focus.clone();
        window.handle_input(
            &focus,
            ElementInputHandler::new(bounds, self.field.clone()),
            cx,
        );
        if let Some(selection) = drawn.selection.take() {
            window.paint_quad(selection)
        }
        let line = drawn.line.take().expect("the line was shaped");
        let _ = line.paint(
            bounds.origin,
            window.line_height(),
            gpui::TextAlign::Left,
            None,
            window,
            cx,
        );
        if focus.is_focused(window)
            && let Some(cursor) = drawn.cursor.take()
        {
            window.paint_quad(cursor);
        }
        self.field.update(cx, |field, _| {
            field.line = Some(line);
            field.bounds = Some(bounds);
        });
    }
}

impl Render for Field {
    fn render(&mut self, _: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        div()
            .flex()
            .w_full()
            .min_w_0()
            .key_context("Field")
            .track_focus(&self.focus)
            .cursor(CursorStyle::IBeam)
            .on_action(cx.listener(Self::backspace))
            .on_action(cx.listener(Self::delete))
            .on_action(cx.listener(Self::left))
            .on_action(cx.listener(Self::right))
            .on_action(cx.listener(Self::select_left))
            .on_action(cx.listener(Self::select_right))
            .on_action(cx.listener(Self::select_all))
            .on_action(cx.listener(Self::home))
            .on_action(cx.listener(Self::end))
            .on_action(cx.listener(Self::paste))
            .on_action(cx.listener(Self::copy))
            .on_action(cx.listener(Self::cut))
            .on_action(cx.listener(Self::palette))
            .on_mouse_down(MouseButton::Left, cx.listener(Self::pressed))
            .on_mouse_up(MouseButton::Left, cx.listener(Self::released))
            .on_mouse_up_out(MouseButton::Left, cx.listener(Self::released))
            .on_mouse_move(cx.listener(Self::dragged))
            .child(Written {
                field: cx.entity().clone(),
            })
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// A flag is two code points and eight bytes, and deleting half of it
    /// leaves a lone regional indicator on screen.
    #[test]
    fn a_flag_is_one_character() {
        let flag = "🇳🇱";
        assert_eq!(flag.len(), 8);
        assert_eq!(before(flag, flag.len()), 0);
        assert_eq!(after(flag, 0), flag.len());
    }

    #[test]
    fn an_accent_stays_on_its_letter() {
        // e followed by a combining acute, not the single precomposed form.
        let text = "cafe\u{301}";
        assert_eq!(before(text, text.len()), 3);
        assert_eq!(after(text, 3), text.len());
    }

    #[test]
    fn plain_text_moves_one_letter_at_a_time() {
        assert_eq!(before("myapp.test", 6), 5);
        assert_eq!(after("myapp.test", 5), 6);
    }

    #[test]
    fn the_ends_hold() {
        assert_eq!(before("myapp", 0), 0);
        assert_eq!(after("myapp", 5), 5);
        assert_eq!(before("", 0), 0);
        assert_eq!(after("", 0), 0);
    }

    #[test]
    fn a_click_lands_on_a_boundary() {
        let text = "a🇳🇱b";
        // Anywhere inside the flag is the start of the flag.
        assert_eq!(boundary(text, 1), 1);
        assert_eq!(boundary(text, 5), 1);
        assert_eq!(boundary(text, 9), 9);
        assert_eq!(boundary(text, 99), text.len());
    }
}

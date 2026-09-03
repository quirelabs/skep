//! Things drawn rather than laid out: the dither, the snow, and the marks.

use std::f32::consts::TAU;

use super::*;

/// No signal. Cells kept or dropped by a hash of where they are and which
/// frame it is, so the field is different every frame and never repeats a
/// pattern the eye can lock onto, with a band rolling down it the way an
/// untuned set used to do.
///
/// It is only ever shown where there is genuinely nothing to show, which is
/// what makes it honest rather than decorative: a site with nothing behind it
/// is a dead channel.
pub(super) fn snow(bounds: Bounds<Pixels>, ink: Hsla, phase: f32, window: &mut Window) {
    const STEP: f32 = 3.;
    const MOST: usize = 3_000;

    let wide = f32::from(bounds.size.width);
    let tall = f32::from(bounds.size.height);
    if wide <= 0. || tall <= 0. {
        return;
    }

    // Stepped rather than smooth: static jumps, it does not fade.
    let frame = (phase * 8.).floor() as u32;
    let columns = (wide / STEP).floor().max(1.) as usize;
    let rows = (tall / STEP).floor().max(1.) as usize;
    let skip = (columns * rows).div_ceil(MOST).max(1);

    // The band, a little brighter, sliding down and round.
    let band = phase * tall;
    let mut placed = 0usize;

    for row in 0..rows {
        for column in 0..columns {
            if !(row * columns + column).is_multiple_of(skip) {
                continue;
            }
            let mut noise = frame
                .wrapping_mul(2_654_435_761)
                .wrapping_add(row as u32)
                .wrapping_mul(40_503)
                .wrapping_add(column as u32)
                .wrapping_mul(2_246_822_519);
            noise ^= noise >> 15;
            if !noise.is_multiple_of(7) {
                continue;
            }

            let y = row as f32 * STEP;
            let near_band = 1. - ((y - band).abs() / (tall * 0.18)).min(1.);
            let mut faded = ink;
            faded.a *= 0.25 + near_band * 0.5;

            window.paint_quad(gpui::fill(
                Bounds {
                    origin: gpui::point(
                        bounds.origin.x + px(column as f32 * STEP),
                        bounds.origin.y + px(y),
                    ),
                    size: gpui::size(px(STEP - 1.), px(STEP - 1.)),
                },
                faded,
            ));
            placed += 1;
            if placed >= MOST {
                return;
            }
        }
    }
}

/// Where there is nothing, in the same grain as everything else.
///
/// Specks of one strength, thinned by how far they are from the middle, so
/// the words sitting there stay the loudest thing on an empty screen. Which
/// ones light is decided by where they are rather than by chance, so the
/// field is the same field every time it is drawn and does not swim when the
/// window changes size.
///
/// This was an ordered matrix once, which was a second texture with its own
/// vocabulary sitting inside a window already made of grain. One material is
/// worth more than two good ones.
pub(super) fn dither(bounds: Bounds<Pixels>, ink: Hsla, window: &mut Window) {
    const CELL: f32 = 2.;
    const MOST: usize = 2_600;
    /// How thick the field is at the edges, before the thinning towards the
    /// middle takes any of it away.
    const MOST_DENSE: f32 = 0.5;

    let wide = f32::from(bounds.size.width);
    let tall = f32::from(bounds.size.height);
    if wide <= 0. || tall <= 0. {
        return;
    }
    let columns = (wide / CELL).floor().max(1.) as usize;
    let rows = (tall / CELL).floor().max(1.) as usize;
    // Thin the whole field rather than stopping it part way across: a texture
    // that ends half way is worse than one that is simply sparser.
    let thinning = ((columns * rows) as f32 * MOST_DENSE / MOST as f32).max(1.);

    let mut lit = 0usize;
    for row in 0..rows {
        for column in 0..columns {
            let x = column as f32 * CELL;
            let y = row as f32 * CELL;
            // Distance from the middle, so the middle stays quiet.
            let from_middle = (((x / wide) - 0.5).abs() + ((y / tall) - 0.5).abs()).min(1.);
            let density = from_middle * MOST_DENSE / thinning;
            if noise(column, row) > density {
                continue;
            }
            window.paint_quad(gpui::fill(
                Bounds {
                    origin: gpui::point(bounds.origin.x + px(x), bounds.origin.y + px(y)),
                    size: gpui::size(px(CELL), px(CELL)),
                },
                ink,
            ));
            lit += 1;
            if lit >= MOST {
                return;
            }
        }
    }
}

/// A small stable number from a string. Not a hash anybody should rely on,
/// just enough to give a sender the same mark every time.
pub(super) fn fingerprint(text: &str) -> u32 {
    let mut sum: u32 = 2_166_136_261;
    for byte in text.trim().to_ascii_lowercase().bytes() {
        sum ^= u32::from(byte);
        sum = sum.wrapping_mul(16_777_619);
    }
    sum
}

pub(super) fn faded(color: Hsla, alpha: f32) -> Hsla {
    Hsla { a: alpha, ..color }
}

/// The light in the window, drawn once into an image rather than painted
/// every frame.
///
/// It is one picture because of what it is: three colours lying in soft bands
/// that bleed into each other, with grain through all of it. Bands are
/// per-pixel arithmetic and grain is a value per pixel, and neither is a
/// rectangle, so neither can be a quad. An earlier attempt drew the grain as
/// quads and had to make each one three pixels across to keep the count sane,
/// which is how a film grain became a chequerboard.
///
/// Drawn once at a fixed size and stretched to whatever the window is. Every
/// distance in it is a fraction of the window rather than a number of pixels,
/// so it has no size of its own to get right, and a window being dragged
/// wider does not cost a picture a frame. Stretching softens the grain a
/// little, which is what grain on film does anyway.
pub(super) fn sky(theme: &Theme) -> Option<std::sync::Arc<gpui::Image>> {
    const WIDE: usize = 1100;
    const TALL: usize = 700;
    let (wide, tall) = (WIDE, TALL);
    let base = channels(theme.backdrop());
    let colours: Vec<[f32; 3]> = theme.sky.iter().copied().map(channels).collect();
    let (carry, grain) = theme.weather;

    // Each band is a line across the window that rises and falls, with a
    // thickness and a colour. Low, because the work happens at the top.
    let bands: [(f32, f32, f32, f32); 3] = [
        // centre, how far it rises, how wide it bleeds, how strong
        (0.74, 0.10, 0.30, 1.00),
        (0.90, 0.07, 0.26, 0.85),
        (0.82, 0.13, 0.34, 0.70),
    ];
    // Two waves apiece, at speeds that do not divide into one another, so the
    // line never repeats across the window and no band mirrors another.
    let waves: [(f32, f32, f32, f32); 3] = [
        (2.1, 0.0, 0.9, 1.7),
        (1.4, 2.2, 3.1, 0.4),
        (2.8, 4.1, 1.1, 5.3),
    ];

    // The wave of each band depends only on the column, so it is worked out
    // once per column rather than once per pixel.
    let mut centres = vec![[0f32; 3]; wide];
    for (column, row) in centres.iter_mut().enumerate() {
        let across = column as f32 / wide as f32;
        for (index, band) in bands.iter().enumerate() {
            let (fast, fast_phase, slow, slow_phase) = waves[index];
            let ripple = 0.68 * (across * fast * TAU + fast_phase).sin()
                + 0.32 * (across * slow * TAU + slow_phase).sin();
            row[index] = band.0 - band.1 * ripple;
        }
    }

    let mut pixels = vec![0u8; wide * tall * 3];
    for y in 0..tall {
        let down = y as f32 / tall as f32;
        // The top of the window is left alone, and the colour gathers towards
        // the bottom, which is where there is room for it.
        let low = smooth((down - 0.28) / 0.72);
        for (x, centre) in centres.iter().enumerate() {
            let mut colour = base;
            for (index, band) in bands.iter().enumerate() {
                let away = (down - centre[index]).abs() / band.2;
                if away >= 1. {
                    continue;
                }
                let weight = smooth(1. - away) * band.3 * carry * low;
                for channel in 0..3 {
                    colour[channel] += (colours[index][channel] - colour[channel]) * weight;
                }
            }
            // Grain last, so it sits in the colour rather than under it. One
            // value per pixel, which is what makes it grain and not a
            // pattern.
            let speck = (noise(x, y) - 0.5) * grain;
            let at = (y * wide + x) * 3;
            for channel in 0..3 {
                let value = ((colour[channel] + speck).clamp(0., 1.) * 255.) as u8;
                // Bitmaps are written blue first.
                pixels[at + 2 - channel] = value;
            }
        }
    }

    Some(std::sync::Arc::new(gpui::Image::from_bytes(
        gpui::ImageFormat::Bmp,
        bitmap(wide, tall, &pixels),
    )))
}

/// A smooth nought to one, so a band has no edge to see.
fn smooth(at: f32) -> f32 {
    let at = at.clamp(0., 1.);
    at * at * (3. - 2. * at)
}

/// A repeatable speck per pixel. Not a good hash, and it does not need to be:
/// it needs to look like nothing, and to look like the same nothing whenever
/// the window is drawn again at the same size.
fn noise(x: usize, y: usize) -> f32 {
    let mut value = (x as u32).wrapping_mul(374_761_393) ^ (y as u32).wrapping_mul(668_265_263);
    value = (value ^ (value >> 13)).wrapping_mul(1_274_126_177);
    ((value ^ (value >> 16)) & 0xffff) as f32 / 65_535.
}

fn channels(colour: Hsla) -> [f32; 3] {
    let rgba: gpui::Rgba = colour.into();
    [rgba.r, rgba.g, rgba.b]
}

/// The one image format that can be written without anything to write it: a
/// header and the pixels, uncompressed, bottom row first.
fn bitmap(wide: usize, tall: usize, pixels: &[u8]) -> Vec<u8> {
    const HEADER: usize = 54;
    // Every row is padded to a multiple of four bytes.
    let stride = (wide * 3).div_ceil(4) * 4;
    let size = HEADER + stride * tall;

    let mut out = Vec::with_capacity(size);
    out.extend_from_slice(b"BM");
    out.extend_from_slice(&(size as u32).to_le_bytes());
    out.extend_from_slice(&0u32.to_le_bytes());
    out.extend_from_slice(&(HEADER as u32).to_le_bytes());
    out.extend_from_slice(&40u32.to_le_bytes());
    out.extend_from_slice(&(wide as i32).to_le_bytes());
    out.extend_from_slice(&(tall as i32).to_le_bytes());
    out.extend_from_slice(&1u16.to_le_bytes());
    out.extend_from_slice(&24u16.to_le_bytes());
    out.extend_from_slice(&0u32.to_le_bytes());
    out.extend_from_slice(&((stride * tall) as u32).to_le_bytes());
    out.extend_from_slice(&2835i32.to_le_bytes());
    out.extend_from_slice(&2835i32.to_le_bytes());
    out.extend_from_slice(&0u32.to_le_bytes());
    out.extend_from_slice(&0u32.to_le_bytes());

    for y in (0..tall).rev() {
        let row = &pixels[y * wide * 3..(y + 1) * wide * 3];
        out.extend_from_slice(row);
        out.resize(out.len() + stride - wide * 3, 0);
    }
    out
}

/// The light under the cursor, made of the same grain the window is made of.
///
/// Brightness is how many specks are lit rather than how bright each one is:
/// every speck is the same strength, and the field simply thins as it goes
/// out, which is what a halftone does and what the picture behind it already
/// looks like. A light with its own smooth falloff sat on top of a grainy
/// window as a different material; this one is the window's own material,
/// gathered.
///
/// Which specks light is decided by where they are, not by chance, so the
/// field is steady while the cursor is still and slides with it rather than
/// boiling.
///
/// The light is one field over the whole window rather than an effect that
/// belongs to whatever the pointer is on. Everything within reach draws the
/// part that falls inside itself, so the rows either side of the one under
/// the cursor catch the edge of it and the window reads as one surface with a
/// light on it. Anything out of reach iterates nothing at all, so this costs
/// the same as an effect on one thing.
pub(super) fn glow(bounds: Bounds<Pixels>, at: Point<Pixels>, ink: Hsla, window: &mut Window) {
    const CELL: f32 = 2.;
    const REACH: f32 = 96.;
    /// The most specks any one thing will draw, so a tall panel under the
    /// cursor cannot cost more than a row does.
    const MOST: usize = 1_600;

    let wide = f32::from(bounds.size.width);
    let tall = f32::from(bounds.size.height);
    if wide <= 0. || tall <= 0. {
        return;
    }
    let (cursor_x, cursor_y) = (f32::from(at.x), f32::from(at.y));
    let (left, top) = (f32::from(bounds.origin.x), f32::from(bounds.origin.y));

    // Only the cells the light could reach, and only the part of those that
    // lies inside this element.
    let columns = wide / CELL;
    let rows = tall / CELL;
    let first_column = ((cursor_x - REACH - left) / CELL)
        .floor()
        .clamp(0., columns) as usize;
    let last_column = ((cursor_x + REACH - left) / CELL).ceil().clamp(0., columns) as usize;
    let first_row = ((cursor_y - REACH - top) / CELL).floor().clamp(0., rows) as usize;
    let last_row = ((cursor_y + REACH - top) / CELL).ceil().clamp(0., rows) as usize;

    let mut lit = 0usize;
    for row in first_row..last_row {
        for column in first_column..last_column {
            let x = left + column as f32 * CELL;
            let y = top + row as f32 * CELL;
            // From the middle of the cell, or the light sits half a cell up
            // and to the left of the pointer.
            let dx = x + CELL / 2. - cursor_x;
            let dy = y + CELL / 2. - cursor_y;
            let away = (dx * dx + dy * dy).sqrt() / REACH;
            if away >= 1. {
                continue;
            }
            // Squared, so the specks gather near the cursor and thin out
            // long before the edge, where the field has nothing to end on.
            let density = (1. - away) * (1. - away);
            // The speck's own position decides whether it is lit, using the
            // same noise the picture behind it is grained with.
            if noise(column, row) > density {
                continue;
            }
            window.paint_quad(gpui::fill(
                Bounds {
                    origin: gpui::point(px(x), px(y)),
                    size: gpui::size(px(CELL), px(CELL)),
                },
                ink,
            ));
            lit += 1;
            if lit >= MOST {
                return;
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::theme::Theme;

    /// Reads a bitmap back the way a decoder would, so the header this writes
    /// by hand is checked against what it claims rather than assumed.
    fn read(bytes: &[u8]) -> (usize, usize, Vec<[u8; 3]>) {
        assert_eq!(&bytes[0..2], b"BM");
        let at = |i: usize| u32::from_le_bytes(bytes[i..i + 4].try_into().unwrap()) as usize;
        assert_eq!(at(2), bytes.len(), "the file says how long it is");
        let offset = at(10);
        let wide = at(18);
        let tall = at(22);
        assert_eq!(u16::from_le_bytes(bytes[28..30].try_into().unwrap()), 24);
        let stride = (wide * 3).div_ceil(4) * 4;

        // Rows are written bottom first, so reading top down means walking
        // back up them.
        let mut pixels = Vec::with_capacity(wide * tall);
        for y in 0..tall {
            let row = offset + (tall - 1 - y) * stride;
            for x in 0..wide {
                let at = row + x * 3;
                pixels.push([bytes[at + 2], bytes[at + 1], bytes[at]]);
            }
        }
        (wide, tall, pixels)
    }

    #[test]
    fn the_sky_is_a_bitmap_a_decoder_can_read() {
        let image = sky(&Theme::dark()).expect("a sky");
        let (wide, tall, pixels) = read(&image.bytes);
        assert!(wide > 0 && tall > 0);
        assert_eq!(pixels.len(), wide * tall);
    }

    /// Drawn once, so it is allowed to cost something. Felt only at startup
    /// and when the machine changes appearance, and measured in a debug
    /// build, which is the slow case and the one that gets run.
    #[test]
    fn the_sky_is_drawn_quickly_enough_to_be_drawn_at_startup() {
        let started = std::time::Instant::now();
        let image = sky(&Theme::dark()).expect("a sky");
        let took = started.elapsed();
        assert!(!image.bytes.is_empty());
        assert!(
            took < std::time::Duration::from_millis(400),
            "the sky took {took:?}"
        );
    }

    #[test]
    fn the_colour_gathers_at_the_bottom_and_leaves_the_top_alone() {
        let image = sky(&Theme::dark()).expect("a sky");
        let (wide, tall, pixels) = read(&image.bytes);
        let warmth = |pixel: &[u8; 3]| i32::from(pixel[0]) - i32::from(pixel[2]);
        let band = wide * (tall / 10);

        let top: i32 = pixels[..band].iter().map(warmth).sum::<i32>() / band as i32;
        let bottom: i32 = pixels[pixels.len() - band..]
            .iter()
            .map(warmth)
            .sum::<i32>()
            / band as i32;

        assert!(
            bottom > top,
            "the bottom should carry the warm end of the sky: top {top}, bottom {bottom}"
        );
    }

    /// A row whose width does not divide by four is the case the padding
    /// exists for, and getting it wrong shears the picture diagonally.
    #[test]
    fn a_row_that_needs_padding_is_still_square() {
        let pixels = vec![7u8; 5 * 2 * 3];
        let bytes = bitmap(5, 2, &pixels);
        let (wide, tall, read_back) = read(&bytes);
        assert_eq!((wide, tall), (5, 2));
        assert!(read_back.iter().all(|pixel| pixel == &[7, 7, 7]));
    }
}

//! Things drawn rather than laid out: the dither, the snow, and the marks.

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

/// The dither. Cells on a fixed grid, kept or dropped by a threshold that
/// comes from where the cell is rather than from chance, which is what stops
/// it swimming when the window changes size.
///
/// It thins towards the middle so the words sitting there stay the loudest
/// thing in an empty screen, and the whole field is capped so an enormous
/// window cannot turn a texture into thousands of quads.
pub(super) fn dither(bounds: Bounds<Pixels>, ink: Hsla, window: &mut Window) {
    const STEP: f32 = 7.;
    const CELL: f32 = 1.;
    const MOST: usize = 2_400;

    let wide = f32::from(bounds.size.width);
    let tall = f32::from(bounds.size.height);
    if wide <= 0. || tall <= 0. {
        return;
    }

    let columns = (wide / STEP).floor().max(1.) as usize;
    let rows = (tall / STEP).floor().max(1.) as usize;
    // Coarsen rather than clip: a texture that stops half way across is worse
    // than one that is simply sparser.
    let skip = (columns * rows).div_ceil(MOST).max(1);

    let mut placed = 0usize;
    for row in 0..rows {
        for column in 0..columns {
            if !(row * columns + column).is_multiple_of(skip) {
                continue;
            }
            // The classic four by four ordered matrix, which is what makes the
            // field read as a texture rather than as noise.
            const MATRIX: [[u8; 4]; 4] =
                [[0, 8, 2, 10], [12, 4, 14, 6], [3, 11, 1, 9], [15, 7, 13, 5]];
            let level = MATRIX[row % 4][column % 4];

            let x = column as f32 * STEP;
            let y = row as f32 * STEP;
            // Distance from the middle, so the centre stays quiet.
            let from_middle = (((x / wide) - 0.5).abs() + ((y / tall) - 0.5).abs()).min(1.);
            if f32::from(level) / 16. > from_middle {
                continue;
            }

            let mut faded = ink;
            faded.a *= 0.5;
            window.paint_quad(gpui::fill(
                Bounds {
                    origin: gpui::point(bounds.origin.x + px(x), bounds.origin.y + px(y)),
                    size: gpui::size(px(CELL), px(CELL)),
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

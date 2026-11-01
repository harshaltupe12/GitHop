//! Tray icon rasteriser (Design §6.2). No image crates: a circle, a 5×7 bitmap
//! glyph and a status dot, anti-aliased by 4×4 supersampling into RGBA8.

use githop_core::ProfileColor;

/// sRGB colour.
pub type Rgb = [u8; 3];

/// Design §2.2 profile colours.
pub fn profile_rgb(c: ProfileColor) -> Rgb {
    match c {
        ProfileColor::Blue => [0x2F, 0x6B, 0xC9],
        ProfileColor::Violet => [0x7B, 0x5C, 0xD6],
        ProfileColor::Pink => [0xC2, 0x3A, 0x7D],
        ProfileColor::Indigo => [0x4C, 0x5B, 0xD4],
        ProfileColor::Plum => [0x9B, 0x4F, 0x96],
        ProfileColor::Slate => [0x5C, 0x66, 0x70],
        ProfileColor::Navy => [0x2B, 0x3A, 0x67],
        ProfileColor::Steel => [0x3F, 0x6E, 0x8C],
    }
}

/// `text-muted` — grey for setup / error states.
pub const TEXT_MUTED: Rgb = [0x6E, 0x6A, 0x74];
/// `blocked` (Vermillion).
pub const BLOCKED: Rgb = [0xFC, 0x4A, 0x1A];
/// `warning` (Sunshine).
pub const WARNING: Rgb = [0xF7, 0xB7, 0x33];
const WHITE: Rgb = [0xFF, 0xFF, 0xFF];

/// What to draw.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct IconSpec {
    /// Circle colour.
    pub fill: Rgb,
    /// Filled circle (true) or outline only (setup needed).
    pub filled: bool,
    /// Letter in the circle (A–Z, 0–9; anything else draws as "G").
    pub initial: char,
    /// Small dot at the bottom-right (blocked / error), if any.
    pub dot: Option<Rgb>,
}

/// Supersamples per axis.
const SS: u32 = 4;

/// Render `spec` to a `size`×`size` RGBA8 buffer (straight alpha).
pub fn render(spec: &IconSpec, size: u32) -> Vec<u8> {
    let s = size as f32;
    let c = s / 2.0;
    let r = s / 2.0 - 0.25;
    let stroke = (s / 10.0).max(1.25);

    // Glyph cell = whole pixels so the letter stays crisp at 16/32 px.
    let cell = (s / 16.0).round().max(1.0);
    let gw = 5.0 * cell;
    let gh = 7.0 * cell;
    let gx = ((s - gw) / 2.0).floor();
    let gy = ((s - gh) / 2.0).floor();
    let glyph = glyph_rows(spec.initial);

    // Status dot: bottom-right, with a transparent cut-out ring around it.
    let dot_r = (s * 0.21).max(2.5);
    let dot_c = s - dot_r - 0.25;
    let gap = (s / 16.0).max(1.0);

    let text_colour = if spec.filled { WHITE } else { spec.fill };
    let shape = Shape {
        c,
        r,
        stroke,
        cell,
        gx,
        gy,
        glyph,
        dot_r,
        dot_c,
        gap,
    };

    let mut out = Vec::with_capacity((size * size * 4) as usize);
    for py in 0..size {
        for px in 0..size {
            let mut acc = [0f32; 4]; // premultiplied
            for sy in 0..SS {
                for sx in 0..SS {
                    let x = px as f32 + (sx as f32 + 0.5) / SS as f32;
                    let y = py as f32 + (sy as f32 + 0.5) / SS as f32;
                    if let Some(rgb) = shade(x, y, &shape, spec, text_colour) {
                        acc[0] += f32::from(rgb[0]);
                        acc[1] += f32::from(rgb[1]);
                        acc[2] += f32::from(rgb[2]);
                        acc[3] += 1.0;
                    }
                }
            }
            let n = (SS * SS) as f32;
            if acc[3] == 0.0 {
                out.extend_from_slice(&[0, 0, 0, 0]);
            } else {
                out.push(to_u8(acc[0] / acc[3]));
                out.push(to_u8(acc[1] / acc[3]));
                out.push(to_u8(acc[2] / acc[3]));
                out.push(to_u8(acc[3] / n * 255.0));
            }
        }
    }
    out
}

/// Geometry shared by all samples of one render.
struct Shape {
    c: f32,
    r: f32,
    stroke: f32,
    cell: f32,
    gx: f32,
    gy: f32,
    glyph: [u8; 7],
    dot_r: f32,
    dot_c: f32,
    gap: f32,
}

/// Colour of one sample, `None` = transparent. Top layer first.
fn shade(x: f32, y: f32, g: &Shape, spec: &IconSpec, text: Rgb) -> Option<Rgb> {
    if spec.dot.is_some() {
        let d = ((x - g.dot_c).powi(2) + (y - g.dot_c).powi(2)).sqrt();
        if d <= g.dot_r {
            return spec.dot;
        }
        if d <= g.dot_r + g.gap {
            return None;
        }
    }
    let d = ((x - g.c).powi(2) + (y - g.c).powi(2)).sqrt();
    if d > g.r {
        return None;
    }
    if in_glyph(x, y, g) {
        return Some(text);
    }
    if spec.filled || d >= g.r - g.stroke {
        Some(spec.fill)
    } else {
        None
    }
}

fn in_glyph(x: f32, y: f32, g: &Shape) -> bool {
    let col = ((x - g.gx) / g.cell).floor();
    let row = ((y - g.gy) / g.cell).floor();
    if !(0.0..5.0).contains(&col) || !(0.0..7.0).contains(&row) {
        return false;
    }
    let bits = g.glyph[row as usize];
    bits & (0b1_0000 >> col as u32) != 0
}

fn to_u8(v: f32) -> u8 {
    v.round().clamp(0.0, 255.0) as u8
}

/// 5×7 bitmap rows (bit 4 = leftmost column) for A–Z and 0–9; others → "G".
pub fn glyph_rows(c: char) -> [u8; 7] {
    match c.to_ascii_uppercase() {
        'A' => [0x0E, 0x11, 0x11, 0x1F, 0x11, 0x11, 0x11],
        'B' => [0x1E, 0x11, 0x11, 0x1E, 0x11, 0x11, 0x1E],
        'C' => [0x0E, 0x11, 0x10, 0x10, 0x10, 0x11, 0x0E],
        'D' => [0x1E, 0x11, 0x11, 0x11, 0x11, 0x11, 0x1E],
        'E' => [0x1F, 0x10, 0x10, 0x1E, 0x10, 0x10, 0x1F],
        'F' => [0x1F, 0x10, 0x10, 0x1E, 0x10, 0x10, 0x10],
        'H' => [0x11, 0x11, 0x11, 0x1F, 0x11, 0x11, 0x11],
        'I' => [0x0E, 0x04, 0x04, 0x04, 0x04, 0x04, 0x0E],
        'J' => [0x07, 0x02, 0x02, 0x02, 0x02, 0x12, 0x0C],
        'K' => [0x11, 0x12, 0x14, 0x18, 0x14, 0x12, 0x11],
        'L' => [0x10, 0x10, 0x10, 0x10, 0x10, 0x10, 0x1F],
        'M' => [0x11, 0x1B, 0x15, 0x15, 0x11, 0x11, 0x11],
        'N' => [0x11, 0x11, 0x19, 0x15, 0x13, 0x11, 0x11],
        'O' => [0x0E, 0x11, 0x11, 0x11, 0x11, 0x11, 0x0E],
        'P' => [0x1E, 0x11, 0x11, 0x1E, 0x10, 0x10, 0x10],
        'Q' => [0x0E, 0x11, 0x11, 0x11, 0x15, 0x12, 0x0D],
        'R' => [0x1E, 0x11, 0x11, 0x1E, 0x14, 0x12, 0x11],
        'S' => [0x0F, 0x10, 0x10, 0x0E, 0x01, 0x01, 0x1E],
        'T' => [0x1F, 0x04, 0x04, 0x04, 0x04, 0x04, 0x04],
        'U' => [0x11, 0x11, 0x11, 0x11, 0x11, 0x11, 0x0E],
        'V' => [0x11, 0x11, 0x11, 0x11, 0x11, 0x0A, 0x04],
        'W' => [0x11, 0x11, 0x11, 0x15, 0x15, 0x15, 0x0A],
        'X' => [0x11, 0x11, 0x0A, 0x04, 0x0A, 0x11, 0x11],
        'Y' => [0x11, 0x11, 0x11, 0x0A, 0x04, 0x04, 0x04],
        'Z' => [0x1F, 0x01, 0x02, 0x04, 0x08, 0x10, 0x1F],
        '0' => [0x0E, 0x11, 0x13, 0x15, 0x19, 0x11, 0x0E],
        '1' => [0x04, 0x0C, 0x04, 0x04, 0x04, 0x04, 0x0E],
        '2' => [0x0E, 0x11, 0x01, 0x02, 0x04, 0x08, 0x1F],
        '3' => [0x1F, 0x02, 0x04, 0x02, 0x01, 0x11, 0x0E],
        '4' => [0x02, 0x06, 0x0A, 0x12, 0x1F, 0x02, 0x02],
        '5' => [0x1F, 0x10, 0x1E, 0x01, 0x01, 0x11, 0x0E],
        '6' => [0x06, 0x08, 0x10, 0x1E, 0x11, 0x11, 0x0E],
        '7' => [0x1F, 0x01, 0x02, 0x04, 0x08, 0x08, 0x08],
        '8' => [0x0E, 0x11, 0x11, 0x0E, 0x11, 0x11, 0x0E],
        '9' => [0x0E, 0x11, 0x11, 0x0F, 0x01, 0x02, 0x0C],
        // 'G' and every other character.
        _ => [0x0E, 0x11, 0x10, 0x17, 0x11, 0x11, 0x0F],
    }
}

/// Tray icon edge in physical pixels for a monitor scale factor (16/20/24/32).
pub fn tray_px(scale: f64) -> u32 {
    match scale {
        s if s >= 1.75 => 32,
        s if s >= 1.375 => 24,
        s if s >= 1.125 => 20,
        _ => 16,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn px(buf: &[u8], size: u32, x: u32, y: u32) -> [u8; 4] {
        let i = ((y * size + x) * 4) as usize;
        [buf[i], buf[i + 1], buf[i + 2], buf[i + 3]]
    }

    fn spec(dot: Option<Rgb>) -> IconSpec {
        IconSpec {
            fill: profile_rgb(ProfileColor::Violet),
            filled: true,
            initial: 'P',
            dot,
        }
    }

    #[test]
    fn sizes_and_transparent_corners() {
        for size in [16, 20, 24, 32] {
            let buf = render(&spec(None), size);
            assert_eq!(buf.len(), (size * size * 4) as usize);
            assert_eq!(px(&buf, size, 0, 0)[3], 0, "corner must be transparent");
        }
    }

    #[test]
    fn filled_circle_has_profile_colour_and_white_letter() {
        let size = 32;
        let buf = render(&spec(None), size);
        // Left edge middle row: inside the circle, outside the glyph.
        assert_eq!(px(&buf, size, 4, 16), [0x7B, 0x5C, 0xD6, 255]);
        // "P" top-left stroke at glyph origin (cell 2 px, glyph 10×14 at 11,9).
        assert_eq!(px(&buf, size, 11, 10), [255, 255, 255, 255]);
    }

    #[test]
    fn dot_is_drawn_bottom_right() {
        let size = 32;
        let buf = render(&spec(Some(BLOCKED)), size);
        let p = px(&buf, size, 26, 26);
        assert_eq!(&p[..3], &BLOCKED);
        assert_eq!(p[3], 255);
        let none = render(&spec(None), size);
        assert_ne!(&px(&none, size, 26, 26)[..3], &BLOCKED);
    }

    #[test]
    fn outline_leaves_centre_empty() {
        let size = 32;
        let s = IconSpec {
            fill: TEXT_MUTED,
            filled: false,
            initial: 'G',
            dot: None,
        };
        let buf = render(&s, size);
        // Between ring and glyph: transparent.
        assert_eq!(px(&buf, size, 7, 16)[3], 0);
        // Ring at the far left: grey.
        assert_eq!(&px(&buf, size, 1, 16)[..3], &TEXT_MUTED);
    }

    #[test]
    fn unknown_letters_fall_back_to_g() {
        assert_eq!(glyph_rows('é'), glyph_rows('G'));
        assert_eq!(glyph_rows('a'), glyph_rows('A'));
    }

    #[test]
    fn tray_sizes_follow_scale() {
        assert_eq!(tray_px(1.0), 16);
        assert_eq!(tray_px(1.25), 20);
        assert_eq!(tray_px(1.5), 24);
        assert_eq!(tray_px(2.0), 32);
        assert_eq!(tray_px(3.0), 32);
    }
}

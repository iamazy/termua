use gpui::{Hsla, Rgba};
use gpui_component::ThemeColor;

use crate::{NamedColor, TermColor};

/// Converts an 8 bit ANSI color to its GPUI equivalent.
/// Accepts `usize` for compatibility with the `alacritty::Colors` interface,
/// Other than that use case, should only be called with values in the `[0,255]` range
pub fn get_color_at_index(index: usize, colors: &ThemeColor) -> Hsla {
    match index {
        // 0-15 are the same as the named colors above
        0 => colors.foreground,
        1 => colors.red,
        2 => colors.green,
        3 => colors.yellow,
        4 => colors.blue,
        5 => colors.magenta,
        6 => colors.cyan,
        7 => colors.background,
        // Bright black is widely used as a "dim grey" (e.g. fish autosuggestions).
        8 => colors.muted_foreground,
        9 => colors.red_light,
        10 => colors.green_light,
        11 => colors.yellow_light,
        12 => colors.blue_light,
        13 => colors.magenta_light,
        14 => colors.cyan_light,
        15 => colors.background,
        // 16-231 are a 6x6x6 RGB color cube, mapped to 0-255 using steps defined by XTerm.
        // See: https://github.com/xterm-x11/xterm-snapshots/blob/master/256colres.pl
        16..=231 => {
            let (r, g, b) = rgb_for_index(index as u8);
            rgba_color(
                if r == 0 { 0 } else { r * 40 + 55 },
                if g == 0 { 0 } else { g * 40 + 55 },
                if b == 0 { 0 } else { b * 40 + 55 },
            )
        }
        // 232-255 are a 24-step grayscale ramp from (8, 8, 8) to (238, 238, 238).
        232..=255 => {
            let i = index as u8 - 232; // Align index to 0..24
            let value = i * 10 + 8;
            rgba_color(value, value, value)
        }
        // For compatibility with the alacritty::Colors interface
        // See: https://github.com/alacritty/alacritty/blob/master/alacritty_terminal/src/term/color.rs
        256 => colors.foreground,
        257 => colors.background,
        258 => colors.caret,
        259 => colors.foreground,
        260 => colors.red_light,
        261 => colors.green_light,
        262 => colors.yellow_light,
        263 => colors.blue_light,
        264 => colors.magenta_light,
        265 => colors.cyan_light,
        266 => colors.background,
        267 => colors.foreground,
        268 => colors.foreground, // 'Dim Background', non-standard color

        _ => gpui::black(),
    }
}

/// Convert terminal colors (named/indexed/RGB) into GPUI colors.
pub fn convert_color(fg: &TermColor, colors: &ThemeColor) -> Hsla {
    match fg {
        TermColor::Named(n) => match n {
            NamedColor::Black => colors.foreground,
            NamedColor::Red => colors.red,
            NamedColor::Green => colors.green,
            NamedColor::Yellow => colors.yellow,
            NamedColor::Blue => colors.blue,
            NamedColor::Magenta => colors.magenta,
            NamedColor::Cyan => colors.cyan,
            NamedColor::White => colors.background,
            // Many tools (including fish autosuggestions) use "bright black" as a dim/grey.
            // Mapping it to `foreground` makes it fully black in light themes and too bright in
            // dark themes. Prefer the theme's muted foreground instead.
            NamedColor::BrightBlack => colors.muted_foreground,
            NamedColor::BrightRed => colors.red_light,
            NamedColor::BrightGreen => colors.green_light,
            NamedColor::BrightYellow => colors.yellow_light,
            NamedColor::BrightBlue => colors.blue_light,
            NamedColor::BrightMagenta => colors.magenta_light,
            NamedColor::BrightCyan => colors.cyan_light,
            NamedColor::BrightWhite => colors.background,
            NamedColor::Foreground => colors.foreground,
            NamedColor::Background => colors.background,
            NamedColor::Cursor => colors.caret,
        },
        TermColor::Rgb(r, g, b) => rgba_color(*r, *g, *b),
        TermColor::Indexed(i) => get_color_at_index(*i as usize, colors),
    }
}

/// Generates the RGB channels in [0, 5] for a given index into the 6x6x6 ANSI color cube.
///
/// See: [8 bit ANSI color](https://en.wikipedia.org/wiki/escape_code#8-bit).
///
/// Wikipedia gives a formula for calculating the index for a given color:
///
/// ```text
/// index = 16 + 36 × r + 6 × g + b (0 ≤ r, g, b ≤ 5)
/// ```
///
/// This function does the reverse, calculating the `r`, `g`, and `b` components from a given index.
fn rgb_for_index(i: u8) -> (u8, u8, u8) {
    debug_assert!((16..=231).contains(&i));
    let i = i - 16;
    let r = (i - (i % 36)) / 36;
    let g = ((i % 36) - (i % 6)) / 6;
    let b = (i % 36) % 6;
    (r, g, b)
}

fn rgba_color(r: u8, g: u8, b: u8) -> Hsla {
    Rgba {
        r: (r as f32 / 255.),
        g: (g as f32 / 255.),
        b: (b as f32 / 255.),
        a: 1.,
    }
    .into()
}

#[cfg(test)]
mod tests {
    use gpui::opaque_grey;

    use super::*;

    #[test]
    fn bright_black_maps_to_muted_foreground() {
        let colors = ThemeColor {
            foreground: gpui::black(),
            muted_foreground: opaque_grey(0.6, 1.0),
            ..ThemeColor::default()
        };

        assert_eq!(
            convert_color(&TermColor::Named(NamedColor::BrightBlack), &colors),
            colors.muted_foreground
        );
        assert_eq!(
            convert_color(&TermColor::Indexed(8), &colors),
            colors.muted_foreground
        );
    }
}

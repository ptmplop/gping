//! Colours.

use anyhow::{anyhow, Result};
use std::str::FromStr;
use tui::style::Color;

pub type Rgb = (u8, u8, u8);

/// Linear blend from `a` (t = 0) to `b` (t = 1).
pub fn mix(a: Rgb, b: Rgb, t: f64) -> Rgb {
    let t = if t.is_nan() { 0.0 } else { t.clamp(0.0, 1.0) };
    let channel = |x: u8, y: u8| (x as f64 + (y as f64 - x as f64) * t).round() as u8;
    (channel(a.0, b.0), channel(a.1, b.1), channel(a.2, b.2))
}

pub fn rgb((r, g, b): Rgb) -> Color {
    Color::Rgb(r, g, b)
}

/// Host colours, in the order they're handed out.
pub const SERIES: [Rgb; 8] = [
    (0x38, 0xbd, 0xf8),
    (0xc0, 0x84, 0xfc),
    (0xa3, 0xe6, 0x35),
    (0xf4, 0x72, 0xb6),
    (0x2d, 0xd4, 0xbf),
    (0xfb, 0x92, 0x3c),
    (0x81, 0x8c, 0xf8),
    (0xcb, 0xd5, 0xe1),
];

/// A colour for each of `count` hosts: the user's `--color` choices first, in order, then
/// the palette, skipping entries that would be hard to tell apart from a colour the user
/// chose. Each comes with an RGB approximation for blending; a named colour such as `red`
/// is still drawn as the terminal's own red, the approximation only shades fills.
pub fn series_colors(user: &[String], count: usize) -> Result<Vec<(Color, Rgb)>> {
    let mut colors = Vec::with_capacity(count);
    for name in user.iter().take(count) {
        let color = Color::from_str(name)
            .map_err(|err| anyhow!("Invalid color code: `{name}` ({err})"))?;
        colors.push((color, approximate(color)));
    }
    let taken: Vec<Rgb> = colors.iter().map(|&(_, rgb)| rgb).collect();
    let mut palette = SERIES
        .iter()
        .copied()
        .filter(|&candidate| taken.iter().all(|&t| distance(candidate, t) > 60.0))
        .collect::<Vec<_>>();
    if palette.is_empty() {
        palette = SERIES.to_vec();
    }
    // Past the end of the palette colours repeat; the lanes and labels still tell hosts apart.
    let mut next = palette.iter().cycle();
    while colors.len() < count {
        let rgb = *next.next().expect("the palette is never empty");
        colors.push((Color::Rgb(rgb.0, rgb.1, rgb.2), rgb));
    }
    Ok(colors)
}

fn distance(a: Rgb, b: Rgb) -> f64 {
    let d = |x: u8, y: u8| (x as f64 - y as f64).powi(2);
    (d(a.0, b.0) + d(a.1, b.1) + d(a.2, b.2)).sqrt()
}

/// The RGB a colour is usually shown as: the xterm defaults for the named and indexed ones.
pub fn approximate(color: Color) -> Rgb {
    const NAMED: [Rgb; 16] = [
        (0, 0, 0),
        (205, 0, 0),
        (0, 205, 0),
        (205, 205, 0),
        (0, 0, 238),
        (205, 0, 205),
        (0, 205, 205),
        (229, 229, 229),
        (127, 127, 127),
        (255, 0, 0),
        (0, 255, 0),
        (255, 255, 0),
        (92, 92, 255),
        (255, 0, 255),
        (0, 255, 255),
        (255, 255, 255),
    ];
    const CUBE: [u8; 6] = [0, 95, 135, 175, 215, 255];
    let index = match color {
        Color::Rgb(r, g, b) => return (r, g, b),
        Color::Indexed(i) => i,
        Color::Black => 0,
        Color::Red => 1,
        Color::Green => 2,
        Color::Yellow => 3,
        Color::Blue => 4,
        Color::Magenta => 5,
        Color::Cyan => 6,
        Color::Gray => 7,
        Color::DarkGray => 8,
        Color::LightRed => 9,
        Color::LightGreen => 10,
        Color::LightYellow => 11,
        Color::LightBlue => 12,
        Color::LightMagenta => 13,
        Color::LightCyan => 14,
        Color::White => 15,
        Color::Reset => 7,
    };
    match index {
        0..=15 => NAMED[index as usize],
        16..=231 => {
            let i = index - 16;
            (CUBE[(i / 36) as usize], CUBE[((i / 6) % 6) as usize], CUBE[(i % 6) as usize])
        }
        _ => {
            let v = 8 + 10 * (index - 232);
            (v, v, v)
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn mixing() {
        assert_eq!(mix((0, 0, 0), (255, 255, 255), 0.0), (0, 0, 0));
        assert_eq!(mix((0, 0, 0), (255, 255, 255), 1.0), (255, 255, 255));
        assert_eq!(mix((0, 100, 200), (100, 0, 200), 0.5), (50, 50, 200));
        assert_eq!(mix((10, 10, 10), (20, 20, 20), 7.0), (20, 20, 20));
        assert_eq!(mix((10, 10, 10), (20, 20, 20), f64::NAN), (10, 10, 10));
    }

    #[test]
    fn user_colours_come_first_and_keep_their_form() {
        let colors = series_colors(&["red".to_string(), "#102030".to_string()], 3).unwrap();
        assert_eq!(colors[0].0, Color::Red);
        assert_eq!(colors[1].0, Color::Rgb(0x10, 0x20, 0x30));
        assert_eq!(colors[2].0, rgb(SERIES[0]));
    }

    #[test]
    fn invalid_colours_are_errors() {
        assert!(series_colors(&["not-a-colour".to_string()], 1).is_err());
    }

    // The old colour picker compared enum values, so `-c green` and the automatic
    // Indexed(2) it then handed to the next host were the same green on screen.
    #[test]
    fn automatic_colours_avoid_the_users_choices() {
        let sky = format!("#{:02x}{:02x}{:02x}", SERIES[0].0, SERIES[0].1, SERIES[0].2);
        let colors = series_colors(&[sky], 3).unwrap();
        assert!(colors[1..].iter().all(|&(_, rgb)| rgb != SERIES[0]));
    }

    // It used to walk Color::Indexed(2..) with an unchecked unwrap: hundreds of hosts
    // overflowed it, and the 257th hung the program.
    #[test]
    fn any_number_of_hosts_gets_a_colour() {
        let colors = series_colors(&[], 300).unwrap();
        assert_eq!(colors.len(), 300);
        assert_eq!(colors[8], colors[0]);
    }

    #[test]
    fn approximations() {
        assert_eq!(approximate(Color::Red), (205, 0, 0));
        assert_eq!(approximate(Color::Indexed(196)), (255, 0, 0));
        assert_eq!(approximate(Color::Indexed(244)), (128, 128, 128));
    }
}

//! The latency plot: a custom braille renderer.
//!
//! ratatui's `Chart` can't do what gping needs: a lost ping has to break the line (its `NaN`
//! sample is otherwise drawn as a jump to the top of the chart), the area under a line
//! wants a tinted background, lost pings want a curtain behind them, and overlapping hosts
//! need a deliberate choice of which colour a shared cell gets. So the plot rasterises each
//! series onto a grid of sub-cell "dots" (2x4 per cell for braille, 1x1 for
//! `--simple-graphics`) and composes the cells itself.
//!
//! Layers, back to front: guide dots, the background glow under each line, the curtain
//! behind lost pings, and the lines.

use crate::theme::{mix, Rgb};
use tui::buffer::Buffer;
use tui::layout::Rect;
use tui::style::Color;

/// How finely a cell is divided.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Marker {
    /// 2x4 dots per cell using the Unicode braille block.
    Braille,
    /// One dot per cell, drawn as `•` (`--simple-graphics`).
    Dot,
}

impl Marker {
    fn resolution(self) -> (usize, usize) {
        match self {
            Marker::Braille => (2, 4),
            Marker::Dot => (1, 1),
        }
    }
}

/// A background tint under a series' line, down to the floor of the plot. It starts at the
/// line's typical level (a rolling median) rather than at the line itself, so a spike stays
/// a crisp stroke instead of growing a tinted tower underneath.
#[derive(Debug, Clone, Copy)]
pub struct Glow {
    pub tint: Rgb,
    /// How much of the tint is blended in at the top of the fill...
    pub top: f64,
    /// ...and at the floor.
    pub bottom: f64,
}

/// Cells either side whose line height the fill's top follows.
const FILL_MEDIAN_REACH: usize = 3;
/// Distinct tint strengths. Fewer means longer runs of identical cells, which keeps frames
/// small; six is already indistinguishable from a smooth gradient.
const FILL_LEVELS: f64 = 6.0;

pub struct Series<'a> {
    /// `(timestamp in seconds, latency in microseconds or NaN)`, in chronological order.
    pub samples: &'a [(f64, f64)],
    /// Colour of the line. Not necessarily RGB: a user's `--color red` stays the terminal's
    /// own red.
    pub color: Color,
    /// Overrides the line colour by the latency a row of the plot stands for, so a line can
    /// change colour as it climbs.
    pub heat: Option<&'a dyn Fn(f64) -> Option<Color>>,
    pub glow: Option<Glow>,
    /// Colour for the cell holding the newest reply, to show where the line is being written.
    pub head: Option<Color>,
}

pub struct Plot<'a> {
    pub series: Vec<Series<'a>>,
    pub x_bounds: [f64; 2],
    pub y_bounds: [f64; 2],
    pub marker: Marker,
    /// Values at which to draw a faint guide.
    pub grid: Vec<f64>,
    pub grid_color: Color,
    /// The background the tints are blended against.
    pub base: Rgb,
    /// Background for the columns that hold a lost ping.
    pub curtain: Option<Color>,
}

/// What the plot drew where, for the widgets that annotate it.
#[derive(Debug, Default)]
pub struct Drawn {
    /// Seconds of history per dot column: the plot scrolls by one dot this often.
    pub slot: f64,
    /// Per series, the plot columns (relative to the plot's left edge) holding a lost ping.
    pub lost: Vec<Vec<u16>>,
    /// Per series, the cell (relative to the plot) holding the newest reply.
    pub heads: Vec<Option<(u16, u16)>>,
    /// Per series, the cell holding the largest reply on screen, and that reply.
    pub peaks: Vec<Option<(u16, u16, f64)>>,
}

/// One series rasterised onto the dot grid.
struct Layer {
    dots: Vec<bool>,
    /// For each dot column, the highest dot row the line reaches (smaller is higher).
    tops: Vec<Option<usize>>,
    lost: Vec<bool>,
    head: Option<(i64, i64)>,
    peak: Option<(i64, i64, f64)>,
}

struct Raster {
    width: usize,
    height: usize,
    res: (usize, usize),
    x_bounds: [f64; 2],
    y_bounds: [f64; 2],
}

impl Raster {
    /// Maps a timestamp to a dot column, unclamped. The window `(x0, x1]` is cut into one
    /// slot per dot column, and slots are anchored to whole multiples of their duration
    /// rather than to the window's edge: as the window scrolls, every point steps left by
    /// exactly one dot at the same moment, instead of each one rounding to its neighbour at
    /// a slightly different time (which makes a scrolling line shimmer). Between steps the
    /// plot doesn't change at all, so nothing has to be redrawn.
    fn x(&self, t: f64) -> i64 {
        let [x0, x1] = self.x_bounds;
        let slot = (x1 - x0) / self.width as f64;
        ((t / slot).floor() - (x1 / slot).floor()) as i64 + self.width as i64 - 1
    }

    /// Maps a latency to a dot row (0 at the top), unclamped but kept within a few plot
    /// heights so off-scale values can't make line drawing loop for long.
    fn y(&self, value: f64) -> i64 {
        let [y0, y1] = self.y_bounds;
        let h = self.height as f64 - 1.0;
        let row = ((y1 - value) / (y1 - y0) * h).round();
        row.clamp(-2.0 * h - 2.0, 3.0 * h + 2.0) as i64
    }

    /// The latency a dot row stands for (its centre).
    fn value_at(&self, row: f64) -> f64 {
        let [y0, y1] = self.y_bounds;
        let h = (self.height as f64 - 1.0).max(1.0);
        y1 - row / h * (y1 - y0)
    }

    fn layer(&self, samples: &[(f64, f64)]) -> Layer {
        let mut layer = Layer {
            dots: vec![false; self.width * self.height],
            tops: vec![None; self.width],
            lost: vec![false; self.width],
            head: None,
            peak: None,
        };
        let set = |x: i64, y: i64, layer: &mut Layer| {
            if x < 0 || y < 0 || x as usize >= self.width || y as usize >= self.height {
                return;
            }
            let (x, y) = (x as usize, y as usize);
            layer.dots[y * self.width + x] = true;
            let top = &mut layer.tops[x];
            *top = Some(top.map_or(y, |t| t.min(y)));
        };

        // Only the samples that can touch the window, plus one on either side so the line
        // runs into the edges instead of stopping short of them.
        let [x0, x1] = self.x_bounds;
        let first = samples.partition_point(|(t, _)| *t <= x0).saturating_sub(1);
        let last = (samples.partition_point(|(t, _)| *t <= x1) + 1).min(samples.len());

        let mut previous: Option<(i64, i64)> = None;
        for &(t, v) in &samples[first..last] {
            let x = self.x(t);
            let visible = x >= 0 && (x as usize) < self.width;
            if v.is_nan() {
                // A lost ping breaks the line: nothing connects the replies either side.
                previous = None;
                if visible {
                    layer.lost[x as usize] = true;
                }
                continue;
            }
            let point = (x, self.y(v));
            match previous {
                Some(from) => line(from, point, |x, y| set(x, y, &mut layer)),
                None => set(point.0, point.1, &mut layer),
            }
            previous = Some(point);
            if visible {
                layer.head = Some(point);
                if layer.peak.is_none_or(|(_, _, peak)| v > peak) {
                    layer.peak = Some((point.0, point.1, v));
                }
            }
        }
        layer
    }

    /// Whether the dot at `(dx, dy)` within the cell at `(cx, cy)` is set in the layer.
    fn dot(&self, layer: &Layer, cx: usize, cy: usize, dx: usize, dy: usize) -> bool {
        layer.dots[(cy * self.res.1 + dy) * self.width + cx * self.res.0 + dx]
    }

    /// The cell holding the dot `(x, y)`, if it is inside the plot.
    fn cell(&self, (x, y): (i64, i64)) -> Option<(u16, u16)> {
        let inside = x >= 0 && y >= 0 && (x as usize) < self.width && (y as usize) < self.height;
        inside.then(|| {
            (
                (x as usize / self.res.0) as u16,
                (y as usize / self.res.1) as u16,
            )
        })
    }
}

/// Bresenham's line, calling `plot` for every dot from `a` to `b` inclusive.
fn line((mut x, mut y): (i64, i64), (x1, y1): (i64, i64), mut plot: impl FnMut(i64, i64)) {
    let dx = (x1 - x).abs();
    let dy = -(y1 - y).abs();
    let sx = if x < x1 { 1 } else { -1 };
    let sy = if y < y1 { 1 } else { -1 };
    let mut err = dx + dy;
    loop {
        plot(x, y);
        if x == x1 && y == y1 {
            return;
        }
        let e2 = 2 * err;
        if e2 >= dy {
            err += dy;
            x += sx;
        }
        if e2 <= dx {
            err += dx;
            y += sy;
        }
    }
}

/// Braille dot bit for the dot at column `dx` (0..2), row `dy` (0..4) of a cell.
const BRAILLE_BITS: [[u8; 4]; 2] = [[0x01, 0x02, 0x04, 0x40], [0x08, 0x10, 0x20, 0x80]];

fn braille(bits: u8) -> char {
    char::from_u32(0x2800 + bits as u32).unwrap_or(' ')
}

impl Plot<'_> {
    pub fn render(&self, area: Rect, buf: &mut Buffer) -> Drawn {
        let area = area.intersection(buf.area);
        let [x0, x1] = self.x_bounds;
        let [y0, y1] = self.y_bounds;
        // Written this way round so NaN bounds count as invalid too.
        let valid = area.width > 0 && area.height > 0 && x1 > x0 && y1 > y0;
        if !valid {
            return Drawn::default();
        }
        let (rx, ry) = self.marker.resolution();
        let raster = Raster {
            width: area.width as usize * rx,
            height: area.height as usize * ry,
            res: (rx, ry),
            x_bounds: self.x_bounds,
            y_bounds: [y0, y1],
        };
        let (w, h) = (area.width as usize, area.height as usize);
        let at = |cx: usize, cy: usize| (area.x + cx as u16, area.y + cy as u16);

        // Guides: a single dot every other column, present but quiet.
        for &value in &self.grid {
            let row = raster.y(value);
            if row < 0 || row as usize >= raster.height {
                continue;
            }
            let (cy, dy) = (row as usize / ry, row as usize % ry);
            for cx in (0..w).step_by(2) {
                let glyph = match self.marker {
                    Marker::Braille => braille(BRAILLE_BITS[0][dy]),
                    Marker::Dot => '·',
                };
                buf[at(cx, cy)].set_char(glyph).set_fg(self.grid_color);
            }
        }

        let layers: Vec<Layer> = self
            .series
            .iter()
            .map(|s| raster.layer(s.samples))
            .collect();

        // Glows: per cell, the strongest of the series' tints wins, so hues never mix.
        let mut glow: Vec<Option<(f64, Rgb)>> = vec![None; w * h];
        for (series, layer) in self.series.iter().zip(&layers) {
            let Some(g) = series.glow else { continue };
            // The replies in each cell column, to find the line's typical level by.
            let mut values: Vec<Vec<f64>> = vec![Vec::new(); w];
            for &(t, v) in series.samples {
                let x = raster.x(t);
                if !v.is_nan() && x >= 0 && (x as usize) < raster.width {
                    values[x as usize / rx].push(v);
                }
            }
            for cx in 0..w {
                // The highest dot of the line in this column.
                let Some(top) = (0..rx).filter_map(|dx| layer.tops[cx * rx + dx]).min() else {
                    continue;
                };
                // The fill's top is the median reply around here (a spike doesn't move a
                // median, however many columns it covers), but never above the line itself.
                let reach =
                    cx.saturating_sub(FILL_MEDIAN_REACH)..(cx + FILL_MEDIAN_REACH + 1).min(w);
                let mut around: Vec<f64> = values[reach].iter().flatten().copied().collect();
                let anchor = if around.is_empty() {
                    top
                } else {
                    around.sort_by(f64::total_cmp);
                    let typical = raster
                        .y(around[around.len() / 2])
                        .clamp(0, raster.height as i64 - 1);
                    (typical as usize).max(top)
                };
                let (first_row, offset) = (anchor / ry, anchor % ry);
                let depth = (h - first_row).max(1) as f64;
                for cy in first_row..h {
                    let mut alpha = g.top + (g.bottom - g.top) * (cy - first_row) as f64 / depth;
                    if cy == first_row {
                        // Only part of the first cell is under the fill's top.
                        alpha *= 1.0 - offset as f64 / ry as f64;
                    }
                    let alpha = (alpha / g.top * FILL_LEVELS).round() / FILL_LEVELS * g.top;
                    if alpha <= 0.0 {
                        continue;
                    }
                    let cell = &mut glow[cy * w + cx];
                    if cell.is_none_or(|(a, _)| alpha > a) {
                        *cell = Some((alpha, g.tint));
                    }
                }
            }
        }
        for cy in 0..h {
            for cx in 0..w {
                if let Some((alpha, tint)) = glow[cy * w + cx] {
                    buf[at(cx, cy)].set_bg(Color::from(mix(self.base, tint, alpha)));
                }
            }
        }

        // Curtains behind lost pings, over the glows.
        let mut drawn = Drawn {
            slot: (x1 - x0) / raster.width as f64,
            ..Drawn::default()
        };
        for layer in &layers {
            let mut columns: Vec<u16> = (0..w)
                .filter(|cx| (0..rx).any(|dx| layer.lost[cx * rx + dx]))
                .map(|cx| cx as u16)
                .collect();
            columns.dedup();
            if let Some(curtain) = self.curtain {
                for &cx in &columns {
                    for cy in 0..h {
                        buf[at(cx as usize, cy)].set_bg(curtain);
                    }
                }
            }
            drawn.lost.push(columns);
        }

        // Lines. A cell shows the union of all the lines through it, coloured by the first
        // host among them, so the first host stays on top where lines cross.
        let mut bits = vec![0u8; w * h];
        let mut owner: Vec<Option<usize>> = vec![None; w * h];
        for (index, layer) in layers.iter().enumerate() {
            for cy in 0..h {
                for cx in 0..w {
                    let mut cell_bits = 0u8;
                    for (dx, column) in BRAILLE_BITS.iter().enumerate().take(rx) {
                        for (dy, bit) in column.iter().enumerate().take(ry) {
                            if raster.dot(layer, cx, cy, dx, dy) {
                                cell_bits |= match self.marker {
                                    Marker::Braille => *bit,
                                    Marker::Dot => 1,
                                };
                            }
                        }
                    }
                    if cell_bits != 0 {
                        bits[cy * w + cx] |= cell_bits;
                        owner[cy * w + cx].get_or_insert(index);
                    }
                }
            }
        }
        let heads: Vec<Option<(u16, u16)>> = layers
            .iter()
            .map(|l| l.head.and_then(|p| raster.cell(p)))
            .collect();
        for cy in 0..h {
            // The latency this row of cells stands for, for heat colouring.
            let value = raster.value_at((cy * ry) as f64 + (ry as f64 - 1.0) / 2.0);
            for cx in 0..w {
                let Some(index) = owner[cy * w + cx] else {
                    continue;
                };
                let series = &self.series[index];
                let glyph = match self.marker {
                    Marker::Braille => braille(bits[cy * w + cx]),
                    Marker::Dot => '•',
                };
                let is_head = heads[index] == Some((cx as u16, cy as u16));
                let color = match (is_head, series.head) {
                    (true, Some(head)) => head,
                    _ => series
                        .heat
                        .and_then(|heat| heat(value))
                        .unwrap_or(series.color),
                };
                buf[at(cx, cy)].set_char(glyph).set_fg(color);
            }
        }

        drawn.heads = heads;
        drawn.peaks = layers
            .iter()
            .map(|l| {
                l.peak
                    .and_then(|(x, y, v)| raster.cell((x, y)).map(|(cx, cy)| (cx, cy, v)))
            })
            .collect();
        drawn
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn plot(samples: &[(f64, f64)]) -> Plot<'_> {
        Plot {
            series: vec![Series {
                samples,
                color: Color::Rgb(255, 0, 0),
                heat: None,
                glow: None,
                head: None,
            }],
            x_bounds: [0.0, 10.0],
            y_bounds: [0.0, 100.0],
            marker: Marker::Braille,
            grid: vec![],
            grid_color: Color::Rgb(40, 40, 40),
            base: (0, 0, 0),
            curtain: None,
        }
    }

    fn render(p: &Plot, w: u16, h: u16) -> (Buffer, Drawn) {
        let mut buf = Buffer::empty(Rect::new(0, 0, w, h));
        let drawn = p.render(buf.area, &mut buf);
        (buf, drawn)
    }

    fn row(buf: &Buffer, y: u16) -> String {
        (0..buf.area.width)
            .map(|x| buf[(x, y)].symbol().to_string())
            .collect()
    }

    fn is_blank(buf: &Buffer) -> bool {
        buf.content.iter().all(|c| c.symbol() == " ")
    }

    // The old renderer handed NaN samples to ratatui's Chart, which maps them to the top row:
    // a host that never answered showed up as a flat line at the maximum latency.
    #[test]
    fn a_host_that_never_answers_draws_no_line() {
        let samples: Vec<(f64, f64)> = (1..10).map(|t| (t as f64, f64::NAN)).collect();
        let (buf, drawn) = render(&plot(&samples), 20, 5);
        assert!(is_blank(&buf));
        assert!(!drawn.lost[0].is_empty(), "the losses are still reported");
        assert_eq!(drawn.heads[0], None);
    }

    #[test]
    fn a_lost_ping_breaks_the_line_and_raises_a_curtain() {
        let samples = [
            (0.5, 50.0),
            (1.0, 50.0),
            (2.0, f64::NAN),
            (9.0, 50.0),
            (10.0, 50.0),
        ];
        let mut p = plot(&samples);
        p.curtain = Some(Color::Rgb(90, 0, 0));
        let (buf, drawn) = render(&p, 20, 4);
        // Columns 5..15 (t = 2.5..7.5) must be empty: no line bridges the gap.
        for y in 0..buf.area.height {
            for x in 5..15 {
                assert_eq!(buf[(x, y)].symbol(), " ", "unexpected dot at {x},{y}");
            }
        }
        // The loss at t = 2 sits in plot column 3 (two dot columns per cell, 0.25 s each).
        assert_eq!(drawn.lost[0], vec![3]);
        assert!((0..4).all(|y| buf[(3, y)].bg == Color::Rgb(90, 0, 0)));
        assert!((0..4).all(|y| buf[(4, y)].bg == Color::Reset));
    }

    #[test]
    fn a_flat_series_is_one_row() {
        let samples = [(0.0, 50.0), (10.0, 50.0)];
        let (buf, _) = render(&plot(&samples), 10, 4);
        let drawn: Vec<u16> = (0..4).filter(|&y| row(&buf, y).trim() != "").collect();
        assert_eq!(drawn.len(), 1, "{:?}", drawn);
    }

    // Whole-slot anchoring: shifting the window by less than a slot changes nothing, and by
    // exactly one slot moves every dot by exactly one column.
    #[test]
    fn scrolling_moves_everything_by_whole_dots() {
        let samples: Vec<(f64, f64)> = (0..400)
            .map(|i| (i as f64 * 0.05, ((i * 37) % 90) as f64))
            .collect();
        // A frame as it would be drawn at `now`: with the samples taken so far.
        let frame = |now: f64| {
            let taken = samples.partition_point(|(t, _)| *t <= now);
            let mut p = plot(&samples[..taken]);
            p.x_bounds = [now - 10.0, now];
            render(&p, 20, 4).0
        };
        let cells = |buf: &Buffer, y: u16| -> Vec<String> {
            (0..buf.area.width)
                .map(|x| buf[(x, y)].symbol().to_string())
                .collect()
        };
        let a = frame(15.0);
        // Less than a slot (0.25 s) later only the newest cell, being written, can differ.
        let a2 = frame(15.1);
        for y in 0..4 {
            assert_eq!(cells(&a, y)[..19], cells(&a2, y)[..19], "row {}", y);
        }
        // Two slots later, everything has moved left by exactly one cell.
        let b = frame(15.5);
        for y in 0..4 {
            assert_eq!(cells(&a, y)[1..18], cells(&b, y)[..17], "row {}", y);
        }
    }

    #[test]
    fn glow_sits_under_the_line_only() {
        let samples = [(0.0, 50.0), (10.0, 50.0)];
        let mut p = plot(&samples);
        p.series[0].glow = Some(Glow {
            tint: (0, 0, 200),
            top: 0.5,
            bottom: 0.1,
        });
        let (buf, _) = render(&p, 10, 4);
        assert!(
            (0..10).all(|x| buf[(x, 0)].bg == Color::Reset),
            "nothing above the line"
        );
        let bottom = buf[(5, 3)].bg;
        let below_line = buf[(5, 2)].bg;
        assert_ne!(bottom, Color::Reset);
        // Fading towards the floor.
        if let (Color::Rgb(_, _, b1), Color::Rgb(_, _, b2)) = (below_line, bottom) {
            assert!(b1 > b2, "{} > {}", b1, b2);
        } else {
            panic!("{:?} {:?}", below_line, bottom);
        }
    }

    // The fill follows the line's typical level, so a short spike doesn't grow a tinted
    // tower under it.
    #[test]
    fn spikes_get_no_fill_tower() {
        let mut samples: Vec<(f64, f64)> = (0..=40).map(|i| (i as f64 * 0.25, 20.0)).collect();
        samples[20].1 = 95.0;
        let mut p = plot(&samples);
        p.series[0].glow = Some(Glow {
            tint: (0, 0, 200),
            top: 0.5,
            bottom: 0.1,
        });
        let (buf, _) = render(&p, 20, 8);
        // Under the spike, the rows above the usual level stay untinted.
        assert!((0..5).all(|y| buf[(10, y)].bg == Color::Reset));
        assert_ne!(buf[(10, 7)].bg, Color::Reset);
    }

    #[test]
    fn heads_and_peaks_are_reported() {
        let samples = [(1.0, 10.0), (5.0, 90.0), (9.0, 30.0), (9.5, f64::NAN)];
        let (_, drawn) = render(&plot(&samples), 20, 4);
        // 0.25 s per dot column: t = 9.0 is dot column 35, cell 17.
        let (hx, _) = drawn.heads[0].unwrap();
        assert_eq!(hx, 17, "the newest reply, not the lost ping after it");
        let (px, py, value) = drawn.peaks[0].unwrap();
        assert_eq!((px, py, value), (9, 0, 90.0));
        assert!((drawn.slot - 0.25).abs() < 1e-9);
    }

    #[test]
    fn heat_recolours_by_height() {
        let samples = [(0.0, 10.0), (5.0, 95.0), (10.0, 10.0)];
        let mut p = plot(&samples);
        let heat = |v: f64| (v > 60.0).then_some(Color::Rgb(255, 128, 0));
        p.series[0].heat = Some(&heat);
        let (buf, _) = render(&p, 20, 4);
        assert_eq!(buf[(10, 0)].fg, Color::Rgb(255, 128, 0));
        assert_eq!(buf[(1, 3)].fg, Color::Rgb(255, 0, 0));
    }

    #[test]
    fn tiny_or_degenerate_areas_do_not_panic() {
        let samples = [(0.0, 50.0), (5.0, f64::NAN), (10.0, 5000.0)];
        for (w, h) in [(0, 0), (1, 1), (0, 5), (5, 0), (2, 1)] {
            render(&plot(&samples), w, h);
        }
        let mut p = plot(&samples);
        p.y_bounds = [10.0, 10.0];
        assert!(is_blank(&render(&p, 10, 4).0));
        p.y_bounds = [0.0, 1.0];
        p.x_bounds = [f64::NAN, 1.0];
        render(&p, 10, 4);
    }

    #[test]
    fn off_scale_values_are_clipped() {
        // 5000 is 50x the axis maximum: the line climbs out of the top and stays out.
        let samples = [(0.0, 50.0), (5.0, 5000.0), (10.0, 50.0)];
        let (buf, _) = render(&plot(&samples), 10, 4);
        assert!(
            row(&buf, 0).trim() != "",
            "the line should reach the top edge"
        );
    }

    #[test]
    fn simple_graphics_uses_bullets() {
        let samples = [(0.0, 50.0), (10.0, 50.0)];
        let mut p = plot(&samples);
        p.marker = Marker::Dot;
        let (buf, _) = render(&p, 10, 4);
        let all: String = (0..4).map(|y| row(&buf, y)).collect();
        assert!(all.contains('•'));
        assert!(!all.chars().any(|c| ('\u{2800}'..='\u{28ff}').contains(&c)));
    }

    #[test]
    fn the_first_series_wins_shared_cells() {
        // 50 and 42 land on dot rows 8 and 9: the same row of cells.
        let a = [(0.0, 50.0), (10.0, 50.0)];
        let b = [(0.0, 42.0), (10.0, 42.0)];
        let mut p = plot(&a);
        p.series.push(Series {
            samples: &b,
            color: Color::Rgb(0, 255, 0),
            heat: None,
            glow: None,
            head: None,
        });
        let (buf, _) = render(&p, 10, 4);
        let colors: Vec<Color> = buf
            .content
            .iter()
            .filter(|c| c.symbol() != " ")
            .map(|c| c.fg)
            .collect();
        assert!(
            colors.iter().all(|&c| c == Color::Rgb(255, 0, 0)),
            "{:?}",
            colors
        );
    }
}

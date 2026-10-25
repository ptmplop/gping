//! Where everything goes, as a pure function of the screen size and the number of hosts.

use tui::layout::Rect;

/// How much of a host's card fits.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum CardKind {
    /// Big digits, window stats and a session sparkline.
    Full,
    /// Big digits and the main stats.
    Compact,
    /// Name and current value as text.
    Slim,
}

/// One host's lane: its plot with axis labels on the left, the loss rail under it, a
/// histogram of where its latency sits, and its card on the right.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Lane {
    pub host: usize,
    pub gutter: Rect,
    pub plot: Rect,
    pub rail: Rect,
    pub hist: Option<Rect>,
    pub card: Rect,
    pub kind: CardKind,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Body {
    Lanes(Vec<Lane>),
    /// Too small for lanes: a line of text per host.
    Lines(Vec<(usize, Rect)>),
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Plan {
    pub masthead: Option<Rect>,
    pub body: Body,
    /// Where to say how many hosts didn't fit, and how many.
    pub more: Option<(Rect, usize)>,
    /// The time axis, under the lanes and lined up with their plots.
    pub axis: Option<Rect>,
    pub footer: Option<Rect>,
}

pub const GUTTER: u16 = 6;
const FULL_CARD: u16 = 33;
const COMPACT_CARD: u16 = 24;
const SLIM_CARD: u16 = 17;
const HIST: u16 = 4;
const MIN_PLOT: u16 = 12;

/// Lays out `hosts` (indices of the hosts on show) in `area`.
pub fn plan(area: Rect, hosts: &[usize]) -> Plan {
    let (w, h) = (area.width, area.height);
    let mut top = area.y;
    let mut bottom = area.bottom();

    let masthead = (h >= 6).then(|| {
        top += 1;
        Rect::new(area.x, area.y, w, 1)
    });
    // A blank row under the title bar, on tall screens.
    let spacer = h >= 30;
    let footer = (h >= 8).then(|| {
        bottom -= 1;
        Rect::new(area.x, bottom, w, 1)
    });
    let rows = bottom.saturating_sub(top);

    if let Some((lanes, axis)) = lanes(area.x, top, w, rows, hosts, spacer) {
        return Plan {
            masthead,
            body: Body::Lanes(lanes),
            more: None,
            axis,
            footer,
        };
    }

    // One line per host, and a line to say how many didn't fit.
    if spacer {
        top += 1;
    }
    let rows = bottom.saturating_sub(top);
    let fits = rows as usize;
    let (shown, more) = if fits == 0 {
        (0, None)
    } else if hosts.len() <= fits {
        (hosts.len(), None)
    } else {
        let shown = fits.saturating_sub(1);
        let more = Rect::new(area.x, top + shown as u16, w, 1);
        (shown, Some((more, hosts.len() - shown)))
    };
    let lines = hosts
        .iter()
        .take(shown)
        .enumerate()
        .map(|(i, &host)| (host, Rect::new(area.x, top + i as u16, w, 1)))
        .collect();
    Plan {
        masthead,
        body: Body::Lines(lines),
        more: more.filter(|(rect, _)| rect.height > 0 && w > 0),
        axis: None,
        footer,
    }
}

fn lanes(
    x: u16,
    top: u16,
    w: u16,
    rows: u16,
    hosts: &[usize],
    spacer: bool,
) -> Option<(Vec<Lane>, Option<Rect>)> {
    let n = hosts.len() as u16;
    if n == 0 {
        return Some((Vec::new(), None));
    }
    // A row for the time axis, then the lanes; a blank row between lanes when they're tall.
    let rows = rows.checked_sub(1)?;
    let gap = if rows.saturating_sub(n - 1) / n >= 8 {
        1
    } else {
        0
    };
    // Every lane the same height, so that one latency sits at one height in each on a shared
    // scale, and every card is laid out alike. A row to spare goes to the spacer under the
    // title bar (which never takes one from the lanes), any others under the time axis.
    let height = rows.checked_sub(gap * (n - 1))? / n;
    if height < 2 {
        return None;
    }
    let spare = rows - gap * (n - 1) - height * n;

    let card_kind = |h: u16| match (h, w) {
        (8.., 100..) => Some(CardKind::Full),
        (5.., 70..) => Some(CardKind::Compact),
        (_, 40..) => Some(CardKind::Slim),
        _ => None,
    };
    // Every lane gets the same card and histogram, so their plots line up over the one
    // time axis; the shortest lane decides.
    let kind = card_kind(height)?;
    let card_w = match kind {
        CardKind::Full => FULL_CARD,
        CardKind::Compact => COMPACT_CARD,
        CardKind::Slim => SLIM_CARD,
    };
    let hist = if w >= 100 && height >= 4 { HIST + 1 } else { 0 };
    let mut lanes = Vec::with_capacity(hosts.len());
    let mut y = top + u16::from(spacer && spare > 0);
    let mut plot_span = (0, 0);
    for &host in hosts {
        let h = height;
        let plot_w = w.checked_sub(GUTTER + hist + 1 + card_w)?;
        if plot_w < MIN_PLOT {
            return None;
        }
        let plot_x = x + GUTTER;
        lanes.push(Lane {
            host,
            gutter: Rect::new(x, y, GUTTER, h),
            plot: Rect::new(plot_x, y, plot_w, h - 1),
            rail: Rect::new(plot_x, y + h - 1, plot_w, 1),
            hist: (hist > 0).then(|| Rect::new(plot_x + plot_w + 1, y, HIST, h - 1)),
            card: Rect::new(x + w - card_w, y, card_w, h),
            kind,
        });
        plot_span = (plot_x, plot_w);
        y += h + gap;
    }
    let axis_y = y - gap;
    let axis = Some(Rect::new(plot_span.0, axis_y, plot_span.1, 1));
    Some((lanes, axis))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn hosts(n: usize) -> Vec<usize> {
        (0..n).collect()
    }

    fn inside(outer: Rect, inner: Rect) -> bool {
        inner.width == 0
            || inner.height == 0
            || (inner.x >= outer.x
                && inner.y >= outer.y
                && inner.right() <= outer.right()
                && inner.bottom() <= outer.bottom())
    }

    fn rects(plan: &Plan) -> Vec<Rect> {
        let mut all: Vec<Rect> = [plan.masthead, plan.axis, plan.footer]
            .iter()
            .flatten()
            .copied()
            .collect();
        if let Some((rect, _)) = plan.more {
            all.push(rect);
        }
        match &plan.body {
            Body::Lanes(lanes) => {
                for lane in lanes {
                    all.extend([lane.gutter, lane.plot, lane.rail, lane.card]);
                    all.extend(lane.hist);
                }
            }
            Body::Lines(lines) => all.extend(lines.iter().map(|(_, r)| *r)),
        }
        all
    }

    // Every size and host count gives rectangles inside the screen that don't overlap.
    #[test]
    fn every_size_is_laid_out_inside_the_area() {
        for w in (0..=200).step_by(3) {
            for h in 0..=60 {
                for n in [1, 2, 3, 5, 8, 13, 30] {
                    let area = Rect::new(2, 1, w, h);
                    let plan = plan(area, &hosts(n));
                    let all = rects(&plan);
                    for (i, a) in all.iter().enumerate() {
                        assert!(inside(area, *a), "{}x{} n={}: {:?} outside", w, h, n, a);
                        for b in &all[i + 1..] {
                            let overlap = a.width > 0
                                && a.height > 0
                                && b.width > 0
                                && b.height > 0
                                && a.intersects(*b);
                            assert!(!overlap, "{}x{} n={}: {:?} overlaps {:?}", w, h, n, a, b);
                        }
                    }
                    // Every host is either on screen or counted in "+N more".
                    let shown = match &plan.body {
                        Body::Lanes(lanes) => lanes.len(),
                        Body::Lines(lines) => lines.len(),
                    };
                    let more = plan.more.map_or(0, |(_, m)| m);
                    if h >= 1 && w >= 1 {
                        assert!(
                            shown + more == n || (shown == 0 && more == 0 && h < 3),
                            "{}x{} n={}",
                            w,
                            h,
                            n
                        );
                    }
                }
            }
        }
    }

    #[test]
    fn three_hosts_at_120x34() {
        let plan = plan(Rect::new(0, 1, 120, 32), &hosts(3));
        let Body::Lanes(lanes) = &plan.body else {
            panic!("{:?}", plan.body)
        };
        assert_eq!(lanes.len(), 3);
        assert!(lanes.iter().all(|l| l.kind == CardKind::Full));
        assert!(lanes.iter().all(|l| l.hist.is_some()));
        assert!(lanes.iter().all(|l| l.plot.height == 8), "{:?}", lanes);
        assert!(plan.masthead.is_some() && plan.footer.is_some() && plan.axis.is_some());
    }

    // Every lane gets the same height and card, however the rows divide, so the plots line
    // up over the shared time axis and a shared scale means the same in each.
    #[test]
    fn lanes_line_up() {
        for rows in 20..60 {
            let plan = plan(Rect::new(0, 0, 120, rows), &hosts(3));
            let Body::Lanes(lanes) = &plan.body else {
                continue;
            };
            assert!(lanes.windows(2).all(|w| w[0].kind == w[1].kind
                && w[0].plot.x == w[1].plot.x
                && w[0].plot.width == w[1].plot.width
                && w[0].plot.height == w[1].plot.height));
            assert_eq!(plan.axis.unwrap().width, lanes[0].plot.width);
        }
    }

    #[test]
    fn eighty_columns_use_compact_cards() {
        let plan = plan(Rect::new(0, 1, 80, 22), &hosts(2));
        let Body::Lanes(lanes) = &plan.body else {
            panic!()
        };
        assert!(lanes
            .iter()
            .all(|l| l.kind == CardKind::Compact && l.hist.is_none()));
    }

    #[test]
    fn many_hosts_in_a_small_window_become_lines() {
        let plan = plan(Rect::new(0, 0, 60, 10), &hosts(10));
        let Body::Lines(lines) = &plan.body else {
            panic!("{:?}", plan.body)
        };
        assert!(!lines.is_empty());
        let (_, more) = plan.more.unwrap();
        assert_eq!(lines.len() + more, 10);
    }
}

//! Number formatting shared by the stats table and the chart axes.
//!
//! Latencies are carried around in microseconds (as `f64`, with `NaN` for a lost ping) and
//! shown in milliseconds with three significant digits, so that a column of them keeps a
//! constant width no matter how the values move: `0.42 ms`, `3.21 ms`, `42.1 ms`, `421 ms`,
//! `1.42 s`.

/// Formats a latency given in microseconds.
pub fn latency(us: f64) -> String {
    if !us.is_finite() || us < 0.0 {
        return "-".to_string();
    }
    let ms = us / 1_000.0;
    // The thresholds sit just under the next power of ten so that rounding can never push a
    // value into a wider form ("9.996" would otherwise print as "10.00 ms").
    if ms < 9.995 {
        format!("{ms:.2} ms")
    } else if ms < 99.95 {
        format!("{ms:.1} ms")
    } else if ms < 999.5 {
        format!("{ms:.0} ms")
    } else {
        let s = ms / 1_000.0;
        if s < 9.995 {
            format!("{s:.2} s")
        } else if s < 99.95 {
            format!("{s:.1} s")
        } else {
            format!("{s:.0} s")
        }
    }
}

/// [`latency`] without the unit when it's milliseconds, for tight columns that say "ms"
/// elsewhere: `7.65`, `105`, but `1.42s`.
pub fn short(us: f64) -> String {
    let text = latency(us);
    match text.strip_suffix(" ms") {
        Some(ms) => ms.to_string(),
        None => text.replace(' ', ""),
    }
}

/// [`latency`] split into the number and its unit, for the big readouts: `("7.65", "ms")`.
pub fn latency_parts(us: f64) -> Option<(String, &'static str)> {
    let text = latency(us);
    let (number, unit) = text.split_once(' ')?;
    let unit = if unit == "s" { "s" } else { "ms" };
    Some((number.to_string(), unit))
}

/// A relative time label for the time axis: `now`, `-5s`, `-1m`, `-1m30s`, `-2h`.
pub fn ago(secs: f64) -> String {
    let secs = secs.round() as i64;
    if secs <= 0 {
        return "now".to_string();
    }
    let (h, m, s) = (secs / 3_600, (secs / 60) % 60, secs % 60);
    match (h, m, s) {
        (0, 0, s) => format!("−{s}s"),
        (0, m, 0) => format!("−{m}m"),
        (0, m, s) => format!("−{m}m{s:02}s"),
        (h, 0, _) => format!("−{h}h"),
        (h, m, _) => format!("−{h}h{m:02}m"),
    }
}

/// The spacing of the time axis labels, in seconds: the smallest of the usual steps that
/// keeps labels at least `min_cols` columns apart on a plot `cols` wide showing `window`
/// seconds.
pub fn time_step(window: f64, cols: u16, min_cols: u16) -> f64 {
    const STEPS: [f64; 16] = [
        1.0, 2.0, 5.0, 10.0, 15.0, 30.0, 60.0, 120.0, 300.0, 600.0, 900.0, 1_800.0, 3_600.0,
        7_200.0, 21_600.0, 43_200.0,
    ];
    let per_col = window / cols.max(1) as f64;
    STEPS
        .iter()
        .copied()
        .find(|step| step / per_col >= min_cols as f64)
        .unwrap_or(86_400.0)
}

/// Picks a "nice" step (1, 2 or 5 times a power of ten) so that `range` is split into
/// roughly `target` intervals.
pub fn nice_step(range: f64, target: usize) -> f64 {
    let target = target.max(1) as f64;
    if !range.is_finite() || range <= 0.0 {
        return 1.0;
    }
    let raw = range / target;
    let magnitude = 10f64.powf(raw.log10().floor());
    let normalised = raw / magnitude;
    let nice = if normalised <= 1.0 {
        1.0
    } else if normalised <= 2.0 {
        2.0
    } else if normalised <= 5.0 {
        5.0
    } else {
        10.0
    };
    nice * magnitude
}

/// Formats a value-axis tick (microseconds). Every tick on an axis is formatted with the
/// same unit and precision, chosen from the axis maximum and the step between ticks.
pub fn axis_tick(us: f64, step_us: f64, max_us: f64) -> String {
    let (value, step, unit) = if max_us >= 1_000_000.0 {
        (us / 1_000_000.0, step_us / 1_000_000.0, "s")
    } else {
        (us / 1_000.0, step_us / 1_000.0, "ms")
    };
    let decimals = if step >= 1.0 {
        0
    } else {
        // 0.5 -> 1, 0.2 -> 1, 0.05 -> 2, ...
        (-step.log10().floor()) as usize
    };
    format!("{value:.decimals$} {unit}")
}

/// A tick label without its unit, which the axis states once: `20`, `2.5`, `1.5s`.
pub fn tick(us: f64, step_us: f64, max_us: f64) -> String {
    let label = axis_tick(us, step_us, max_us);
    match label.strip_suffix(" ms") {
        Some(ms) => ms.to_string(),
        None => label.replace(' ', ""),
    }
}

/// Formats an elapsed number of seconds as `42s`, `3m 07s` or `2h 05m`.
pub fn elapsed(secs: u64) -> String {
    let (h, m, s) = (secs / 3_600, (secs / 60) % 60, secs % 60);
    if h > 0 {
        format!("{h}h {m:02}m")
    } else if m > 0 {
        format!("{m}m {s:02}s")
    } else {
        format!("{s}s")
    }
}

/// A length of time as `1.2 s`, `14 s` or `2m 05s`.
pub fn duration(secs: f64) -> String {
    if secs < 9.95 {
        format!("{secs:.1} s")
    } else if secs < 59.5 {
        format!("{secs:.0} s")
    } else {
        elapsed(secs.round() as u64)
    }
}

/// Formats a ratio in `0..=1` as a percentage with at most one decimal (`0%`, `2.5%`, `100%`).
pub fn percent(ratio: f64) -> String {
    let pct = (ratio * 100.0).clamp(0.0, 100.0);
    if pct == 0.0 || pct >= 99.95 || (pct - pct.round()).abs() < 0.05 {
        format!("{pct:.0}%")
    } else {
        format!("{pct:.1}%")
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// The widest string [`latency`] produces, e.g. `42.1 ms`.
    const LATENCY_WIDTH: usize = 7;

    #[test]
    fn latency_uses_three_significant_digits() {
        assert_eq!(latency(420.0), "0.42 ms");
        assert_eq!(latency(3_214.0), "3.21 ms");
        assert_eq!(latency(42_149.0), "42.1 ms");
        assert_eq!(latency(421_000.0), "421 ms");
        assert_eq!(latency(1_420_000.0), "1.42 s");
        assert_eq!(latency(14_200_000.0), "14.2 s");
    }

    #[test]
    fn latency_never_rounds_into_a_wider_form() {
        for us in [9_996.0, 99_960.0, 999_600.0, 9_996_000.0] {
            assert!(
                latency(us).len() <= LATENCY_WIDTH,
                "{us} -> {}",
                latency(us)
            );
        }
        assert_eq!(latency(9_996.0), "10.0 ms");
        assert_eq!(latency(999_600.0), "1.00 s");
    }

    #[test]
    fn latency_of_a_lost_ping_is_a_dash() {
        assert_eq!(latency(f64::NAN), "-");
    }

    #[test]
    fn latency_width_is_bounded() {
        let mut us = 1.0;
        while us < 1e9 {
            assert!(
                latency(us).len() <= LATENCY_WIDTH,
                "{us} -> {}",
                latency(us)
            );
            us *= 1.07;
        }
    }

    #[test]
    fn nice_steps() {
        assert_eq!(nice_step(100.0, 5), 20.0);
        assert_eq!(nice_step(100.0, 4), 50.0);
        assert_eq!(nice_step(7.0, 4), 2.0);
        assert_eq!(nice_step(0.9, 3), 0.5);
        assert_eq!(nice_step(0.0, 3), 1.0);
        assert_eq!(nice_step(f64::NAN, 3), 1.0);
    }

    #[test]
    fn axis_ticks_share_unit_and_precision() {
        assert_eq!(axis_tick(20_000.0, 20_000.0, 100_000.0), "20 ms");
        assert_eq!(axis_tick(500.0, 500.0, 2_000.0), "0.5 ms");
        assert_eq!(axis_tick(1_500_000.0, 500_000.0, 2_000_000.0), "1.5 s");
        assert_eq!(axis_tick(0.0, 50.0, 200.0), "0.00 ms");
    }

    #[test]
    fn durations() {
        assert_eq!(duration(1.2), "1.2 s");
        assert_eq!(duration(14.0), "14 s");
        assert_eq!(duration(125.0), "2m 05s");
    }

    #[test]
    fn tick_labels() {
        assert_eq!(tick(5_000.0, 1_000.0, 5_000.0), "5");
        assert_eq!(tick(2_500.0, 500.0, 5_000.0), "2.5");
        assert_eq!(tick(1_500_000.0, 500_000.0, 2_000_000.0), "1.5s");
    }

    #[test]
    fn elapsed_times() {
        assert_eq!(elapsed(7), "7s");
        assert_eq!(elapsed(187), "3m 07s");
        assert_eq!(elapsed(7_500), "2h 05m");
    }

    #[test]
    fn short_forms() {
        assert_eq!(short(7_654.0), "7.65");
        assert_eq!(short(105_000.0), "105");
        assert_eq!(short(1_420_000.0), "1.42s");
        assert_eq!(short(f64::NAN), "-");
    }

    #[test]
    fn readout_parts() {
        assert_eq!(latency_parts(7_654.0), Some(("7.65".to_string(), "ms")));
        assert_eq!(latency_parts(1_420_000.0), Some(("1.42".to_string(), "s")));
        assert_eq!(latency_parts(f64::NAN), None);
    }

    #[test]
    fn relative_times() {
        assert_eq!(ago(0.0), "now");
        assert_eq!(ago(30.0), "−30s");
        assert_eq!(ago(60.0), "−1m");
        assert_eq!(ago(90.0), "−1m30s");
        assert_eq!(ago(7_200.0), "−2h");
        assert_eq!(ago(5_400.0), "−1h30m");
    }

    #[test]
    fn time_steps_keep_labels_apart() {
        // 30 s over 100 columns: 5 s is 16.7 columns, the first step at least 10 apart.
        assert_eq!(time_step(30.0, 100, 10), 5.0);
        assert_eq!(time_step(30.0, 50, 10), 10.0);
        assert_eq!(time_step(600.0, 100, 10), 60.0);
        assert_eq!(time_step(30.0, 0, 10), 300.0);
    }

    #[test]
    fn percentages() {
        assert_eq!(percent(0.0), "0%");
        assert_eq!(percent(0.025), "2.5%");
        assert_eq!(percent(1.0 / 3.0), "33.3%");
        assert_eq!(percent(1.0), "100%");
        assert_eq!(percent(0.5), "50%");
    }
}

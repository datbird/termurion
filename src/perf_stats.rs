//! iTerm2's GPU renderer statistics Termurion shows: the FPS meter's moving average and text
//! (`iTermMetalDriver.m:1800-1807`). Pure, so the rules are tested here.

/// The meter's text color, magenta (`iTermMetalPerFrameState.m:2531-2552`).
pub const FPS_FG: [f32; 4] = [1.0, 0.0, 1.0, 1.0];
/// The meter's background, near black.
pub const FPS_BG: [f32; 4] = [0.1, 0.1, 0.1, 1.0];

/// The weight of the old value (`iTermMetalDriver`'s average of frame periods).
pub const FPS_ALPHA: f64 = 0.75;

/// An exponential moving average (`iTermExponentialMovingAverage`): the first sample is the
/// value; after that `alpha` of the old value and the rest of the new sample.
#[derive(Clone, Copy, Debug)]
pub struct MovingAverage {
    value: f64,
    alpha: f64,
    initialized: bool,
}

impl MovingAverage {
    pub fn new(alpha: f64) -> MovingAverage {
        MovingAverage { value: 0.0, alpha, initialized: false }
    }

    pub fn add(&mut self, v: f64) {
        self.value = if self.initialized { self.alpha * self.value + (1.0 - self.alpha) * v } else { v };
        self.initialized = true;
    }

    pub fn value(&self) -> f64 {
        self.value
    }
}

/// `iTermMetalDriver.m:1800-1807`: the frame number and the rate the average period gives.
/// A period under a millisecond reads 0.
pub fn fps_text(frame: u64, period: f64) -> String {
    let fps = if period < 0.001 { 0 } else { (1.0 / period).round() as u64 };
    format!(" [Frame {frame}: {fps} fps] ")
}

/// `GPU Renderer Availability` (`iTermApplicationDelegate.m:3720-3733`).
pub const GPU_TITLE: &str = "GPU Renderer Availability";
pub const GPU_ENABLED: &str = "GPU rendering is enabled for the current session.";

/// Said for a window that has no GPU device (it never claims "enabled" then).
pub const GPU_NONE: &str = "GPU rendering is not available for this window.";

/// The alert's text: enabled with the adapter's name and backend, or `GPU_NONE`.
pub fn gpu_text(adapter: Option<(&str, &str)>) -> String {
    match adapter {
        Some((name, backend)) => format!("{GPU_ENABLED}\n\n{}", gpu_adapter_line(name, backend)),
        None => GPU_NONE.to_string(),
    }
}

/// Termurion's second paragraph: the adapter the window draws on.
pub fn gpu_adapter_line(name: &str, backend: &str) -> String {
    format!("Adapter: {name} ({backend})")
}

/// A number as the histogram summary shows it: up to two decimals, no trailing zeros.
fn num(v: f64) -> String {
    let s = format!("{v:.2}");
    s.trim_end_matches('0').trim_end_matches('.').to_string()
}

/// How many values the histogram keeps for its percentiles (`iTermHistogram.mm:258`).
const RESERVOIR: usize = 100;

/// iTerm2's histogram summary without the sparkline glyphs (`iTermHistogram.mm:511-528`).
/// Like iTerm2's sampler it keeps a reservoir of `RESERVOIR` values (reservoir sampling,
/// `:60-76`) plus the exact count, sum, min and max, so memory and the cost of a summary stay
/// bounded however many frames are timed. Percentiles interpolate linearly between ranks.
#[derive(Clone, Debug)]
pub struct Histogram {
    samples: Vec<f64>,
    count: u64,
    sum: f64,
    min: f64,
    max: f64,
    /// xorshift state for the reservoir's random slot (no clock or crate needed).
    rng: u64,
}

impl Default for Histogram {
    fn default() -> Self {
        Histogram { samples: Vec::new(), count: 0, sum: 0.0, min: 0.0, max: 0.0, rng: 0x9E37_79B9_7F4A_7C15 }
    }
}

impl Histogram {
    pub fn add(&mut self, v: f64) {
        if self.count == 0 {
            (self.min, self.max) = (v, v);
        } else {
            self.min = self.min.min(v);
            self.max = self.max.max(v);
        }
        self.sum += v;
        if self.samples.len() < RESERVOIR {
            self.samples.push(v);
        } else {
            self.rng ^= self.rng << 13;
            self.rng ^= self.rng >> 7;
            self.rng ^= self.rng << 17;
            // A slot in 0..=count: only the first RESERVOIR slots keep the value.
            let r = (self.rng % (self.count + 1)) as usize;
            if r < RESERVOIR {
                self.samples[r] = v;
            }
        }
        self.count += 1;
    }

    /// The values kept (a reservoir of at most 100), for a chart.
    pub fn samples(&self) -> &[f64] {
        &self.samples
    }

    /// The mean of every value added.
    pub fn mean(&self) -> f64 {
        if self.count == 0 { 0.0 } else { self.sum / self.count as f64 }
    }

    pub fn count(&self) -> u64 {
        self.count
    }

    pub fn sum(&self) -> f64 {
        self.sum
    }

    /// `value_for_percentile` (`iTermHistogram.mm:150-165`): the kept values sorted, the one at
    /// floor(n * p), at most the last. 0 when nothing is kept.
    pub fn percentile(&self, p: f64) -> f64 {
        if self.samples.is_empty() {
            return 0.0;
        }
        let mut s = self.samples.clone();
        s.sort_by(|a, b| a.total_cmp(b));
        let i = ((s.len() as f64 * p).floor() as usize).min(s.len() - 1);
        s[i]
    }

    /// `graphString` (`iTermHistogram.mm:572-583`): one block glyph a bucket, with the buckets
    /// of the Freedman-Diaconis rule (at most 15), tall as the bucket's share of the largest.
    pub fn graph_string(&self) -> String {
        let n = self.samples.len();
        if n == 0 {
            return String::new();
        }
        let mut buckets: Vec<usize> = Vec::new();
        let mut width = 2.0 * (self.percentile(0.75) - self.percentile(0.25)) / (n as f64).cbrt();
        if width <= 0.0 {
            buckets.push(n);
        } else {
            let min = self.percentile(0.0);
            let range = self.percentile(1.0) - min;
            if range / width > 15.0 {
                width = range / 15.0;
            }
            for v in &self.samples {
                let b = ((v - min) / width) as usize;
                if buckets.len() <= b {
                    buckets.resize(b + 1, 0);
                }
                buckets[b] += 1;
            }
        }
        let largest = buckets.iter().copied().max().unwrap_or(1) as f64;
        const GLYPHS: [char; 8] = ['\u{2581}', '\u{2582}', '\u{2583}', '\u{2584}', '\u{2585}', '\u{2586}', '\u{2587}', '\u{2588}'];
        buckets
            .iter()
            .map(|c| {
                let f = (*c as f64 / largest).min(1.0);
                if f <= 0.0 { ' ' } else { GLYPHS[(f * 7.0).round() as usize] }
            })
            .collect()
    }

    #[cfg(test)]
    fn stored(&self) -> usize {
        self.samples.len()
    }

    pub fn summary(&self) -> String {
        if self.count == 0 {
            return "No data".to_string();
        }
        let mut s = self.samples.clone();
        s.sort_by(|a, b| a.total_cmp(b));
        let n = s.len();
        let pct = |p: f64| {
            let r = (n - 1) as f64 * p;
            let (lo, hi) = (r.floor() as usize, r.ceil() as usize);
            s[lo] + (s[hi] - s[lo]) * (r - lo as f64)
        };
        format!(
            "{} {}  Count={} Mean={} p50={} p95={} Sum={}",
            num(self.min),
            num(self.max),
            self.count,
            num(self.sum / self.count as f64),
            num(pct(0.5)),
            num(pct(0.95)),
            num(self.sum)
        )
    }
}

/// `TabularFormatter.swift`: columns of padded text, a header line first, joined with two spaces.
pub struct Tabular {
    columns: Vec<(String, bool)>,
    rows: Vec<Vec<String>>,
}

impl Tabular {
    /// `iTermHistogram.tabularFormatterTime` (`iTermHistogram.swift:10-30`).
    pub fn time() -> Tabular {
        let c = |l: &str, left: bool| (l.to_string(), left);
        Tabular { columns: vec![c("Min Time", false), c("Distribution", true), c("Max Time", false), c("# Samples", false), c("Mean Time", false), c("P50", false), c("P95", false), c("Total Time", false), c("", true)], rows: Vec::new() }
    }

    /// `add(to:precision:units:label:)` (`iTermHistogram.swift:32-52`): nothing for an empty histogram.
    pub fn add_histogram(&mut self, h: &Histogram, precision: usize, units: &str, label: &str) {
        if h.count() == 0 {
            return;
        }
        let f = |v: f64| format!("{v:.precision$} {units}");
        self.rows.push(vec![f(h.percentile(0.0)), h.graph_string(), f(h.percentile(1.0)), h.count().to_string(), f(h.sum() / h.count() as f64), f(h.percentile(0.5)), f(h.percentile(0.95)), f(h.sum()), label.to_string()]);
    }

    pub fn text(&self) -> String {
        let width = |i: usize| self.rows.iter().map(|r| r[i].chars().count()).chain(Some(self.columns[i].0.chars().count())).max().unwrap_or(0);
        let widths: Vec<usize> = (0..self.columns.len()).map(width).collect();
        let pad = |cells: Vec<&str>| {
            cells
                .iter()
                .enumerate()
                .map(|(i, s)| {
                    let gap = widths[i].saturating_sub(s.chars().count());
                    if self.columns[i].1 { format!("{s}{}", " ".repeat(gap)) } else { format!("{}{s}", " ".repeat(gap)) }
                })
                .collect::<Vec<_>>()
                .join("  ")
        };
        let mut lines = vec![pad(self.columns.iter().map(|c| c.0.as_str()).collect())];
        lines.extend(self.rows.iter().map(|r| pad(r.iter().map(String::as_str).collect())));
        lines.join("\n")
    }
}

/// `iTermMetalDriver.m:401-409`: the share of frames not drawn, "%0.1f%%\n".
pub fn dropped_text(dropped: u64, total: u64) -> String {
    let pct = if total == 0 { 0.0 } else { dropped as f64 * 100.0 / total as f64 };
    format!("{pct:.1}%\n")
}

/// iTerm2's saved precise-timer logs (`iTermPreciseTimer.m:327-346`), keyed by identifier.
/// Holds counters and timings only.
#[derive(Clone, Debug, Default)]
pub struct Logs {
    saved: std::collections::BTreeMap<String, String>,
}

impl Logs {
    pub fn save(&mut self, id: &str, log: &str) {
        self.saved.insert(id.to_string(), log.to_string());
    }

    /// A closed window's logs go (keys "Window {n}: ...").
    pub fn forget_window(&mut self, n: usize) {
        let prefix = format!("Window {n}: ");
        self.saved.retain(|k, _| !k.starts_with(&prefix));
    }

    /// What Copy Performance Stats puts on the clipboard.
    pub fn text(&self) -> String {
        let mut out = String::new();
        for (key, log) in &self.saved {
            let multi = log.split('\n').count() > 1;
            out.push_str(&format!("Precise timers {key}:{}{log}\n", if multi { "\n" } else { "" }));
        }
        out
    }
}

/// One window's frame counters and timings (`iTermMetalDriver.m:401-409`).
#[derive(Clone, Debug, Default)]
pub struct FrameStats {
    total: u64,
    dropped: u64,
    start_to_start: Histogram,
    drawable: Histogram,
}

impl FrameStats {
    /// Counts one frame of window number `window`: `start_gap` is the milliseconds since the
    /// window's last frame started, `drawable` the milliseconds the surface texture took, and
    /// `dropped` is a frame that got no texture (a timeout or an occluded window). Every tenth
    /// frame (`total % 10 == 1`) saves the logs.
    pub fn frame(&mut self, start_gap: Option<f64>, drawable: Option<f64>, dropped: bool, logs: &mut Logs, window: usize) {
        self.total += 1;
        if dropped {
            self.dropped += 1;
        }
        if let Some(g) = start_gap {
            self.start_to_start.add(g);
        }
        if let Some(d) = drawable.filter(|_| !dropped) {
            self.drawable.add(d);
        }
        if self.total % 10 == 1 {
            let key = |what: &str| format!("Window {window}: {what}");
            logs.save(&key("Dropped frames"), &dropped_text(self.dropped, self.total));
            logs.save(&key("Start-to-Start Time (ms)"), &self.start_to_start.summary());
            logs.save(&key("Current Drawable Time (ms)"), &self.drawable.summary());
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_time_table_follows_iterm2() {
        let mut h = Histogram::default();
        for v in [1.0, 2.0, 2.0, 3.0, 10.0] {
            h.add(v);
        }
        assert_eq!((h.percentile(0.0), h.percentile(0.5), h.percentile(1.0)), (1.0, 2.0, 10.0));
        let mut t = Tabular::time();
        t.add_histogram(&Histogram::default(), 2, "\u{b5}s", "empty");
        t.add_histogram(&h, 2, "\u{b5}s", "Trigger \u{201c}a\u{201d} with regex \u{201c}b\u{201d}");
        let text = t.text();
        let lines: Vec<&str> = text.lines().collect();
        assert_eq!(lines.len(), 2, "an empty histogram adds no row");
        let head = lines[0].split_whitespace().collect::<Vec<_>>().join(" ");
        assert_eq!(head, "Min Time Distribution Max Time # Samples Mean Time P50 P95 Total Time");
        assert!(lines[1].contains("1.00 \u{b5}s") && lines[1].contains("10.00 \u{b5}s") && lines[1].contains("18.00 \u{b5}s"), "{}", lines[1]);
        assert!(lines[1].contains('\u{2588}'), "the graph has a full block");
    }

    #[test]
    fn fps_meter_follows_iterm2() {
        let mut a = MovingAverage::new(0.75);
        a.add(0.02);
        assert!((a.value() - 0.02).abs() < 1e-12, "the first sample is the value");
        a.add(0.04);
        assert!((a.value() - 0.025).abs() < 1e-12);
        assert_eq!(fps_text(7, 0.016), " [Frame 7: 63 fps] ");
        assert_eq!(fps_text(1, 0.0005), " [Frame 1: 0 fps] ");
        assert_eq!(fps_text(2, 0.0), " [Frame 2: 0 fps] ");
        assert_eq!(FPS_FG, [1.0, 0.0, 1.0, 1.0]);
        assert_eq!(FPS_BG, [0.1, 0.1, 0.1, 1.0]);
    }

    /// `iTermPreciseTimer.m:336-346`: "Precise timers <key>:", a newline when the log has more
    /// than one line, the log and a newline, per saved log. The brief's literal omits the newline
    /// after the first colon although its own rule (and the source) put it there.
    #[test]
    fn perf_logs_follow_iterm2() {
        let mut logs = Logs::default();
        assert_eq!(logs.text(), "");
        logs.save("Window 1: Start-to-Start Time (ms)", "No data");
        logs.save("Window 1: Dropped frames", "0.0%\n");
        assert_eq!(
            logs.text(),
            "Precise timers Window 1: Dropped frames:\n0.0%\n\nPrecise timers Window 1: Start-to-Start Time (ms):No data\n"
        );
        logs.save("Window 1: Dropped frames", "2.5%\n");
        assert!(logs.text().contains("frames:\n2.5%\n\n"), "a save replaces the log under its key");
        assert_eq!(dropped_text(1, 40), "2.5%\n");
        assert_eq!(dropped_text(0, 0), "0.0%\n");
        let mut h = Histogram::default();
        assert_eq!(h.summary(), "No data");
        for v in [1.0, 2.0, 3.0, 4.0] {
            h.add(v);
        }
        assert_eq!(h.summary(), "1 4  Count=4 Mean=2.5 p50=2.5 p95=3.85 Sum=10");
    }

    /// A closed window's saved logs and counters go, so a new window that reuses its number
    /// starts clean; other windows' logs stay.
    #[test]
    fn a_closed_windows_logs_are_forgotten() {
        let mut logs = Logs::default();
        logs.save("Window 1: Dropped frames", "0.0%\n");
        logs.save("Window 12: Dropped frames", "1.0%\n");
        logs.save("Window 2: Dropped frames", "2.0%\n");
        logs.forget_window(1);
        let t = logs.text();
        assert!(!t.contains("Window 1:") && t.contains("Window 12:") && t.contains("Window 2:"), "{t}");
    }

    /// The availability text says "enabled" only for a window that has a GPU.
    #[test]
    fn gpu_text_needs_a_gpu() {
        assert_eq!(gpu_text(Some(("Radeon", "Dx12"))), format!("{GPU_ENABLED}\n\nAdapter: Radeon (Dx12)"));
        assert_eq!(gpu_text(None), GPU_NONE);
        assert_ne!(GPU_NONE, GPU_ENABLED);
    }

    /// iTerm2's sampler keeps a reservoir (100 values) and the exact count, sum, min and max
    /// (`iTermHistogram.mm:20-76`, `:360-369`): memory stays bounded however long the app runs.
    #[test]
    fn histogram_memory_is_bounded() {
        let mut h = Histogram::default();
        for i in 1..=100_000u32 {
            h.add(i as f64);
        }
        assert!(h.stored() <= 100, "{} samples kept", h.stored());
        let s = h.summary();
        assert!(s.starts_with("1 100000  Count=100000 Mean=50000.5 "), "{s}");
        assert!(s.ends_with("Sum=5000050000"), "{s}");
    }

    /// Every tenth frame (`total % 10 == 1`) saves the three logs under the window's name;
    /// dropped frames count in the total. Only counters and timings are kept.
    #[test]
    fn frame_stats_save_every_tenth_frame() {
        let mut stats = FrameStats::default();
        let mut logs = Logs::default();
        stats.frame(Some(16.0), Some(0.5), false, &mut logs, 1);
        let t = logs.text();
        assert!(t.contains("Precise timers Window 1: Dropped frames:\n0.0%\n\n"), "{t}");
        assert!(t.contains("Window 1: Start-to-Start Time (ms):16 16  Count=1 Mean=16 p50=16 p95=16 Sum=16\n"), "{t}");
        assert!(t.contains("Window 1: Current Drawable Time (ms):0.5 0.5  Count=1 Mean=0.5 p50=0.5 p95=0.5 Sum=0.5\n"), "{t}");
        for _ in 0..9 {
            stats.frame(Some(20.0), Some(1.0), false, &mut logs, 1);
        }
        assert!(logs.text().contains("Count=1 "), "frames 2 to 10 do not save");
        stats.frame(None, None, true, &mut logs, 1);
        let t = logs.text();
        assert!(t.contains("Count=10 "), "frame 11 saves the ten timed frames: {t}");
        assert!(t.contains("Dropped frames:\n9.1%\n"), "1 of 11 frames dropped: {t}");
        FrameStats::default().frame(None, None, false, &mut logs, 2);
        assert!(logs.text().contains("Window 2: Dropped frames"), "{}", logs.text());
    }

    /// `AD:3720-3733`.
    #[test]
    fn gpu_availability_follows_iterm2() {
        assert_eq!(GPU_TITLE, "GPU Renderer Availability");
        assert_eq!(GPU_ENABLED, "GPU rendering is enabled for the current session.");
        assert_eq!(gpu_adapter_line("Intel(R) UHD Graphics", "Dx12"), "Adapter: Intel(R) UHD Graphics (Dx12)");
    }
}

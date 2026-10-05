//! iTerm2's GPU renderer statistics Tandem shows: the FPS meter's moving average and text
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

#[cfg(test)]
mod tests {
    use super::*;

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
}

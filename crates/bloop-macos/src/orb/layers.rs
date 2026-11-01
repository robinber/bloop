//! Core Animation layers of the orb: the pill, the application icon, and the bars.

#![expect(
    unsafe_code,
    reason = "the layer contents, the gradient colors, and the mask take Objective-C objects that the compiler cannot check"
)]

use std::time::Duration;

use objc2::rc::Retained;
use objc2::runtime::AnyObject;
use objc2_app_kit::NSImage;
use objc2_core_foundation::{CFType, CGFloat, CGPoint, CGRect, CGSize};
use objc2_core_graphics::CGColor;
use objc2_foundation::NSArray;
use objc2_quartz_core::{CAGradientLayer, CALayer, CATransaction, CATransform3D};

pub(super) const PILL: CGSize = CGSize::new(150.0, 56.0);
pub(super) const BAR_COUNT: usize = 7;
pub(super) const BAR_MIN: CGFloat = 6.0;
pub(super) const BAR_MOVE: Duration = Duration::from_millis(80);
pub(super) const SHRINK: Duration = Duration::from_millis(120);

const OPEN: Duration = Duration::from_millis(150);
const PADDING: CGFloat = 18.0;
const ICON_SIZE: CGFloat = 20.0;
const BAR_WIDTH: CGFloat = 4.0;
const BAR_PITCH: CGFloat = 11.5;
const BAR_MAX: CGFloat = 24.0;
const CLOSED_SCALE: CGFloat = 0.8;

const WEIGHTS: [CGFloat; BAR_COUNT] = [0.5, 0.7, 0.9, 1.0, 0.85, 0.65, 0.45];
const VARIATION: CGFloat = 0.15;
const RISE: CGFloat = 0.6;
const FALL: CGFloat = 0.12;
const FLOOR_DB: CGFloat = -50.0;
const CEILING_DB: CGFloat = -10.0;

/// The colors of the bars.
#[derive(Debug, Clone, Copy)]
pub(super) enum Palette {
    /// Orange at the bottom, yellow at the top.
    Voice,
    /// Red.
    Error,
}

/// The layer tree of the orb. The pill starts closed.
#[derive(Debug)]
pub(super) struct Layers {
    root: Retained<CALayer>,
    pill: Retained<CALayer>,
    icon: Retained<CALayer>,
    gradient: Retained<CAGradientLayer>,
    bars: [Retained<CALayer>; BAR_COUNT],
    voice: Retained<NSArray>,
    error: Retained<NSArray>,
}

impl Layers {
    pub(super) fn new() -> Self {
        let bounds = CGRect::new(CGPoint::new(0.0, 0.0), PILL);
        let root = CALayer::layer();
        root.setFrame(bounds);

        let pill = CALayer::layer();
        pill.setFrame(bounds);
        pill.setCornerRadius(PILL.height / 2.0);
        pill.setBackgroundColor(Some(&CGColor::new_srgb(0.07, 0.07, 0.08, 0.94)));
        root.addSublayer(&pill);

        let icon = CALayer::layer();
        icon.setFrame(CGRect::new(
            CGPoint::new(PADDING, (PILL.height - ICON_SIZE) / 2.0),
            CGSize::new(ICON_SIZE, ICON_SIZE),
        ));
        pill.addSublayer(&icon);

        let mask = CALayer::layer();
        let opaque = CGColor::new_srgb(0.0, 0.0, 0.0, 1.0);
        let bars: [Retained<CALayer>; BAR_COUNT] = std::array::from_fn(|_| CALayer::layer());
        let mut center = BAR_WIDTH / 2.0;
        for bar in &bars {
            bar.setBounds(bar_bounds(BAR_MIN));
            bar.setPosition(CGPoint::new(center, BAR_MAX / 2.0));
            bar.setCornerRadius(BAR_WIDTH / 2.0);
            bar.setBackgroundColor(Some(&opaque));
            mask.addSublayer(bar);
            center += BAR_PITCH;
        }
        let width = center - BAR_PITCH + BAR_WIDTH / 2.0;
        mask.setFrame(CGRect::new(
            CGPoint::new(0.0, 0.0),
            CGSize::new(width, BAR_MAX),
        ));

        let gradient = CAGradientLayer::layer();
        gradient.setFrame(CGRect::new(
            CGPoint::new(PILL.width - PADDING - width, (PILL.height - BAR_MAX) / 2.0),
            CGSize::new(width, BAR_MAX),
        ));
        // SAFETY: `mask` is a new layer without a superlayer, as `CALayer.mask` requires.
        unsafe { gradient.setMask(Some(&mask)) };
        pill.addSublayer(&gradient);

        let layers = Self {
            root,
            pill,
            icon,
            gradient,
            bars,
            voice: colors([1.0, 0.45, 0.1], [1.0, 0.85, 0.2]),
            error: colors([0.9, 0.15, 0.15], [1.0, 0.35, 0.3]),
        };
        layers.set_palette(Palette::Voice);
        transaction(None, || layers.set_pill(0.0, CLOSED_SCALE));
        layers
    }

    /// The layer that the panel view hosts.
    pub(super) fn root(&self) -> &CALayer {
        &self.root
    }

    /// Sets the scale of the screen that shows the icon. Set the icon again after this call.
    pub(super) fn set_scale(&self, scale: CGFloat) {
        self.icon.setContentsScale(scale);
    }

    /// Shows the application icon, or no icon.
    pub(super) fn set_icon(&self, icon: Option<&NSImage>) {
        let contents: Option<&AnyObject> = icon.map(|icon| -> &AnyObject { icon });
        transaction(None, || {
            // SAFETY: on macOS, `CALayer.contents` accepts an `NSImage`. No contents clears the layer.
            unsafe { self.icon.setContents(contents) };
        });
    }

    pub(super) fn set_palette(&self, palette: Palette) {
        let colors = match palette {
            Palette::Voice => &self.voice,
            Palette::Error => &self.error,
        };
        transaction(None, || {
            // SAFETY: `colors` contains only `CGColor` objects, as `CAGradientLayer.colors` requires.
            unsafe { self.gradient.setColors(Some(colors)) };
        });
    }

    /// Gives the bars their minimum height, without animation.
    pub(super) fn reset_bars(&self) {
        transaction(None, || self.resize_bars(&[BAR_MIN; BAR_COUNT]));
    }

    /// Animates the bars to these heights.
    pub(super) fn move_bars(&self, heights: &[CGFloat; BAR_COUNT]) {
        transaction(Some(BAR_MOVE), || self.resize_bars(heights));
    }

    /// The pill grows to its full size and fades in.
    pub(super) fn open(&self) {
        transaction(Some(OPEN), || self.set_pill(1.0, 1.0));
    }

    /// The pill shrinks and fades out.
    pub(super) fn shrink(&self) {
        transaction(Some(SHRINK), || self.set_pill(0.0, CLOSED_SCALE));
    }

    fn resize_bars(&self, heights: &[CGFloat; BAR_COUNT]) {
        for (bar, &height) in self.bars.iter().zip(heights) {
            bar.setBounds(bar_bounds(height));
        }
    }

    fn set_pill(&self, opacity: f32, scale: CGFloat) {
        self.pill.setOpacity(opacity);
        self.pill
            .setTransform(CATransform3D::new_scale(scale, scale, 1.0));
    }
}

/// Converts the audio level to bar heights: decibels, fast rise, slow fall, and a small random variation.
#[derive(Debug)]
pub(super) struct BarHeights {
    loudness: CGFloat,
    random: u64,
}

impl Default for BarHeights {
    fn default() -> Self {
        Self {
            loudness: 0.0,
            random: 0x9E37_79B9_7F4A_7C15,
        }
    }
}

impl BarHeights {
    /// Starts again from silence.
    pub(super) fn reset(&mut self) {
        self.loudness = 0.0;
    }

    /// Returns the heights for the next frame.
    pub(super) fn next(&mut self, level: f32) -> [CGFloat; BAR_COUNT] {
        let target = loudness(level);
        let rate = if target > self.loudness { RISE } else { FALL };
        self.loudness += (target - self.loudness) * rate;
        WEIGHTS.map(|weight| {
            let variation = 1.0 + VARIATION * (2.0 * self.random() - 1.0);
            bar_height(self.loudness * weight * variation)
        })
    }

    fn random(&mut self) -> CGFloat {
        let mut bits = self.random;
        bits ^= bits << 13;
        bits ^= bits >> 7;
        bits ^= bits << 17;
        self.random = bits;
        CGFloat::from_bits(0x3FF0_0000_0000_0000 | (bits >> 12)) - 1.0
    }
}

/// Fixed heights that follow the bar weights, without variation.
pub(super) fn resting_heights(loudness: CGFloat) -> [CGFloat; BAR_COUNT] {
    WEIGHTS.map(|weight| bar_height(loudness * weight))
}

fn loudness(level: f32) -> CGFloat {
    let decibels = 20.0 * CGFloat::from(level).max(1e-6).log10();
    ((decibels - FLOOR_DB) / (CEILING_DB - FLOOR_DB)).clamp(0.0, 1.0)
}

fn bar_height(amount: CGFloat) -> CGFloat {
    BAR_MIN + (BAR_MAX - BAR_MIN) * amount.clamp(0.0, 1.0)
}

fn bar_bounds(height: CGFloat) -> CGRect {
    CGRect::new(CGPoint::new(0.0, 0.0), CGSize::new(BAR_WIDTH, height))
}

fn colors(bottom: [CGFloat; 3], top: [CGFloat; 3]) -> Retained<NSArray> {
    let [red, green, blue] = bottom;
    let bottom = CGColor::new_srgb(red, green, blue, 1.0);
    let [red, green, blue] = top;
    let top = CGColor::new_srgb(red, green, blue, 1.0);
    let bottom: &CFType = &bottom;
    let top: &CFType = &top;
    NSArray::from_slice(&[bottom.as_ref(), top.as_ref()])
}

/// Applies `changes` in one Core Animation transaction: animated for `duration`, or immediate.
fn transaction(duration: Option<Duration>, changes: impl FnOnce()) {
    CATransaction::begin();
    match duration {
        Some(duration) => CATransaction::setAnimationDuration(duration.as_secs_f64()),
        None => CATransaction::setDisableActions(true),
    }
    changes();
    CATransaction::commit();
}

#[cfg(test)]
mod tests {
    use super::{BAR_COUNT, BAR_MAX, BAR_MIN, BarHeights, loudness, resting_heights};

    const CENTER: usize = 3;

    fn fraction(height: f64) -> f64 {
        (height - BAR_MIN) / (BAR_MAX - BAR_MIN)
    }

    #[test]
    fn silence_shows_seven_short_bars() {
        let heights = BarHeights::default().next(0.0);

        assert_eq!(heights.len(), BAR_COUNT);
        assert!(heights.iter().all(|&height| fraction(height).abs() < 1e-9));
    }

    #[test]
    fn heights_stay_between_the_minimum_and_the_maximum() {
        let mut bars = BarHeights::default();

        for level in [0.0, 1e-9, 0.01, 0.3, 1.0, f32::NAN, -1.0, f32::INFINITY].repeat(20) {
            let heights = bars.next(level);
            assert!(
                heights
                    .iter()
                    .all(|height| (BAR_MIN..=BAR_MAX).contains(height)),
                "level {level}: {heights:?}"
            );
        }
    }

    #[test]
    fn the_bars_rise_fast_and_fall_slowly() {
        let mut bars = BarHeights::default();

        assert!(fraction(bars.next(1.0)[CENTER]) > 0.5);

        for _ in 0..60 {
            bars.next(1.0);
        }
        assert!(fraction(bars.next(0.0)[CENTER]) > 0.7);
    }

    #[test]
    fn the_level_maps_from_minus_50_to_minus_10_decibels() {
        assert!(loudness(0.0).abs() < 1e-9);
        assert!((loudness(10_f32.powf(-30.0 / 20.0)) - 0.5).abs() < 1e-3);
        assert!((loudness(1.0) - 1.0).abs() < 1e-9);
    }

    #[test]
    fn resting_heights_follow_the_weights() {
        let heights = resting_heights(1.0);

        assert!((heights[CENTER] - BAR_MAX).abs() < 1e-9);
        assert!(heights.iter().all(|&height| height <= heights[CENTER]));
    }
}

//! Core Animation layers of the orb: the pill, the application icon, the bars, and the fallback card.
//!
//! The root layer has the size of the card. The pill sits at its bottom center, and becomes the card.

#![expect(
    unsafe_code,
    reason = "the layer contents, the gradient colors, the mask, and the text layer string and font take objects whose class the compiler cannot check, and the truncation mode reads the Core Animation constant kCATruncationEnd, an external static"
)]

use std::time::Duration;

use objc2::rc::Retained;
use objc2::runtime::AnyObject;
use objc2_app_kit::{NSFont, NSImage};
use objc2_core_foundation::{CFType, CGFloat, CGPoint, CGRect, CGSize};
use objc2_core_graphics::CGColor;
use objc2_core_text::CTFont;
use objc2_foundation::{NSArray, NSString, ns_string};
use objc2_quartz_core::{
    CAGradientLayer, CALayer, CATextLayer, CATransaction, CATransform3D, kCATruncationEnd,
};

/// The size of the panel: the size of the card.
pub(super) const CANVAS: CGSize = CARD;
pub(super) const BAR_COUNT: usize = 7;
pub(super) const BAR_MIN: CGFloat = 6.0;
pub(super) const BAR_MOVE: Duration = Duration::from_millis(80);
pub(super) const SHRINK: Duration = Duration::from_millis(120);

const PILL: CGSize = CGSize::new(150.0, 56.0);
const CARD: CGSize = CGSize::new(340.0, 104.0);
const CARD_RADIUS: CGFloat = 16.0;
const CARD_PADDING: CGFloat = 16.0;
const CARD_CHARS: usize = 240;
const TEXT_SIZE: CGFloat = 14.0;
const HINT_SIZE: CGFloat = 12.0;
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
    /// Deep purple at the bottom, burgundy at the top.
    Voice,
    /// Red.
    Error,
}

/// The content of the orb.
///
/// `Card` borrows the final text. The type has no `Debug`, because `NSString` prints its content.
#[derive(Clone, Copy)]
pub(super) enum Shape<'a> {
    /// The application icon and the bars.
    Pill,
    /// The final text and the instruction to paste it.
    Card(&'a NSString),
}

/// The layer tree of the orb. The pill starts closed.
#[derive(Debug)]
pub(super) struct Layers {
    root: Retained<CALayer>,
    pill: Retained<CALayer>,
    icon: Retained<CALayer>,
    gradient: Retained<CAGradientLayer>,
    bars: [Retained<CALayer>; BAR_COUNT],
    text: Retained<CATextLayer>,
    hint: Retained<CATextLayer>,
    voice: Retained<NSArray>,
    error: Retained<NSArray>,
}

impl Layers {
    pub(super) fn new() -> Self {
        let root = CALayer::layer();
        root.setFrame(CGRect::new(CGPoint::new(0.0, 0.0), CANVAS));

        let pill = CALayer::layer();
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

        let text_width = CARD.width - 2.0 * CARD_PADDING;
        let text = text_layer(
            CGRect::new(
                CGPoint::new(CARD_PADDING, 34.0),
                CGSize::new(text_width, 56.0),
            ),
            TEXT_SIZE,
            &CGColor::new_srgb(0.95, 0.95, 0.97, 1.0),
        );
        pill.addSublayer(&text);
        let hint = text_layer(
            CGRect::new(
                CGPoint::new(CARD_PADDING, 12.0),
                CGSize::new(text_width, 16.0),
            ),
            HINT_SIZE,
            &CGColor::new_srgb(0.62, 0.62, 0.66, 1.0),
        );
        set_string(&hint, Some(ns_string!("Copied, press ⌘V")));
        pill.addSublayer(&hint);

        let layers = Self {
            root,
            pill,
            icon,
            gradient,
            bars,
            text,
            hint,
            voice: colors([0.29, 0.0, 0.66], [0.5, 0.0, 0.125]),
            error: colors([0.9, 0.15, 0.15], [1.0, 0.35, 0.3]),
        };
        layers.set_palette(Palette::Voice);
        layers.set_shape(Shape::Pill, None);
        transaction(None, || layers.set_pill(0.0, CLOSED_SCALE));
        layers
    }

    /// The layer that the panel view hosts.
    pub(super) fn root(&self) -> &CALayer {
        &self.root
    }

    /// Sets the scale of the screen that shows the icon and the text. Set the icon again after this call.
    pub(super) fn set_scale(&self, scale: CGFloat) {
        self.icon.setContentsScale(scale);
        self.text.setContentsScale(scale);
        self.hint.setContentsScale(scale);
    }

    /// Gives the pill its size and content: animated for `animation`, or immediate.
    ///
    /// The pill grows from its bottom center to the card. The pill shape clears the card text.
    pub(super) fn set_shape(&self, shape: Shape<'_>, animation: Option<Duration>) {
        let card = match shape {
            Shape::Pill => {
                transaction(None, || set_string(&self.text, None));
                false
            }
            Shape::Card(text) => {
                transaction(None, || set_string(&self.text, Some(text)));
                true
            }
        };
        let (size, radius) = if card {
            (CARD, CARD_RADIUS)
        } else {
            (PILL, PILL.height / 2.0)
        };
        let (pill_opacity, card_opacity) = if card { (0.0, 1.0) } else { (1.0, 0.0) };
        transaction(animation, || {
            self.pill
                .setBounds(CGRect::new(CGPoint::new(0.0, 0.0), size));
            self.pill
                .setPosition(CGPoint::new(CANVAS.width / 2.0, size.height / 2.0));
            self.pill.setCornerRadius(radius);
            self.icon.setOpacity(pill_opacity);
            self.gradient.setOpacity(pill_opacity);
            self.text.setOpacity(card_opacity);
            self.hint.setOpacity(card_opacity);
        });
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

/// The start of the final text that the card shows, with an ellipsis if the text is longer.
pub(super) fn excerpt(text: &str) -> String {
    let mut chars = text.chars();
    let mut excerpt: String = chars.by_ref().take(CARD_CHARS).collect();
    if chars.next().is_some() {
        excerpt.push('…');
    }
    excerpt
}

/// A hidden text layer with the system font that wraps its text and truncates it at the end.
fn text_layer(frame: CGRect, size: CGFloat, color: &CGColor) -> Retained<CATextLayer> {
    let layer = CATextLayer::layer();
    layer.setFrame(frame);
    layer.setFontSize(size);
    layer.setForegroundColor(Some(color));
    layer.setWrapped(true);
    let font = NSFont::systemFontOfSize(size);
    let font: &CTFont = font.as_ref();
    // SAFETY: `CATextLayer.font` accepts a `CTFont`.
    unsafe { layer.setFont(Some(font)) };
    // SAFETY: Core Animation sets this constant before `main` and never changes it.
    layer.setTruncationMode(unsafe { kCATruncationEnd });
    layer.setOpacity(0.0);
    layer
}

fn set_string(layer: &CATextLayer, text: Option<&NSString>) {
    let string: Option<&AnyObject> = text.map(|text| -> &AnyObject { text });
    // SAFETY: `CATextLayer.string` accepts an `NSString`. No string clears the layer.
    unsafe { layer.setString(string) };
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
    use super::{
        BAR_COUNT, BAR_MAX, BAR_MIN, BarHeights, CARD_CHARS, excerpt, loudness, resting_heights,
    };

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

    #[test]
    fn a_short_text_shows_completely() {
        assert_eq!(excerpt("Bonjour, ça va ?"), "Bonjour, ça va ?");
    }

    #[test]
    fn a_long_text_shows_its_start_and_an_ellipsis() {
        let text = "é".repeat(CARD_CHARS + 1);

        let shown = excerpt(&text);

        assert_eq!(shown.chars().count(), CARD_CHARS + 1);
        assert!(shown.ends_with("é…"));
        assert_eq!(excerpt(&"é".repeat(CARD_CHARS)), "é".repeat(CARD_CHARS));
    }
}

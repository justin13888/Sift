//! Colour, in the perceptual space D-27's transform requires.
//!
//! # Why a perceptual space rather than HSL
//!
//! D-27 rejects naive HSL inversion for two named failures: it **muddies mid-tones** and it
//! **shifts hues**. A brand's blue becomes a different blue, which is the visible defect
//! that makes users turn the feature off — and FR-31 defaults it off partly because of
//! exactly that risk.
//!
//! Oklab is used because lightness in it is close to perceived lightness, so "invert and
//! compress the lightness" is an operation on the thing a reader actually sees rather than
//! on a number that correlates with it.

/// A colour, in sRGB, with alpha.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Rgb {
    pub r: f64,
    pub g: f64,
    pub b: f64,
    pub a: f64,
}

/// Oklab: perceptual lightness, and two opponent axes.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Oklab {
    /// 0 is black, 1 is white.
    pub l: f64,
    pub a: f64,
    pub b: f64,
}

impl Oklab {
    /// Distance from the neutral axis. **Chroma is what D-27 clamps** to avoid neon
    /// artefacts, and what the gradient rule tests every stop against.
    #[must_use]
    pub fn chroma(self) -> f64 {
        self.a.hypot(self.b)
    }

    /// Hue angle, which the transform **preserves**.
    #[must_use]
    pub fn hue(self) -> f64 {
        self.b.atan2(self.a)
    }

    /// Rebuild from lightness, chroma and hue.
    #[must_use]
    pub fn from_lch(l: f64, chroma: f64, hue: f64) -> Self {
        Self {
            l,
            a: chroma * hue.cos(),
            b: chroma * hue.sin(),
        }
    }
}

fn srgb_to_linear(c: f64) -> f64 {
    if c <= 0.040_45 {
        c / 12.92
    } else {
        ((c + 0.055) / 1.055).powf(2.4)
    }
}

fn linear_to_srgb(c: f64) -> f64 {
    if c <= 0.003_130_8 {
        12.92 * c
    } else {
        1.055 * c.powf(1.0 / 2.4) - 0.055
    }
}

impl Rgb {
    #[must_use]
    pub fn to_oklab(self) -> Oklab {
        let (r, g, b) = (
            srgb_to_linear(self.r),
            srgb_to_linear(self.g),
            srgb_to_linear(self.b),
        );
        let l = 0.412_221_470_8 * r + 0.536_332_536_3 * g + 0.051_445_992_9 * b;
        let m = 0.211_903_498_2 * r + 0.680_699_545_1 * g + 0.107_396_956_6 * b;
        let s = 0.088_302_461_9 * r + 0.281_718_837_6 * g + 0.629_978_700_5 * b;
        let (l, m, s) = (l.cbrt(), m.cbrt(), s.cbrt());
        Oklab {
            l: 0.210_454_255_3 * l + 0.793_617_785_0 * m - 0.004_072_046_8 * s,
            a: 1.977_998_495_1 * l - 2.428_592_205_0 * m + 0.450_593_709_9 * s,
            b: 0.025_904_037_1 * l + 0.782_771_766_2 * m - 0.808_675_766_0 * s,
        }
    }

    #[must_use]
    pub fn from_oklab(c: Oklab, alpha: f64) -> Self {
        let l = c.l + 0.396_337_777_4 * c.a + 0.215_803_757_3 * c.b;
        let m = c.l - 0.105_561_345_8 * c.a - 0.063_854_172_8 * c.b;
        let s = c.l - 0.089_484_177_5 * c.a - 1.291_485_548_0 * c.b;
        let (l, m, s) = (l * l * l, m * m * m, s * s * s);
        Self {
            r: linear_to_srgb(4.076_741_662_1 * l - 3.307_711_591_3 * m + 0.230_969_929_2 * s)
                .clamp(0.0, 1.0),
            g: linear_to_srgb(-1.268_438_004_6 * l + 2.609_757_401_1 * m - 0.341_319_396_5 * s)
                .clamp(0.0, 1.0),
            b: linear_to_srgb(-0.004_196_086_3 * l - 0.703_418_614_7 * m + 1.707_614_701_0 * s)
                .clamp(0.0, 1.0),
            a: alpha,
        }
    }

    /// WCAG relative luminance, for the contrast ratio below.
    #[must_use]
    pub fn relative_luminance(self) -> f64 {
        0.2126 * srgb_to_linear(self.r)
            + 0.7152 * srgb_to_linear(self.g)
            + 0.0722 * srgb_to_linear(self.b)
    }

    #[must_use]
    pub fn to_css(self) -> String {
        let to_byte = |c: f64| (c * 255.0).round().clamp(0.0, 255.0) as u8;
        if (self.a - 1.0).abs() < f64::EPSILON {
            format!(
                "#{:02x}{:02x}{:02x}",
                to_byte(self.r),
                to_byte(self.g),
                to_byte(self.b)
            )
        } else {
            format!(
                "rgba({},{},{},{})",
                to_byte(self.r),
                to_byte(self.g),
                to_byte(self.b),
                self.a
            )
        }
    }
}

/// The contrast ratio between two colours.
///
/// **The threshold this is compared against is not recorded anywhere**, which is why NFR-47
/// is registered as an outstanding gate value: the number has to be calibrated against
/// fidelity-corpus pairs already agreed to render acceptably, and that corpus does not
/// exist yet. [`ACCEPTABLE_CONTRAST`] is a stated starting value and says so.
#[must_use]
pub fn contrast_ratio(a: Rgb, b: Rgb) -> f64 {
    let (x, y) = (a.relative_luminance(), b.relative_luminance());
    let (lighter, darker) = if x > y { (x, y) } else { (y, x) };
    (lighter + 0.05) / (darker + 0.05)
}

/// Provisional. See NFR-47's outstanding-value issue: this is a starting point, not a
/// derived threshold, and moving it is an amendment rather than a tuning.
pub const ACCEPTABLE_CONTRAST: f64 = 4.5;

/// The raised contrast threshold, for a reader whose system asks for increased contrast.
///
/// [The UI shell](../../../../docs/architecture/ui-shell.md) requires the dark transform's
/// repair target to move under that preference —
/// a user who asked the system for more contrast has not asked for it everywhere except
/// inside the message — and does not say to what. This is the third value of the same
/// unrecorded set as [`ACCEPTABLE_CONTRAST`] and [`NEAR_NEUTRAL_CHROMA`], registered in
/// issue #26 and blocked on the fidelity corpus (Q-10).
///
/// **Provisional.** A stated starting value rather than a derived one: the enhanced-contrast
/// ratio accessibility guidance already names for body text. It MUST stay above
/// [`ACCEPTABLE_CONTRAST`], or the preference would lower the bar it exists to raise; the
/// build holds that. Moving it is an amendment rather than a tuning.
pub const INCREASED_CONTRAST_THRESHOLD: f64 = 7.0;

// An amendment that moved either value past the other fails to compile rather than shipping
// a preference that lowers the bar.
const _: () = assert!(INCREASED_CONTRAST_THRESHOLD > ACCEPTABLE_CONTRAST);

/// The threshold step 5 of the dark transform repairs toward.
///
/// One place decides which of the two provisional values applies, so the transform and
/// anything that later reports against NFR-47 cannot disagree about it.
#[must_use]
pub const fn contrast_threshold(increased_contrast: bool) -> f64 {
    if increased_contrast {
        INCREASED_CONTRAST_THRESHOLD
    } else {
        ACCEPTABLE_CONTRAST
    }
}

/// The chroma below which a colour counts as near-neutral.
///
/// D-27's gradient rule tests **every** stop against this: a gradient is transformed only
/// when all of its stops are below it, so a mixed-stop gradient is excluded by construction
/// rather than by a special case. The same threshold governs borders, shadows and outlines.
///
/// Also provisional, and from the same unrecorded set.
pub const NEAR_NEUTRAL_CHROMA: f64 = 0.04;

/// Parse a CSS colour.
///
/// Returns `None` for anything unrecognised, and the transform then **leaves that
/// declaration alone** — which is the right failure: an untransformed colour is the
/// sender's, and a guessed one is Sift's.
#[must_use]
pub fn parse(value: &str) -> Option<Rgb> {
    let v = value.trim().to_ascii_lowercase();
    if let Some(hex) = v.strip_prefix('#') {
        return parse_hex(hex);
    }
    if let Some(inner) = v.strip_prefix("rgb(").and_then(|s| s.strip_suffix(')')) {
        return parse_rgb(inner, 1.0);
    }
    if let Some(inner) = v.strip_prefix("rgba(").and_then(|s| s.strip_suffix(')')) {
        let parts: Vec<&str> = inner.split(',').collect();
        let alpha = parts
            .get(3)
            .and_then(|a| a.trim().parse().ok())
            .unwrap_or(1.0);
        return parse_rgb(&parts[..parts.len().min(3)].join(","), alpha);
    }
    named(&v)
}

fn parse_hex(hex: &str) -> Option<Rgb> {
    let expand = |c: char| u8::from_str_radix(&format!("{c}{c}"), 16).ok();
    let bytes: Vec<u8> = match hex.len() {
        3 | 4 => hex.chars().map(expand).collect::<Option<_>>()?,
        6 | 8 => (0..hex.len() / 2)
            .map(|i| u8::from_str_radix(&hex[i * 2..i * 2 + 2], 16).ok())
            .collect::<Option<_>>()?,
        _ => return None,
    };
    Some(Rgb {
        r: f64::from(*bytes.first()?) / 255.0,
        g: f64::from(*bytes.get(1)?) / 255.0,
        b: f64::from(*bytes.get(2)?) / 255.0,
        a: bytes.get(3).map_or(1.0, |a| f64::from(*a) / 255.0),
    })
}

fn parse_rgb(inner: &str, alpha: f64) -> Option<Rgb> {
    let parts: Vec<f64> = inner
        .split(',')
        .map(|p| {
            let p = p.trim();
            p.strip_suffix('%').map_or_else(
                || p.parse::<f64>().map(|v| v / 255.0),
                |pct| pct.parse::<f64>().map(|v| v / 100.0),
            )
        })
        .collect::<Result<_, _>>()
        .ok()?;
    Some(Rgb {
        r: (*parts.first()?).clamp(0.0, 1.0),
        g: (*parts.get(1)?).clamp(0.0, 1.0),
        b: (*parts.get(2)?).clamp(0.0, 1.0),
        a: alpha,
    })
}

/// The named colours mail actually uses.
#[must_use]
pub fn named(name: &str) -> Option<Rgb> {
    let hex = match name.trim().to_ascii_lowercase().as_str() {
        "black" => "000000",
        "white" => "ffffff",
        "red" => "ff0000",
        "green" => "008000",
        "blue" => "0000ff",
        "yellow" => "ffff00",
        "cyan" | "aqua" => "00ffff",
        "magenta" | "fuchsia" => "ff00ff",
        "gray" | "grey" => "808080",
        "silver" => "c0c0c0",
        "lightgray" | "lightgrey" => "d3d3d3",
        "darkgray" | "darkgrey" => "a9a9a9",
        "maroon" => "800000",
        "olive" => "808000",
        "lime" => "00ff00",
        "navy" => "000080",
        "teal" => "008080",
        "purple" => "800080",
        "orange" => "ffa500",
        "transparent" => {
            return Some(Rgb {
                r: 0.0,
                g: 0.0,
                b: 0.0,
                a: 0.0,
            });
        }
        _ => return None,
    };
    parse_hex(hex)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn rgb(r: u8, g: u8, b: u8) -> Rgb {
        Rgb {
            r: f64::from(r) / 255.0,
            g: f64::from(g) / 255.0,
            b: f64::from(b) / 255.0,
            a: 1.0,
        }
    }

    #[test]
    fn oklab_round_trips() {
        for c in [
            rgb(255, 0, 0),
            rgb(0, 128, 64),
            rgb(255, 255, 255),
            rgb(0, 0, 0),
            rgb(17, 34, 51),
        ] {
            let back = Rgb::from_oklab(c.to_oklab(), 1.0);
            for (a, b) in [(c.r, back.r), (c.g, back.g), (c.b, back.b)] {
                assert!((a - b).abs() < 0.01, "{c:?} -> {back:?}");
            }
        }
    }

    #[test]
    fn lightness_is_ordered_the_way_a_reader_sees_it() {
        // The property the whole choice of space rests on: if lightness in this space did
        // not track perceived lightness, "invert the lightness" would be operating on a
        // number that merely correlates with what a reader sees.
        let black = rgb(0, 0, 0).to_oklab().l;
        let grey = rgb(128, 128, 128).to_oklab().l;
        let white = rgb(255, 255, 255).to_oklab().l;
        assert!(black < grey && grey < white);
        assert!(black < 0.01 && white > 0.99);
    }

    #[test]
    fn a_saturated_colour_has_chroma_and_a_grey_does_not() {
        assert!(rgb(255, 0, 0).to_oklab().chroma() > NEAR_NEUTRAL_CHROMA);
        assert!(rgb(128, 128, 128).to_oklab().chroma() < NEAR_NEUTRAL_CHROMA);
        assert!(rgb(250, 250, 250).to_oklab().chroma() < NEAR_NEUTRAL_CHROMA);
    }

    #[test]
    fn the_increased_contrast_preference_raises_the_bar_rather_than_lowering_it() {
        // The UI shell's requirement: a user who asked the system for more contrast has not
        // asked for it everywhere except inside the message. A provisional value that fell
        // below the ordinary one would invert that request; the ordering itself is held at
        // compile time beside the constant, and this holds the selection.
        assert!(contrast_threshold(true) > contrast_threshold(false));
        assert!((contrast_threshold(false) - ACCEPTABLE_CONTRAST).abs() < f64::EPSILON);
        assert!((contrast_threshold(true) - INCREASED_CONTRAST_THRESHOLD).abs() < f64::EPSILON);
    }

    #[test]
    fn hex_parses_in_every_length_mail_uses() {
        assert_eq!(parse("#f00"), Some(rgb(255, 0, 0)));
        assert_eq!(parse("#ff0000"), Some(rgb(255, 0, 0)));
        assert_eq!(parse("#FF0000"), Some(rgb(255, 0, 0)));
        let with_alpha = parse("#ff000080").expect("8-digit hex");
        assert!((with_alpha.a - 0.5).abs() < 0.01);
    }

    #[test]
    fn functional_and_named_colours_parse() {
        assert_eq!(parse("rgb(255, 0, 0)"), Some(rgb(255, 0, 0)));
        assert_eq!(parse("red"), Some(rgb(255, 0, 0)));
        assert_eq!(parse("  WHITE  "), Some(rgb(255, 255, 255)));
        assert_eq!(parse("transparent").map(|c| c.a), Some(0.0));
        let translucent = parse("rgba(255,0,0,0.5)").expect("rgba");
        assert!((translucent.a - 0.5).abs() < f64::EPSILON);
    }

    #[test]
    fn an_unrecognised_colour_does_not_parse() {
        // The transform then leaves that declaration alone, which is the right failure: an
        // untransformed colour is the sender's, and a guessed one would be Sift's.
        for v in [
            "",
            "#",
            "#12345",
            "notacolour",
            "color-mix(in oklab, red, blue)",
            "var(--brand)",
        ] {
            assert_eq!(parse(v), None, "{v} parsed");
        }
    }

    #[test]
    fn contrast_is_symmetric_and_bounded() {
        let (black, white) = (rgb(0, 0, 0), rgb(255, 255, 255));
        assert!((contrast_ratio(black, white) - 21.0).abs() < 0.1);
        assert!((contrast_ratio(white, black) - contrast_ratio(black, white)).abs() < f64::EPSILON);
        assert!((contrast_ratio(white, white) - 1.0).abs() < f64::EPSILON);
    }

    #[test]
    fn css_output_round_trips_through_the_parser() {
        for c in [rgb(17, 34, 51), rgb(255, 255, 255), rgb(0, 0, 0)] {
            assert_eq!(parse(&c.to_css()), Some(c));
        }
    }
}

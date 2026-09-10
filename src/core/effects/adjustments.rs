use super::{ImageFilter, ViewEffects, ViewTransform};
use serde::{Deserialize, Serialize};

/// Fixed-point controls keep persisted values, CPU rendering and GPU cache keys identical.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize)]
pub struct ToneControls {
    pub gamma_pct: u16,
    pub brightness_pct: i16,
    pub contrast_pct: u16,
    pub black: u8,
    pub white: u8,
    pub filter_pct: u16,
}

impl<'de> Deserialize<'de> for ToneControls {
    fn deserialize<D: serde::Deserializer<'de>>(d: D) -> Result<Self, D::Error> {
        let v = serde_json::Value::deserialize(d)?;
        let mut controls = Self::default();
        macro_rules! read {
            ($field:ident, $min:expr, $max:expr) => {
                if let Some(value) = v
                    .get(stringify!($field))
                    .and_then(|v| v.as_i64())
                    .filter(|v| ($min..=$max).contains(v))
                {
                    controls.$field = value as _;
                }
            };
        }
        read!(gamma_pct, 10, 400);
        read!(brightness_pct, -100, 100);
        read!(contrast_pct, 0, 200);
        read!(black, 0, 254);
        read!(white, 1, 255);
        read!(filter_pct, 0, 200);
        if controls.black >= controls.white {
            controls.black = 0;
        }
        Ok(controls)
    }
}

impl Default for ToneControls {
    fn default() -> Self {
        Self {
            gamma_pct: 120,
            brightness_pct: 0,
            contrast_pct: 100,
            black: 0,
            white: 255,
            filter_pct: 100,
        }
    }
}

impl ToneControls {
    pub fn normalize(&mut self) {
        self.gamma_pct = self.gamma_pct.clamp(10, 400);
        self.brightness_pct = self.brightness_pct.clamp(-100, 100);
        self.contrast_pct = self.contrast_pct.min(200);
        self.filter_pct = self.filter_pct.min(200);
        if self.black >= self.white {
            self.black = 0;
            self.white = 255;
        }
    }

    pub fn color_is_neutral(self) -> bool {
        self.brightness_pct == 0 && self.contrast_pct == 100 && self.black == 0 && self.white == 255
    }

    pub fn channel(self, value: u8, gamma: bool, invert: bool) -> u8 {
        if self.color_is_neutral() && !gamma {
            return if invert { 255 - value } else { value };
        }
        let black = self.black as f32 / 255.0;
        let white = self.white as f32 / 255.0;
        let mut v =
            ((value as f32 / 255.0 - black) / (white - black).max(1.0 / 255.0)).clamp(0.0, 1.0);
        v = ((v - 0.5) * (self.contrast_pct as f32 / 100.0)
            + 0.5
            + self.brightness_pct as f32 / 100.0)
            .clamp(0.0, 1.0);
        if gamma {
            v = v.powf(100.0 / self.gamma_pct.max(1) as f32);
        }
        // Match the legacy gamma toggle: round before integer inversion.
        let v = (v * 255.0).round().clamp(0.0, 255.0) as u8;
        if invert {
            255 - v
        } else {
            v
        }
    }
}

/// Global viewing adjustments deliberately exclude page orientation and book identity.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Default, Serialize, Deserialize)]
#[serde(default)]
pub struct SavedViewAdjustments {
    #[serde(deserialize_with = "tolerant")]
    pub filter: ImageFilter,
    #[serde(deserialize_with = "tolerant")]
    pub gamma: bool,
    #[serde(deserialize_with = "tolerant")]
    pub invert_colors: bool,
    pub tone: ToneControls,
}

impl SavedViewAdjustments {
    pub fn effects(self, transform: ViewTransform) -> ViewEffects {
        let mut tone = self.tone;
        tone.normalize();
        ViewEffects {
            transform,
            filter: self.filter,
            gamma: self.gamma,
            invert_colors: self.invert_colors,
            tone,
        }
    }
}

impl ViewEffects {
    /// Canonical pixel request: inactive control values do not invalidate caches.
    pub fn for_render(mut self) -> Self {
        self.tone.normalize();
        self.transform.rotation_quadrants %= 4;
        if !self.gamma {
            self.tone.gamma_pct = 120;
        }
        if self.tone.filter_pct == 0 {
            self.filter = ImageFilter::None;
        }
        if self.filter == ImageFilter::None {
            self.tone.filter_pct = 100;
        }
        self
    }

    pub fn saved(self) -> SavedViewAdjustments {
        SavedViewAdjustments {
            filter: self.filter,
            gamma: self.gamma,
            invert_colors: self.invert_colors,
            tone: self.tone,
        }
    }

    pub fn uncorrected(self) -> Self {
        Self {
            transform: self.transform,
            ..Self::default()
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn neutral_and_legacy_gamma_match() {
        let tone = ToneControls::default();
        for v in 0..=255 {
            assert_eq!(tone.channel(v, false, false), v);
            assert_eq!(tone.channel(v, true, false), super::super::gamma_channel(v));
            assert_eq!(
                tone.channel(v, true, true),
                255 - super::super::gamma_channel(v)
            );
        }
    }
    #[test]
    fn invalid_controls_are_bounded() {
        let mut tone = ToneControls {
            gamma_pct: 0,
            brightness_pct: 300,
            contrast_pct: 999,
            black: 255,
            white: 0,
            filter_pct: 900,
        };
        tone.normalize();
        assert_eq!(
            (
                tone.gamma_pct,
                tone.brightness_pct,
                tone.contrast_pct,
                tone.black,
                tone.white,
                tone.filter_pct
            ),
            (10, 100, 200, 0, 255, 200)
        );
    }

    #[test]
    fn adjustment_transfer_table_preserves_pixels_and_alpha() {
        let pixels = (0..=255)
            .map(|v| egui::Color32::from_rgba_unmultiplied(v, 255 - v, v / 2, 128))
            .collect();
        let image = egui::ColorImage::new([256, 1], pixels);
        for gamma in [false, true] {
            for invert in [false, true] {
                let effects = ViewEffects {
                    gamma,
                    invert_colors: invert,
                    tone: ToneControls {
                        gamma_pct: 175,
                        brightness_pct: -7,
                        contrast_pct: 135,
                        black: 12,
                        white: 243,
                        ..Default::default()
                    },
                    ..Default::default()
                };
                let output = super::super::apply_effects_to_image(&image, effects);
                let owned = super::super::apply_effects_to_owned_image(image.clone(), effects);
                assert_eq!(owned.pixels, output.pixels);
                for (source, actual) in image.pixels.iter().zip(output.pixels) {
                    let [r, g, b, a] = source.to_srgba_unmultiplied();
                    assert_eq!(
                        actual,
                        egui::Color32::from_rgba_unmultiplied(
                            effects.tone.channel(r, gamma, invert),
                            effects.tone.channel(g, gamma, invert),
                            effects.tone.channel(b, gamma, invert),
                            a
                        )
                    );
                }
            }
        }
    }
}

fn tolerant<'de, D: serde::Deserializer<'de>, T: serde::de::DeserializeOwned + Default>(
    d: D,
) -> Result<T, D::Error> {
    let v = serde_json::Value::deserialize(d)?;
    Ok(serde_json::from_value(v).unwrap_or_default())
}

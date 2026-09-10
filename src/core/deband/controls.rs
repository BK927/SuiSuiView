use super::{DebandParams, DebandStrength};
use crate::core::i18n::I18n;
use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize)]
pub struct CustomDeband {
    pub threshold_hundredths: u16,
    pub radius: u8,
    pub grain_hundredths: u16,
    pub iterations: u8,
}
impl<'de> Deserialize<'de> for CustomDeband {
    fn deserialize<D: serde::Deserializer<'de>>(d: D) -> Result<Self, D::Error> {
        let v = serde_json::Value::deserialize(d)?;
        let mut controls = Self::default();
        macro_rules! read {
            ($field:ident, $min:expr, $max:expr) => {
                if let Some(value) = v
                    .get(stringify!($field))
                    .and_then(|v| v.as_u64())
                    .filter(|v| ($min..=$max).contains(v))
                {
                    controls.$field = value as _;
                }
            };
        }
        read!(threshold_hundredths, 0, 800);
        read!(radius, 1, 32);
        read!(grain_hundredths, 0, 200);
        read!(iterations, 1, 4);
        Ok(controls)
    }
}
impl Default for CustomDeband {
    fn default() -> Self {
        Self::MEDIUM
    }
}
impl CustomDeband {
    pub const MEDIUM: Self = Self {
        threshold_hundredths: 300,
        radius: 12,
        grain_hundredths: 80,
        iterations: 3,
    };
    pub fn normalize(&mut self) {
        self.threshold_hundredths = self.threshold_hundredths.min(800);
        self.radius = self.radius.clamp(1, 32);
        self.grain_hundredths = self.grain_hundredths.min(200);
        self.iterations = self.iterations.clamp(1, 4);
    }
    fn params(self) -> DebandParams {
        DebandParams {
            iterations: self.iterations as u32,
            base_radius: self.radius as f32,
            threshold: self.threshold_hundredths as f32 / 100.0,
            grain: self.grain_hundredths as f32 / 100.0,
        }
    }
}

/// Render requests carry values, never a reference to mutable global preferences.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Default)]
pub enum ResolvedDeband {
    #[default]
    Off,
    Weak,
    Medium,
    Strong,
    Custom(CustomDeband),
}
impl ResolvedDeband {
    pub fn resolve(strength: DebandStrength, mut custom: CustomDeband) -> Self {
        custom.normalize();
        match strength {
            DebandStrength::Off => Self::Off,
            DebandStrength::Weak => Self::Weak,
            DebandStrength::Medium => Self::Medium,
            DebandStrength::Strong => Self::Strong,
            DebandStrength::Custom if custom.threshold_hundredths == 0 => Self::Off,
            DebandStrength::Custom => {
                for preset in [
                    DebandStrength::Weak,
                    DebandStrength::Medium,
                    DebandStrength::Strong,
                ] {
                    let p = preset.params().unwrap();
                    if (p.threshold * 100.0).round() as u16 == custom.threshold_hundredths
                        && p.base_radius as u8 == custom.radius
                        && (p.grain * 100.0).round() as u16 == custom.grain_hundredths
                        && p.iterations as u8 == custom.iterations
                    {
                        return Self::resolve(preset, custom);
                    }
                }
                Self::Custom(custom)
            }
        }
    }
    pub(crate) fn params(self) -> Option<DebandParams> {
        match self {
            Self::Off => None,
            Self::Weak => DebandStrength::Weak.params(),
            Self::Medium => DebandStrength::Medium.params(),
            Self::Strong => DebandStrength::Strong.params(),
            Self::Custom(c) => Some(c.params()),
        }
    }
    pub fn strength(self) -> DebandStrength {
        match self {
            Self::Off => DebandStrength::Off,
            Self::Weak => DebandStrength::Weak,
            Self::Medium => DebandStrength::Medium,
            Self::Strong => DebandStrength::Strong,
            Self::Custom(_) => DebandStrength::Custom,
        }
    }
    pub fn is_active(self) -> bool {
        !matches!(self, Self::Off)
    }
    pub fn token(self) -> &'static str {
        self.strength().token()
    }
    pub fn label_i18n(self, i18n: I18n) -> String {
        self.strength().label_i18n(i18n)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::hash::{Hash, Hasher};
    #[test]
    fn every_custom_value_affects_cache_identity() {
        let key = |c| {
            let mut h = std::collections::hash_map::DefaultHasher::new();
            ResolvedDeband::Custom(c).hash(&mut h);
            h.finish()
        };
        let base = CustomDeband::MEDIUM;
        for other in [
            CustomDeband {
                threshold_hundredths: 301,
                ..base
            },
            CustomDeband { radius: 13, ..base },
            CustomDeband {
                grain_hundredths: 81,
                ..base
            },
            CustomDeband {
                iterations: 4,
                ..base
            },
        ] {
            assert_ne!(key(base), key(other));
        }
    }
}

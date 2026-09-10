use super::{CpuScaleFilter, WgpuDownscaleMethod};
use serde::{Deserialize, Deserializer, Serialize};

/// One malformed new setting must never discard unrelated settings or reading history.
pub fn deserialize_or_default<'de, D, T>(deserializer: D) -> Result<T, D::Error>
where
    D: Deserializer<'de>,
    T: serde::de::DeserializeOwned + Default,
{
    let value = serde_json::Value::deserialize(deserializer)?;
    Ok(serde_json::from_value(value).unwrap_or_default())
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(default)]
pub struct ExpertDownscale {
    #[serde(deserialize_with = "deserialize_or_default")]
    pub cpu: CpuDownscaleChoice,
    #[serde(deserialize_with = "deserialize_or_default")]
    pub gpu: GpuDownscaleChoice,
}
impl Default for ExpertDownscale {
    fn default() -> Self {
        Self {
            cpu: CpuDownscaleChoice::default(),
            gpu: GpuDownscaleChoice::default(),
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Default, Serialize, Deserialize)]
pub enum CpuDownscaleChoice {
    #[default]
    CatmullRom,
    Mitchell,
    Lanczos3,
}
impl CpuDownscaleChoice {
    pub const ALL: [Self; 3] = [Self::CatmullRom, Self::Mitchell, Self::Lanczos3];
    pub fn filter(self) -> CpuScaleFilter {
        match self {
            Self::CatmullRom => CpuScaleFilter::CatmullRom,
            Self::Mitchell => CpuScaleFilter::Mitchell,
            Self::Lanczos3 => CpuScaleFilter::Lanczos3,
        }
    }
}
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Default, Serialize, Deserialize)]
pub enum GpuDownscaleChoice {
    #[default]
    Lanczos3,
    Mitchell,
    Hamming,
}
impl GpuDownscaleChoice {
    pub const ALL: [Self; 3] = [Self::Lanczos3, Self::Mitchell, Self::Hamming];
    pub fn method(self) -> WgpuDownscaleMethod {
        match self {
            Self::Lanczos3 => WgpuDownscaleMethod::PyramidLanczos3,
            Self::Mitchell => WgpuDownscaleMethod::PyramidMitchell,
            Self::Hamming => WgpuDownscaleMethod::PyramidHamming,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::core::deband::{CustomDeband, DebandStrength, ResolvedDeband};
    use crate::core::state::AppSettings;

    #[test]
    fn adjustment_settings_recover_individual_fields() {
        let settings: AppSettings = serde_json::from_value(serde_json::json!({
            "view_adjustments": {"gamma": true, "filter": "invalid", "tone": {
                "gamma_pct": 175, "brightness_pct": "broken", "contrast_pct": 140,
                "black": 22, "white": 999, "filter_pct": 75}},
            "custom_deband": {"radius": 19, "iterations": "broken", "grain_hundredths": 30},
            "expert_downscale": {"cpu": "Mitchell", "gpu": "retired"},
            "background_refine": "broken", "deband": "Medium"
        }))
        .unwrap();
        let tone = settings.view_adjustments.tone;
        assert!(settings.view_adjustments.gamma);
        assert_eq!(
            (
                tone.gamma_pct,
                tone.brightness_pct,
                tone.contrast_pct,
                tone.black,
                tone.white
            ),
            (175, 0, 140, 22, 255)
        );
        assert_eq!(settings.expert_downscale.cpu, CpuDownscaleChoice::Mitchell);
        assert_eq!(settings.expert_downscale.gpu, GpuDownscaleChoice::Lanczos3);
        assert_eq!(
            (
                settings.custom_deband.radius,
                settings.custom_deband.iterations
            ),
            (19, 3)
        );
        assert!(!settings.background_refine);
        assert_eq!(settings.deband, DebandStrength::Medium);
    }

    #[test]
    fn legacy_adjustment_defaults_and_roundtrip() {
        for strength in ["Off", "Weak", "Medium", "Strong"] {
            let old: AppSettings = serde_json::from_value(serde_json::json!({"deband": strength,
                "wgpu_downscale_method": "PyramidHamming", "cpu_downscale_filter": "Lanczos3"}))
            .unwrap();
            assert_eq!(old.expert_downscale, ExpertDownscale::default());
            assert_eq!(old.view_adjustments, Default::default());
            let loaded: AppSettings =
                serde_json::from_str(&serde_json::to_string(&old).unwrap()).unwrap();
            assert_eq!(loaded.deband, old.deband);
            assert_eq!(loaded.view_adjustments, old.view_adjustments);
        }
        assert_eq!(
            ResolvedDeband::resolve(DebandStrength::Custom, CustomDeband::MEDIUM),
            ResolvedDeband::Medium
        );
    }
}

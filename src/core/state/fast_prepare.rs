use crate::core::i18n::I18n;
use serde::{Deserialize, Deserializer, Serialize};

/// A format exception to the global fast-preparation switch. The global switch
/// remains authoritative: turning it off disables every sampled/scaled path.
#[derive(Debug, Default, Clone, Copy, PartialEq, Eq, Hash, Serialize)]
pub enum FastPrepareOverride {
    #[default]
    Inherit,
    On,
    Off,
}

impl<'de> Deserialize<'de> for FastPrepareOverride {
    fn deserialize<D: Deserializer<'de>>(deserializer: D) -> Result<Self, D::Error> {
        let value = serde_json::Value::deserialize(deserializer)?;
        Ok(match value.as_str() {
            Some("On") => Self::On,
            Some("Off") => Self::Off,
            _ => Self::Inherit,
        })
    }
}

impl FastPrepareOverride {
    pub const ALL: [Self; 3] = [Self::Inherit, Self::On, Self::Off];

    pub fn enabled(self, master_enabled: bool) -> bool {
        master_enabled && self != Self::Off
    }

    pub fn label_i18n(self, i18n: I18n) -> String {
        i18n.text(match self {
            Self::Inherit => "settings.decoder.fast_prepare.inherit",
            Self::On => "state.on",
            Self::Off => "state.off",
        })
    }
}

#[derive(Debug, Default, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(default)]
pub struct FastPrepareOverrides {
    pub jpeg: FastPrepareOverride,
    pub png: FastPrepareOverride,
    pub webp: FastPrepareOverride,
    pub bmp: FastPrepareOverride,
    pub gif: FastPrepareOverride,
}

impl FastPrepareOverrides {
    /// The resolved mask is useful in diagnostic cache names. DecodeOptions also
    /// hashes the original choices, so an override change cannot reuse stale work.
    pub fn enabled_mask(self, master_enabled: bool) -> u8 {
        [self.jpeg, self.png, self.webp, self.bmp, self.gif]
            .into_iter()
            .enumerate()
            .fold(0, |mask, (bit, value)| {
                mask | (u8::from(value.enabled(master_enabled)) << bit)
            })
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn fast_prepare_master_off_overrules_every_format_choice() {
        for value in FastPrepareOverride::ALL {
            assert!(!value.enabled(false));
        }
        assert!(FastPrepareOverride::Inherit.enabled(true));
        assert!(FastPrepareOverride::On.enabled(true));
        assert!(!FastPrepareOverride::Off.enabled(true));
    }

    #[test]
    fn fast_prepare_old_and_partially_invalid_settings_keep_safe_defaults() {
        assert_eq!(
            serde_json::from_str::<FastPrepareOverrides>("{}").unwrap(),
            FastPrepareOverrides::default()
        );
        let decoded: FastPrepareOverrides = serde_json::from_str(
            r#"{"jpeg":"Off","png":"future","webp":12,"bmp":null,"gif":"On"}"#,
        )
        .unwrap();
        assert_eq!(decoded.jpeg, FastPrepareOverride::Off);
        assert_eq!(decoded.gif, FastPrepareOverride::On);
        assert_eq!(decoded.png, FastPrepareOverride::Inherit);
        assert_eq!(decoded.webp, FastPrepareOverride::Inherit);
        assert_eq!(decoded.bmp, FastPrepareOverride::Inherit);
        assert_eq!(decoded.enabled_mask(true), 0b11110);
        assert_eq!(decoded.enabled_mask(false), 0);
        assert_eq!(
            serde_json::from_str::<FastPrepareOverrides>(&serde_json::to_string(&decoded).unwrap())
                .unwrap(),
            decoded
        );
    }
}

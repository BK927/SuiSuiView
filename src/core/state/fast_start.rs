use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, Default)]
pub struct FastStartFailureNotice {
    #[serde(default)]
    pub id: String,
    #[serde(default)]
    pub generated_at: String,
    #[serde(default)]
    pub stage: String,
    #[serde(default)]
    pub error: String,
    #[serde(default)]
    pub gpu_name: Option<String>,
    #[serde(default)]
    pub backend: Option<String>,
    #[serde(default)]
    pub device_type: Option<String>,
    #[serde(default)]
    pub diagnostic_path: Option<String>,
    #[serde(default)]
    pub shown: bool,
}

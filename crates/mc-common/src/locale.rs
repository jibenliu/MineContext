use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Locale {
    ZhCn,
    EnUs,
}

impl Locale {
    pub fn from_config(value: &str) -> Self {
        if value.to_ascii_lowercase().starts_with("en") {
            Self::EnUs
        } else {
            Self::ZhCn
        }
    }
}

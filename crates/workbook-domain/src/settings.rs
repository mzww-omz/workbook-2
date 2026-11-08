use serde::{Deserialize, Serialize};
use thiserror::Error;

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum ThemeSetting {
    #[default]
    System,
    Light,
    Dark,
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(default, deny_unknown_fields)]
pub struct Settings {
    pub theme: ThemeSetting,
    pub new_limit: u32,
    pub automatic_rating: bool,
    pub retention: f32,
    pub external_images: bool,
    pub auto_backup: bool,
    /// IANA time zone used for the 4 a.m. study-day boundary.
    pub timezone: String,
}

impl Default for Settings {
    fn default() -> Self {
        Self {
            theme: ThemeSetting::System,
            new_limit: 10,
            automatic_rating: true,
            retention: 0.9,
            external_images: true,
            auto_backup: true,
            timezone: iana_time_zone::get_timezone().unwrap_or_else(|_| "UTC".to_string()),
        }
    }
}

#[derive(Clone, Debug, Error, PartialEq, Eq)]
#[error("{0}")]
pub struct SettingsError(pub String);

impl Settings {
    pub fn validate(&self) -> Result<(), SettingsError> {
        if !(1..=100).contains(&self.new_limit) {
            return Err(SettingsError(
                "新規問題数は1〜100にしてください".to_string(),
            ));
        }
        if !(0.70..=0.97).contains(&self.retention) {
            return Err(SettingsError(
                "目標定着率は0.70〜0.97にしてください".to_string(),
            ));
        }
        self.timezone
            .parse::<chrono_tz::Tz>()
            .map_err(|_| SettingsError(format!("不明なタイムゾーンです: {}", self.timezone)))?;
        Ok(())
    }

    pub fn effective_timezone(&self) -> String {
        if self.timezone.trim().is_empty() {
            iana_time_zone::get_timezone().unwrap_or_else(|_| "UTC".to_string())
        } else {
            self.timezone.clone()
        }
    }

    pub fn to_json(&self) -> String {
        serde_json::to_string(self).expect("settings serialization cannot fail")
    }

    pub fn from_json(raw: &str) -> Result<Self, SettingsError> {
        let settings: Settings =
            serde_json::from_str(raw).map_err(|error| SettingsError(error.to_string()))?;
        settings.validate()?;
        Ok(settings)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn defaults_match_the_product_contract() {
        let settings = Settings {
            timezone: "Asia/Tokyo".to_string(),
            ..Settings::default()
        };
        assert_eq!(settings.new_limit, 10);
        assert_eq!(settings.retention, 0.9);
        assert!(settings.automatic_rating);
        settings.validate().unwrap();
    }

    #[test]
    fn retention_outside_the_supported_range_is_rejected() {
        let settings = Settings {
            retention: 0.99,
            timezone: "UTC".to_string(),
            ..Settings::default()
        };
        assert!(settings.validate().is_err());
    }
}

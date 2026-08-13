//! On-disk configuration (TOML). Parsing is permissive (unknown keys are
//! ignored, forward-compatible with future config sections); values are
//! validated once, at load time, since this is the process's external-input
//! boundary.

use serde::{Deserialize, Serialize};
use std::path::Path;

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(default)]
pub struct WheelConfig {
    /// How many physical wheel detents are required per volume change.
    /// Lower = more sensitive (1 is the most sensitive setting).
    pub notches_per_step: i32,
    /// Reverse the wheel direction (forward lowers volume, back raises it).
    pub invert: bool,
    /// How many volume-key presses (~2% each, OS-dependent) to send per
    /// notch that crosses the threshold. Raise this if a single OS volume
    /// step per detent feels too slow.
    pub sensitivity: i32,
}

impl Default for WheelConfig {
    fn default() -> Self {
        Self {
            notches_per_step: 1,
            invert: false,
            // 1 (one OS volume step, ~2%, per detent) measured too slow on
            // a real MX Master; 2 (~4%) is the confirmed-comfortable value.
            sensitivity: 2,
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(default)]
pub struct GeneralConfig {
    pub autostart: bool,
}

impl Default for GeneralConfig {
    fn default() -> Self {
        Self { autostart: true }
    }
}

/// Linux-only settings. Present on every platform (so the config format
/// stays uniform) but only consulted by `platform::linux`.
#[derive(Debug, Clone, PartialEq, Eq, Default, Serialize, Deserialize)]
#[serde(default)]
pub struct LinuxConfig {
    /// Explicit `/dev/input/eventN` path, overriding auto-detection by
    /// `REL_HWHEEL` capability (useful when more than one wheel-capable
    /// device is present).
    pub device_path: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Eq, Default, Serialize, Deserialize)]
#[serde(default)]
pub struct Config {
    pub wheel: WheelConfig,
    pub general: GeneralConfig,
    pub linux: LinuxConfig,
}

#[derive(Debug)]
pub enum ConfigError {
    Read(std::io::Error),
    Write(std::io::Error),
    Parse(toml::de::Error),
    Serialize(toml::ser::Error),
    InvalidNotchesPerStep(i32),
    InvalidSensitivity(i32),
}

impl std::fmt::Display for ConfigError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            ConfigError::Read(e) => write!(f, "cannot read config file: {e}"),
            ConfigError::Write(e) => write!(f, "cannot write config file: {e}"),
            ConfigError::Parse(e) => write!(f, "invalid config syntax: {e}"),
            ConfigError::Serialize(e) => write!(f, "cannot serialize config: {e}"),
            ConfigError::InvalidNotchesPerStep(n) => {
                write!(f, "wheel.notches_per_step must be positive, got {n}")
            }
            ConfigError::InvalidSensitivity(n) => {
                write!(f, "wheel.sensitivity must be positive, got {n}")
            }
        }
    }
}

impl std::error::Error for ConfigError {}

impl Config {
    pub fn parse(toml_text: &str) -> Result<Self, ConfigError> {
        let config: Config = toml::from_str(toml_text).map_err(ConfigError::Parse)?;
        config.validate()?;
        Ok(config)
    }

    fn validate(&self) -> Result<(), ConfigError> {
        if self.wheel.notches_per_step <= 0 {
            return Err(ConfigError::InvalidNotchesPerStep(
                self.wheel.notches_per_step,
            ));
        }
        if self.wheel.sensitivity <= 0 {
            return Err(ConfigError::InvalidSensitivity(self.wheel.sensitivity));
        }
        Ok(())
    }

    /// Load from `path`, or fall back to defaults when the file is absent
    /// (first run / "install and forget" — a missing config is not an
    /// error). A present-but-malformed file is still rejected.
    pub fn load_or_default(path: &Path) -> Result<Self, ConfigError> {
        match std::fs::read_to_string(path) {
            Ok(text) => Self::parse(&text),
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => Ok(Self::default()),
            Err(e) => Err(ConfigError::Read(e)),
        }
    }

    /// Serialize and write to `path`, creating its parent directory if
    /// needed. Used by the tray menu's live "invert"/"sensitivity" toggles
    /// so a choice made there survives a restart.
    pub fn save(&self, path: &Path) -> Result<(), ConfigError> {
        let text = toml::to_string_pretty(self).map_err(ConfigError::Serialize)?;
        if let Some(parent) = path.parent() {
            std::fs::create_dir_all(parent).map_err(ConfigError::Write)?;
        }
        std::fs::write(path, text).map_err(ConfigError::Write)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::io::Write;

    // --- correctness ---

    #[test]
    fn empty_toml_uses_defaults() {
        let config = Config::parse("").unwrap();
        assert_eq!(config, Config::default());
        assert_eq!(config.wheel.notches_per_step, 1);
        assert!(!config.wheel.invert);
        assert!(config.general.autostart);
    }

    #[test]
    fn explicit_values_are_parsed() {
        let toml = r#"
            [wheel]
            notches_per_step = 3
            invert = true

            [general]
            autostart = false
        "#;
        let config = Config::parse(toml).unwrap();
        assert_eq!(config.wheel.notches_per_step, 3);
        assert!(config.wheel.invert);
        assert!(!config.general.autostart);
    }

    #[test]
    fn sensitivity_is_parsed() {
        let config = Config::parse("[wheel]\nsensitivity = 4\n").unwrap();
        assert_eq!(config.wheel.sensitivity, 4);
    }

    #[test]
    fn sensitivity_defaults_to_two() {
        assert_eq!(Config::default().wheel.sensitivity, 2);
    }

    #[test]
    fn linux_device_path_is_parsed() {
        let config = Config::parse("[linux]\ndevice_path = \"/dev/input/event7\"\n").unwrap();
        assert_eq!(
            config.linux.device_path.as_deref(),
            Some("/dev/input/event7")
        );
    }

    #[test]
    fn linux_device_path_defaults_to_none() {
        let config = Config::parse("").unwrap();
        assert_eq!(config.linux.device_path, None);
    }

    #[test]
    fn partial_section_keeps_other_defaults() {
        let config = Config::parse("[wheel]\ninvert = true\n").unwrap();
        assert_eq!(config.wheel.notches_per_step, 1); // default kept
        assert!(config.wheel.invert);
    }

    #[test]
    fn missing_file_falls_back_to_defaults() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("does-not-exist.toml");
        let config = Config::load_or_default(&path).unwrap();
        assert_eq!(config, Config::default());
    }

    #[test]
    fn present_file_is_loaded() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("config.toml");
        let mut f = std::fs::File::create(&path).unwrap();
        write!(f, "[wheel]\nnotches_per_step = 5\n").unwrap();
        drop(f);

        let config = Config::load_or_default(&path).unwrap();
        assert_eq!(config.wheel.notches_per_step, 5);
    }

    #[test]
    fn saved_config_round_trips_through_load() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("config.toml");
        let mut config = Config::default();
        config.wheel.invert = true;
        config.wheel.sensitivity = 4;

        config.save(&path).unwrap();
        let loaded = Config::load_or_default(&path).unwrap();
        assert_eq!(loaded, config);
    }

    #[test]
    fn save_creates_missing_parent_directory() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("nested").join("config.toml");
        Config::default().save(&path).unwrap();
        assert!(path.exists());
    }

    #[test]
    fn save_overwrites_existing_file() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("config.toml");
        std::fs::write(&path, "not valid toml [[[").unwrap();

        Config::default().save(&path).unwrap();
        let loaded = Config::load_or_default(&path).unwrap();
        assert_eq!(loaded, Config::default());
    }

    // --- rejection: malformed/invalid input at the load boundary ---

    #[test]
    fn malformed_toml_is_rejected_not_defaulted() {
        let err = Config::parse("this is not valid toml [[[").unwrap_err();
        assert!(matches!(err, ConfigError::Parse(_)));
    }

    #[test]
    fn zero_notches_per_step_is_rejected() {
        let err = Config::parse("[wheel]\nnotches_per_step = 0\n").unwrap_err();
        assert!(matches!(err, ConfigError::InvalidNotchesPerStep(0)));
    }

    #[test]
    fn negative_notches_per_step_is_rejected() {
        let err = Config::parse("[wheel]\nnotches_per_step = -2\n").unwrap_err();
        assert!(matches!(err, ConfigError::InvalidNotchesPerStep(-2)));
    }

    #[test]
    fn zero_sensitivity_is_rejected() {
        let err = Config::parse("[wheel]\nsensitivity = 0\n").unwrap_err();
        assert!(matches!(err, ConfigError::InvalidSensitivity(0)));
    }

    #[test]
    fn negative_sensitivity_is_rejected() {
        let err = Config::parse("[wheel]\nsensitivity = -3\n").unwrap_err();
        assert!(matches!(err, ConfigError::InvalidSensitivity(-3)));
    }

    // --- misuse: unknown keys, empty file on disk, wrong type ---

    #[test]
    fn unknown_keys_are_ignored_for_forward_compatibility() {
        let config =
            Config::parse("[wheel]\nnotches_per_step = 2\nsome_future_key = 42\n").unwrap();
        assert_eq!(config.wheel.notches_per_step, 2);
    }

    #[test]
    fn empty_file_on_disk_is_valid_and_uses_defaults() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("config.toml");
        std::fs::File::create(&path).unwrap();
        let config = Config::load_or_default(&path).unwrap();
        assert_eq!(config, Config::default());
    }

    #[test]
    fn wrong_type_is_rejected() {
        let err = Config::parse("[wheel]\nnotches_per_step = \"three\"\n").unwrap_err();
        assert!(matches!(err, ConfigError::Parse(_)));
    }
}

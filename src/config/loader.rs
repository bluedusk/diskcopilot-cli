use std::path::PathBuf;

use anyhow::Result;
use serde::Deserialize;

fn default_min_size() -> String {
    "1M".to_string()
}

fn default_theme() -> String {
    "dark".to_string()
}

fn default_large_file_threshold() -> String {
    "500M".to_string()
}

fn default_recent_days() -> u32 {
    7
}

fn default_old_days() -> u32 {
    365
}

#[derive(Debug, Deserialize, Clone)]
pub struct ScanConfig {
    #[serde(default = "default_min_size")]
    pub default_min_size: String,
}

impl Default for ScanConfig {
    fn default() -> Self {
        Self {
            default_min_size: default_min_size(),
        }
    }
}

#[derive(Debug, Deserialize, Clone)]
pub struct TuiConfig {
    #[serde(default = "default_theme")]
    pub theme: String,
    #[serde(default = "default_large_file_threshold")]
    pub large_file_threshold: String,
    #[serde(default = "default_recent_days")]
    pub recent_days: u32,
    #[serde(default = "default_old_days")]
    pub old_days: u32,
}

impl Default for TuiConfig {
    fn default() -> Self {
        Self {
            theme: default_theme(),
            large_file_threshold: default_large_file_threshold(),
            recent_days: default_recent_days(),
            old_days: default_old_days(),
        }
    }
}

#[derive(Debug, Deserialize, Clone)]
pub struct Config {
    #[serde(default)]
    pub scan: ScanConfig,
    #[serde(default)]
    pub tui: TuiConfig,
}

impl Default for Config {
    fn default() -> Self {
        Self {
            scan: ScanConfig::default(),
            tui: TuiConfig::default(),
        }
    }
}

/// Returns the path to the config file: `~/.diskcopilot/config.toml`
pub fn config_path() -> PathBuf {
    let mut path = dirs::home_dir().unwrap_or_else(|| PathBuf::from("."));
    path.push(".diskcopilot");
    path.push("config.toml");
    path
}

/// Loads config from `~/.diskcopilot/config.toml`, falling back to defaults if not found.
pub fn load_config() -> Result<Config> {
    let path = config_path();
    if !path.exists() {
        return Ok(Config::default());
    }
    let contents = std::fs::read_to_string(&path)?;
    let config: Config = toml::from_str(&contents)?;
    Ok(config)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_default_config_values() {
        let config = Config::default();
        assert_eq!(config.scan.default_min_size, "1M");
        assert_eq!(config.tui.theme, "dark");
        assert_eq!(config.tui.large_file_threshold, "500M");
        assert_eq!(config.tui.recent_days, 7);
        assert_eq!(config.tui.old_days, 365);
    }

    #[test]
    fn test_partial_toml_override_falls_back_to_defaults() {
        let toml_str = r#"
[tui]
theme = "light"
"#;
        let config: Config = toml::from_str(toml_str).expect("failed to parse TOML");
        // Overridden field
        assert_eq!(config.tui.theme, "light");
        // Fields not in TOML should use defaults
        assert_eq!(config.tui.large_file_threshold, "500M");
        assert_eq!(config.tui.recent_days, 7);
        assert_eq!(config.tui.old_days, 365);
        // scan section not present, should use defaults
        assert_eq!(config.scan.default_min_size, "1M");
    }
}

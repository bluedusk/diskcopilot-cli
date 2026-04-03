use std::path::PathBuf;

use anyhow::Result;
use serde::Deserialize;

fn default_min_size() -> String {
    "1M".to_string()
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

#[derive(Debug, Default, Deserialize, Clone)]
pub struct Config {
    #[serde(default)]
    pub scan: ScanConfig,
}

/// Returns the path to the config file: `~/.diskcopilot/config.toml`
pub fn config_path() -> PathBuf {
    let mut path = dirs::home_dir().unwrap_or_else(|| PathBuf::from("."));
    path.push(".diskcopilot");
    path.push("config.toml");
    path
}

/// Loads config from `~/.diskcopilot/config.toml`, falling back to defaults if not found.
// TODO: wire up config loading — currently unused; main.rs uses hardcoded defaults
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
    }

    #[test]
    fn test_partial_toml_override_falls_back_to_defaults() {
        let toml_str = r#"
[scan]
default_min_size = "10M"
"#;
        let config: Config = toml::from_str(toml_str).expect("failed to parse TOML");
        assert_eq!(config.scan.default_min_size, "10M");
    }

    #[test]
    fn test_empty_toml_uses_defaults() {
        let config: Config = toml::from_str("").expect("failed to parse TOML");
        assert_eq!(config.scan.default_min_size, "1M");
    }
}

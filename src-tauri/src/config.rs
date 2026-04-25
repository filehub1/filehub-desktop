use serde::{Deserialize, Serialize};
use std::fs;
use std::path::{Path, PathBuf};

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct AppConfig {
    #[serde(default)]
    pub indexed_directories: Vec<String>,
    #[serde(default)]
    pub use_admin_mode: bool,
    #[serde(default = "default_address")]
    pub address: String,
    #[serde(default)]
    pub exclude_patterns: Vec<String>,
    #[serde(default = "default_theme")]
    pub theme: String,
    #[serde(default = "default_open_mode")]
    pub open_mode: String,
    #[serde(default)]
    pub lan_enabled: bool,
    #[serde(default)]
    pub lan_user: String,
    #[serde(default)]
    pub lan_pass: String,
}

fn default_dirs() -> Vec<String> {
    vec![]
}
fn default_address() -> String { "127.0.0.1:6543".to_string() }
fn default_theme() -> String { "dark".to_string() }
fn default_open_mode() -> String { "local".to_string() }

impl Default for AppConfig {
    fn default() -> Self {
        Self {
            indexed_directories: default_dirs(),
            use_admin_mode: false,
            address: default_address(),
            exclude_patterns: vec![],
            theme: default_theme(),
            open_mode: default_open_mode(),
            lan_enabled: false,
            lan_user: String::new(),
            lan_pass: String::new(),
        }
    }
}

pub fn config_path() -> PathBuf {
    std::env::current_exe()
        .ok()
        .and_then(|p| p.parent().map(|d| d.join("filehub.yml")))
        .unwrap_or_else(|| PathBuf::from("filehub.yml"))
}

pub fn load_config() -> AppConfig {
    let path = config_path();
    if path.exists() {
        if let Ok(content) = fs::read_to_string(&path) {
            if let Ok(cfg) = serde_yaml::from_str::<AppConfig>(&content) {
                return cfg;
            }
        }
    }
    AppConfig::default()
}

pub fn save_config(cfg: &AppConfig) -> std::io::Result<()> {
    let path = config_path();
    let content = serde_yaml::to_string(cfg).unwrap_or_default();
    fs::write(path, content)
}

pub fn parse_address(address: &str) -> (String, u16) {
    if let Some(pos) = address.rfind(':') {
        let host = address[..pos].to_string();
        let port = address[pos + 1..].parse::<u16>().unwrap_or(6543);
        return (host, port);
    }
    ("127.0.0.1".to_string(), 6543)
}

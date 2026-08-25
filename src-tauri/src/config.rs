use std::fs;
use std::path::{Path, PathBuf};

use anyhow::{Context, Result};
use serde::{Deserialize, Serialize};
use tauri::{AppHandle, Manager};

use crate::auth;
use crate::DEFAULT_PORT;

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct AgentConfig {
    pub token: String,
    pub port: u16,
    #[serde(default)]
    pub default_printer_id: Option<String>,
    #[serde(default)]
    pub paired_origin: Option<String>,
    #[serde(default)]
    pub paired_at: Option<String>,
    #[serde(default)]
    pub first_run: bool,
}

impl Default for AgentConfig {
    fn default() -> Self {
        Self {
            token: auth::generate_token(),
            port: DEFAULT_PORT,
            default_printer_id: None,
            paired_origin: None,
            paired_at: None,
            first_run: true,
        }
    }
}

impl AgentConfig {
    pub fn load_or_create(path: &Path) -> Result<Self> {
        if path.exists() {
            let raw = fs::read_to_string(path)
                .with_context(|| format!("read {}", path.display()))?;
            let mut cfg: Self = serde_json::from_str(&raw).context("parse agent config")?;
            if cfg.token.trim().is_empty() {
                cfg.token = auth::generate_token();
            }
            if cfg.port == 0 {
                cfg.port = DEFAULT_PORT;
            }
            cfg.first_run = false;
            cfg.save(path)?;
            return Ok(cfg);
        }

        if let Some(parent) = path.parent() {
            fs::create_dir_all(parent)?;
        }
        let cfg = Self::default();
        cfg.save(path)?;
        Ok(cfg)
    }

    pub fn save(&self, path: &Path) -> Result<()> {
        if let Some(parent) = path.parent() {
            fs::create_dir_all(parent)?;
        }
        let mut store = self.clone();
        store.first_run = false;
        fs::write(path, serde_json::to_string_pretty(&store)?)
            .with_context(|| format!("write {}", path.display()))?;
        Ok(())
    }
}

pub fn config_file_path(app: &AppHandle) -> Result<PathBuf> {
    let dir = app.path().app_config_dir().context("app config dir")?;
    Ok(dir.join("config.json"))
}

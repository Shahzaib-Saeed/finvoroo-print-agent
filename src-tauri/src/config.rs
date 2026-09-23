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
    /// Every Finvoroo origin that has paired or reconnected on this PC.
    /// Pairing is per machine, not per browser URL.
    #[serde(default)]
    pub paired_origins: Vec<String>,
    #[serde(default)]
    pub paired_at: Option<String>,
    #[serde(default)]
    pub first_run: bool,
    /// Last version written by this agent build (used to detect upgrades).
    #[serde(default)]
    pub installed_version: Option<String>,
    /// Version immediately before the current install (shown after an update).
    #[serde(default)]
    pub previous_version: Option<String>,
}

impl Default for AgentConfig {
    fn default() -> Self {
        Self {
            token: auth::generate_token(),
            port: DEFAULT_PORT,
            default_printer_id: None,
            paired_origin: None,
            paired_origins: Vec::new(),
            paired_at: None,
            first_run: true,
            installed_version: None,
            previous_version: None,
        }
    }
}

impl AgentConfig {
    /// Record a version change when the agent binary is newer than config.
    pub fn apply_version_tracking(&mut self, current: &str) -> bool {
        let prior = self.installed_version.clone();
        if prior.as_deref() == Some(current) {
            return false;
        }
        if let Some(old) = prior.filter(|v| v != current) {
            self.previous_version = Some(old);
        }
        self.installed_version = Some(current.to_string());
        true
    }

    pub fn is_paired(&self) -> bool {
        !self.token.trim().is_empty()
            && (self
                .paired_origin
                .as_ref()
                .is_some_and(|origin| !origin.trim().is_empty())
                || self
                    .paired_origins
                    .iter()
                    .any(|origin| !origin.trim().is_empty()))
    }

    pub fn remember_paired_origin(&mut self, origin: &str) {
        let origin = origin.trim();
        if origin.is_empty() {
            return;
        }
        self.paired_origin = Some(origin.to_string());
        if !self
            .paired_origins
            .iter()
            .any(|existing| auth::origins_match_for_reconnect(existing, origin))
        {
            self.paired_origins.push(origin.to_string());
        }
    }

    pub fn clear_pairing(&mut self) {
        self.paired_origin = None;
        self.paired_origins.clear();
        self.paired_at = None;
    }

    fn normalize_pairing(&mut self) {
        if let Some(origin) = self.paired_origin.clone() {
            self.remember_paired_origin(&origin);
        }
        self.paired_origins.retain(|origin| !origin.trim().is_empty());
    }

    pub fn load_or_create(path: &Path) -> Result<Self> {
        if path.exists() {
            let raw = fs::read_to_string(path)
                .with_context(|| format!("read {}", path.display()))?;
            let mut cfg = match serde_json::from_str::<Self>(&raw) {
                Ok(cfg) => cfg,
                Err(err) => {
                    tracing::error!(
                        error = %err,
                        path = %path.display(),
                        "print agent config unreadable; restoring pairing from backup"
                    );
                    let bak = path.with_extension("json.bak");
                    let _ = fs::copy(path, bak);
                    salvage_config(&raw)
                }
            };
            cfg.normalize_pairing();
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

fn salvage_config(raw: &str) -> AgentConfig {
    let mut cfg = AgentConfig::default();
    cfg.first_run = false;
    let Ok(value) = serde_json::from_str::<serde_json::Value>(raw) else {
        return cfg;
    };
    if let Some(token) = value.get("token").and_then(|v| v.as_str()) {
        if !token.trim().is_empty() {
            cfg.token = token.trim().to_string();
        }
    }
    if let Some(port) = value.get("port").and_then(|v| v.as_u64()) {
        if port > 0 && port <= u16::MAX as u64 {
            cfg.port = port as u16;
        }
    }
    if let Some(printer) = value.get("default_printer_id").and_then(|v| v.as_str()) {
        if !printer.trim().is_empty() {
            cfg.default_printer_id = Some(printer.trim().to_string());
        }
    }
    if let Some(origin) = value.get("paired_origin").and_then(|v| v.as_str()) {
        cfg.remember_paired_origin(origin);
    }
    if let Some(origins) = value.get("paired_origins").and_then(|v| v.as_array()) {
        for origin in origins.iter().filter_map(|v| v.as_str()) {
            cfg.remember_paired_origin(origin);
        }
    }
    if let Some(at) = value.get("paired_at").and_then(|v| v.as_str()) {
        cfg.paired_at = Some(at.to_string());
    }
    cfg
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn pairing_survives_origin_list_and_clear() {
        let mut cfg = AgentConfig::default();
        assert!(!cfg.is_paired());
        cfg.remember_paired_origin("https://app.finvoroo.com");
        assert!(cfg.is_paired());
        cfg.remember_paired_origin("http://127.0.0.1:47391");
        assert_eq!(cfg.paired_origins.len(), 2);
        cfg.remember_paired_origin("http://localhost:47391");
        assert_eq!(cfg.paired_origins.len(), 2);
        cfg.clear_pairing();
        assert!(!cfg.is_paired());
        assert!(cfg.paired_origins.is_empty());
    }

    #[test]
    fn salvage_keeps_token_and_origin() {
        let raw = r#"{
            "token": "abc123",
            "port": 17392,
            "paired_origin": "https://app.finvoroo.com",
            "extra_future_field": true
        }"#;
        let cfg = salvage_config(raw);
        assert_eq!(cfg.token, "abc123");
        assert_eq!(cfg.paired_origin.as_deref(), Some("https://app.finvoroo.com"));
        assert!(cfg.is_paired());
    }
}

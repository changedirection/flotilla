use std::collections::HashMap;
use std::path::PathBuf;

use async_trait::async_trait;
use serde::{Deserialize, Serialize};
use tokio::process::Command;
use tracing::info;

use crate::providers::types::*;
#[allow(unused_imports)]
use crate::template::WorkspaceTemplate;

#[derive(Debug, Clone, Serialize, Deserialize, Default)]
struct ZellijState {
    #[serde(default)]
    tabs: HashMap<String, TabState>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
struct TabState {
    working_directory: String,
    created_at: String,
}

pub struct ZellijWorkspaceManager;

impl Default for ZellijWorkspaceManager {
    fn default() -> Self {
        Self::new()
    }
}

impl ZellijWorkspaceManager {
    pub fn new() -> Self {
        Self
    }

    /// Run `zellij action <args>` and return stdout, or an error on failure.
    pub async fn zellij_action(args: &[&str]) -> Result<String, String> {
        let mut cmd_args = vec!["action"];
        cmd_args.extend_from_slice(args);

        let output = Command::new("zellij")
            .args(&cmd_args)
            .stdin(std::process::Stdio::null())
            .output()
            .await
            .map_err(|e| format!("failed to run zellij action: {e}"))?;

        if !output.status.success() {
            let stderr = String::from_utf8_lossy(&output.stderr).trim().to_string();
            let stdout = String::from_utf8_lossy(&output.stdout).trim().to_string();
            return Err(format!(
                "zellij action {} failed: {}",
                args.first().unwrap_or(&""),
                if stderr.is_empty() { &stdout } else { &stderr }
            ));
        }

        Ok(String::from_utf8_lossy(&output.stdout).trim().to_string())
    }

    /// Check that `zellij --version` reports >= 0.40.
    /// Parses output like "zellij 0.42.2".
    pub fn check_version() -> Result<(), String> {
        let output = std::process::Command::new("zellij")
            .arg("--version")
            .stdin(std::process::Stdio::null())
            .output()
            .map_err(|e| format!("failed to run zellij --version: {e}"))?;

        let version_str = String::from_utf8_lossy(&output.stdout).trim().to_string();
        let version_part = version_str
            .strip_prefix("zellij ")
            .ok_or_else(|| format!("unexpected zellij version output: {version_str}"))?;

        let parts: Vec<&str> = version_part.split('.').collect();
        if parts.len() < 2 {
            return Err(format!("cannot parse zellij version: {version_part}"));
        }

        let major: u32 = parts[0]
            .parse()
            .map_err(|_| format!("invalid major version: {}", parts[0]))?;
        let minor: u32 = parts[1]
            .parse()
            .map_err(|_| format!("invalid minor version: {}", parts[1]))?;

        if major == 0 && minor < 40 {
            return Err(format!(
                "zellij >= 0.40 required, found {version_part}"
            ));
        }

        info!("zellij version {version_part} OK");
        Ok(())
    }

    /// Return the current Zellij session name from the environment.
    pub fn session_name() -> Result<String, String> {
        std::env::var("ZELLIJ_SESSION_NAME")
            .map_err(|_| "not running inside a zellij session (ZELLIJ_SESSION_NAME not set)".to_string())
    }

    /// Return the state file path: `~/.config/flotilla/zellij/{session}/state.toml`.
    pub fn state_path(session: &str) -> Result<PathBuf, String> {
        let config_dir = dirs::config_dir()
            .ok_or_else(|| "could not determine config directory".to_string())?;
        Ok(config_dir
            .join("flotilla")
            .join("zellij")
            .join(session)
            .join("state.toml"))
    }

    /// Load persisted state for the given session. Returns default on any error.
    fn load_state(session: &str) -> ZellijState {
        let path = match Self::state_path(session) {
            Ok(p) => p,
            Err(_) => return ZellijState::default(),
        };
        let contents = match std::fs::read_to_string(&path) {
            Ok(c) => c,
            Err(_) => return ZellijState::default(),
        };
        toml::from_str(&contents).unwrap_or_default()
    }

    /// Save state for the given session. Silently ignores errors.
    #[allow(dead_code)]
    fn save_state(session: &str, state: &ZellijState) {
        let path = match Self::state_path(session) {
            Ok(p) => p,
            Err(_) => return,
        };
        if let Some(parent) = path.parent() {
            let _ = std::fs::create_dir_all(parent);
        }
        if let Ok(contents) = toml::to_string(state) {
            let _ = std::fs::write(&path, contents);
        }
    }
}

#[async_trait]
impl super::WorkspaceManager for ZellijWorkspaceManager {
    fn display_name(&self) -> &str {
        "zellij Workspaces"
    }

    async fn list_workspaces(&self) -> Result<Vec<Workspace>, String> {
        let output = Self::zellij_action(&["query-tab-names"]).await?;
        let tab_names: Vec<&str> = output.lines().filter(|l| !l.is_empty()).collect();

        // Try to load state for enrichment
        let state = Self::session_name()
            .map(|s| Self::load_state(&s))
            .unwrap_or_default();

        let workspaces = tab_names
            .into_iter()
            .map(|name| {
                let mut directories = Vec::new();
                let mut correlation_keys = Vec::new();

                if let Some(tab) = state.tabs.get(name) {
                    let path = PathBuf::from(&tab.working_directory);
                    correlation_keys.push(CorrelationKey::CheckoutPath(path.clone()));
                    directories.push(path);
                }

                Workspace {
                    ws_ref: name.to_string(),
                    name: name.to_string(),
                    directories,
                    correlation_keys,
                }
            })
            .collect();

        Ok(workspaces)
    }

    async fn create_workspace(&self, _config: &WorkspaceConfig) -> Result<Workspace, String> {
        todo!()
    }

    async fn select_workspace(&self, ws_ref: &str) -> Result<(), String> {
        info!("zellij: switching to tab '{ws_ref}'");
        Self::zellij_action(&["go-to-tab-name", ws_ref]).await?;
        Ok(())
    }
}

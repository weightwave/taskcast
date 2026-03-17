use serde::{Deserialize, Serialize};
use std::collections::HashMap;
use std::path::PathBuf;

const DEFAULT_URL: &str = "http://localhost:3721";

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct NodeEntry {
    pub url: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub token: Option<String>,
    #[serde(rename = "tokenType", skip_serializing_if = "Option::is_none")]
    pub token_type: Option<TokenType>,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
#[serde(rename_all = "camelCase")]
pub enum TokenType {
    Jwt,
    Admin,
}

#[derive(Debug, Clone)]
pub struct NodeListEntry {
    pub name: String,
    pub entry: NodeEntry,
    pub current: bool,
}

#[derive(Debug, Default, Serialize, Deserialize)]
struct NodeConfigData {
    current: Option<String>,
    nodes: HashMap<String, NodeEntry>,
}

pub struct NodeConfigManager {
    config_path: PathBuf,
}

impl NodeConfigManager {
    pub fn new(config_dir: PathBuf) -> Self {
        Self {
            config_path: config_dir.join("nodes.json"),
        }
    }

    /// Returns the currently selected node, or a default localhost entry if none is set.
    pub fn get_current(&self) -> NodeEntry {
        let data = self.load();
        if let Some(ref name) = data.current {
            if let Some(entry) = data.nodes.get(name) {
                return entry.clone();
            }
        }
        NodeEntry {
            url: DEFAULT_URL.to_string(),
            token: None,
            token_type: None,
        }
    }

    /// Get a node by name.
    pub fn get(&self, name: &str) -> Option<NodeEntry> {
        let data = self.load();
        data.nodes.get(name).cloned()
    }

    /// Add or overwrite a named node.
    pub fn add(&self, name: &str, entry: NodeEntry) {
        let mut data = self.load();
        data.nodes.insert(name.to_string(), entry);
        self.save(&data);
    }

    /// Remove a named node. Returns an error if the node does not exist.
    /// If the removed node was the current node, current is reset to None.
    pub fn remove(&self, name: &str) -> Result<(), String> {
        let mut data = self.load();
        if !data.nodes.contains_key(name) {
            return Err(format!("Node \"{name}\" not found"));
        }
        data.nodes.remove(name);
        if data.current.as_deref() == Some(name) {
            data.current = None;
        }
        self.save(&data);
        Ok(())
    }

    /// Set the current active node. Returns an error if the node does not exist.
    pub fn set_current(&self, name: &str) -> Result<(), String> {
        let mut data = self.load();
        if !data.nodes.contains_key(name) {
            return Err(format!("Node \"{name}\" not found"));
        }
        data.current = Some(name.to_string());
        self.save(&data);
        Ok(())
    }

    /// List all nodes with a `current` marker.
    pub fn list(&self) -> Vec<NodeListEntry> {
        let data = self.load();
        let mut entries: Vec<NodeListEntry> = data
            .nodes
            .into_iter()
            .map(|(name, entry)| {
                let current = data.current.as_deref() == Some(&name);
                NodeListEntry {
                    name,
                    entry,
                    current,
                }
            })
            .collect();
        entries.sort_by(|a, b| a.name.cmp(&b.name));
        entries
    }

    fn load(&self) -> NodeConfigData {
        match std::fs::read_to_string(&self.config_path) {
            Ok(raw) => serde_json::from_str(&raw).unwrap_or_default(),
            Err(_) => NodeConfigData::default(),
        }
    }

    fn save(&self, data: &NodeConfigData) {
        if let Some(parent) = self.config_path.parent() {
            let _ = std::fs::create_dir_all(parent);
        }
        let json = serde_json::to_string_pretty(data).expect("failed to serialize node config");
        std::fs::write(&self.config_path, &json).expect("failed to write node config");

        // Restrict file permissions on Unix (nodes.json may contain tokens)
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            let perms = std::fs::Permissions::from_mode(0o600);
            let _ = std::fs::set_permissions(&self.config_path, perms);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use tempfile::TempDir;

    fn temp_manager() -> (TempDir, NodeConfigManager) {
        let dir = TempDir::new().unwrap();
        let mgr = NodeConfigManager::new(dir.path().to_path_buf());
        (dir, mgr)
    }

    #[test]
    fn default_localhost_when_no_config() {
        let (_dir, mgr) = temp_manager();
        let current = mgr.get_current();
        assert_eq!(current.url, "http://localhost:3721");
        assert_eq!(current.token, None);
        assert_eq!(current.token_type, None);
    }

    #[test]
    fn add_and_get() {
        let (_dir, mgr) = temp_manager();
        mgr.add(
            "prod",
            NodeEntry {
                url: "https://tc.example.com".to_string(),
                token: Some("ey...".to_string()),
                token_type: Some(TokenType::Jwt),
            },
        );
        let node = mgr.get("prod").expect("node should exist");
        assert_eq!(node.url, "https://tc.example.com");
        assert_eq!(node.token, Some("ey...".to_string()));
        assert_eq!(node.token_type, Some(TokenType::Jwt));
    }

    #[test]
    fn set_current_and_get_current() {
        let (_dir, mgr) = temp_manager();
        mgr.add(
            "prod",
            NodeEntry {
                url: "https://tc.example.com".to_string(),
                token: Some("tok".to_string()),
                token_type: Some(TokenType::Jwt),
            },
        );
        mgr.set_current("prod").unwrap();
        let current = mgr.get_current();
        assert_eq!(current.url, "https://tc.example.com");
        assert_eq!(current.token, Some("tok".to_string()));
    }

    #[test]
    fn remove_node() {
        let (_dir, mgr) = temp_manager();
        mgr.add(
            "prod",
            NodeEntry {
                url: "https://tc.example.com".to_string(),
                token: None,
                token_type: None,
            },
        );
        mgr.remove("prod").unwrap();
        assert!(mgr.get("prod").is_none());
    }

    #[test]
    fn list_with_current_marker() {
        let (_dir, mgr) = temp_manager();
        mgr.add(
            "local",
            NodeEntry {
                url: "http://localhost:3721".to_string(),
                token: None,
                token_type: None,
            },
        );
        mgr.add(
            "prod",
            NodeEntry {
                url: "https://tc.example.com".to_string(),
                token: Some("tok".to_string()),
                token_type: Some(TokenType::Jwt),
            },
        );
        mgr.set_current("prod").unwrap();
        let list = mgr.list();
        assert_eq!(list.len(), 2);

        let local_entry = list.iter().find(|n| n.name == "local").unwrap();
        assert!(!local_entry.current);

        let prod_entry = list.iter().find(|n| n.name == "prod").unwrap();
        assert!(prod_entry.current);
        assert_eq!(prod_entry.entry.url, "https://tc.example.com");
        assert_eq!(prod_entry.entry.token, Some("tok".to_string()));
        assert_eq!(prod_entry.entry.token_type, Some(TokenType::Jwt));
    }

    #[test]
    fn error_on_use_nonexistent() {
        let (_dir, mgr) = temp_manager();
        let result = mgr.set_current("ghost");
        assert!(result.is_err());
        assert!(result.unwrap_err().contains("not found"));
    }

    #[test]
    fn error_on_remove_nonexistent() {
        let (_dir, mgr) = temp_manager();
        let result = mgr.remove("ghost");
        assert!(result.is_err());
        assert!(result.unwrap_err().contains("not found"));
    }

    #[test]
    fn reset_current_when_removing_current_node() {
        let (_dir, mgr) = temp_manager();
        mgr.add(
            "prod",
            NodeEntry {
                url: "https://tc.example.com".to_string(),
                token: None,
                token_type: None,
            },
        );
        mgr.set_current("prod").unwrap();
        assert_eq!(mgr.get_current().url, "https://tc.example.com");
        mgr.remove("prod").unwrap();
        // After removing current, should fall back to default
        let current = mgr.get_current();
        assert_eq!(current.url, "http://localhost:3721");
    }

    #[test]
    fn persistence_across_instances() {
        let dir = TempDir::new().unwrap();
        let path = dir.path().to_path_buf();

        let mgr1 = NodeConfigManager::new(path.clone());
        mgr1.add(
            "staging",
            NodeEntry {
                url: "https://s.tc.io".to_string(),
                token: Some("admin_xxx".to_string()),
                token_type: Some(TokenType::Admin),
            },
        );
        mgr1.set_current("staging").unwrap();

        // Create a brand new instance pointing at same config dir
        let mgr2 = NodeConfigManager::new(path);
        let current = mgr2.get_current();
        assert_eq!(current.url, "https://s.tc.io");
        assert_eq!(current.token, Some("admin_xxx".to_string()));
        assert_eq!(current.token_type, Some(TokenType::Admin));

        let node = mgr2.get("staging").expect("node should exist");
        assert_eq!(node.url, "https://s.tc.io");
    }
}

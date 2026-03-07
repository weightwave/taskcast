use clap::Subcommand;

use crate::node_config::{NodeConfigManager, NodeEntry, NodeListEntry, TokenType};

#[derive(Subcommand)]
pub enum NodeCommands {
    /// Add a Taskcast server node
    Add {
        name: String,
        #[arg(long)]
        url: String,
        #[arg(long)]
        token: Option<String>,
        /// Token type: jwt or admin
        #[arg(long, default_value = "jwt")]
        token_type: String,
    },
    /// Remove a node
    Remove { name: String },
    /// Set the default node
    Use { name: String },
    /// List all nodes
    List,
}

/// Format the node list for display. Matches TypeScript output format.
pub fn format_node_list(nodes: &[NodeListEntry]) -> String {
    if nodes.is_empty() {
        return "No nodes configured. Using default: http://localhost:3721".to_string();
    }

    nodes
        .iter()
        .map(|n| {
            let marker = if n.current { "*" } else { " " };
            let token_info = match &n.entry.token_type {
                Some(TokenType::Jwt) => " (jwt)".to_string(),
                Some(TokenType::Admin) => " (admin)".to_string(),
                None => String::new(),
            };
            format!("{} {}  {}{}", marker, n.name, n.entry.url, token_info)
        })
        .collect::<Vec<_>>()
        .join("\n")
}

fn get_config_manager() -> NodeConfigManager {
    let config_dir = dirs::home_dir()
        .expect("could not determine home directory")
        .join(".taskcast");
    NodeConfigManager::new(config_dir)
}

pub fn run(command: NodeCommands) {
    let mgr = get_config_manager();

    match command {
        NodeCommands::Add {
            name,
            url,
            token,
            token_type,
        } => {
            let tt = if token.is_some() {
                match token_type.as_str() {
                    "admin" => Some(TokenType::Admin),
                    _ => Some(TokenType::Jwt),
                }
            } else {
                None
            };
            mgr.add(
                &name,
                NodeEntry {
                    url: url.clone(),
                    token,
                    token_type: tt,
                },
            );
            println!("Added node \"{name}\" -> {url}");
        }
        NodeCommands::Remove { name } => match mgr.remove(&name) {
            Ok(()) => println!("Removed node \"{name}\""),
            Err(e) => {
                eprintln!("Error: {e}");
                std::process::exit(1);
            }
        },
        NodeCommands::Use { name } => match mgr.set_current(&name) {
            Ok(()) => println!("Switched to node \"{name}\""),
            Err(e) => {
                eprintln!("Error: {e}");
                std::process::exit(1);
            }
        },
        NodeCommands::List => {
            let nodes = mgr.list();
            println!("{}", format_node_list(&nodes));
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn format_empty_list() {
        let result = format_node_list(&[]);
        assert_eq!(
            result,
            "No nodes configured. Using default: http://localhost:3721"
        );
    }

    #[test]
    fn format_list_with_current_marker() {
        let nodes = vec![
            NodeListEntry {
                name: "prod".to_string(),
                entry: NodeEntry {
                    url: "https://tc.example.com".to_string(),
                    token: Some("tok".to_string()),
                    token_type: Some(TokenType::Jwt),
                },
                current: true,
            },
            NodeListEntry {
                name: "local".to_string(),
                entry: NodeEntry {
                    url: "http://localhost:3721".to_string(),
                    token: None,
                    token_type: None,
                },
                current: false,
            },
        ];
        let result = format_node_list(&nodes);
        assert_eq!(
            result,
            "* prod  https://tc.example.com (jwt)\n  local  http://localhost:3721"
        );
    }

    #[test]
    fn format_list_admin_token_type() {
        let nodes = vec![NodeListEntry {
            name: "staging".to_string(),
            entry: NodeEntry {
                url: "https://s.tc.io".to_string(),
                token: Some("admin_xxx".to_string()),
                token_type: Some(TokenType::Admin),
            },
            current: false,
        }];
        let result = format_node_list(&nodes);
        assert_eq!(result, "  staging  https://s.tc.io (admin)");
    }

    #[test]
    fn format_list_no_token_type() {
        let nodes = vec![NodeListEntry {
            name: "local".to_string(),
            entry: NodeEntry {
                url: "http://localhost:3721".to_string(),
                token: None,
                token_type: None,
            },
            current: false,
        }];
        let result = format_node_list(&nodes);
        assert_eq!(result, "  local  http://localhost:3721");
    }

    #[test]
    fn format_list_single_current_node() {
        let nodes = vec![NodeListEntry {
            name: "prod".to_string(),
            entry: NodeEntry {
                url: "https://tc.example.com".to_string(),
                token: Some("ey...".to_string()),
                token_type: Some(TokenType::Jwt),
            },
            current: true,
        }];
        let result = format_node_list(&nodes);
        assert!(result.starts_with("* "));
        assert!(result.contains("prod"));
        assert!(result.contains("https://tc.example.com"));
        assert!(result.contains("(jwt)"));
    }
}

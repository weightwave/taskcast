use serde::Serialize;

use crate::node_config::{NodeEntry, TokenType};

fn default_client() -> reqwest::Client {
    reqwest::Client::builder()
        .timeout(std::time::Duration::from_secs(30))
        .connect_timeout(std::time::Duration::from_secs(10))
        .build()
        .unwrap()
}

/// HTTP client for communicating with a Taskcast server.
///
/// Wraps `reqwest::Client` with base URL and optional auth token handling.
/// Supports admin token exchange: when the node uses an admin token, the
/// client first POSTs to `/admin/token` to obtain a JWT for subsequent requests.
pub struct TaskcastClient {
    http: reqwest::Client,
    base_url: String,
    token: Option<String>,
}

#[derive(serde::Deserialize)]
struct AdminTokenResponse {
    token: String,
    #[serde(rename = "expiresAt")]
    #[allow(dead_code)]
    expires_at: Option<u64>,
}

#[allow(dead_code)]
impl TaskcastClient {
    /// Create a client directly with optional token.
    pub fn new(base_url: String, token: Option<String>) -> Self {
        Self {
            http: default_client(),
            base_url: base_url.trim_end_matches('/').to_string(),
            token,
        }
    }

    /// Create a client from a NodeEntry. For admin tokens, exchanges via /admin/token first.
    pub async fn from_node(node: &NodeEntry) -> Result<Self, Box<dyn std::error::Error>> {
        let base_url = node.url.trim_end_matches('/').to_string();

        if node.token_type == Some(TokenType::Admin) {
            if let Some(ref admin_token) = node.token {
                let http = default_client();
                let res = http
                    .post(format!("{base_url}/admin/token"))
                    .json(&serde_json::json!({ "adminToken": admin_token }))
                    .send()
                    .await?;

                if !res.status().is_success() {
                    let status = res.status();
                    let message = match res.json::<serde_json::Value>().await {
                        Ok(body) => body
                            .get("error")
                            .and_then(|e| e.as_str())
                            .map(|s| s.to_string())
                            .unwrap_or_else(|| format!("HTTP {status}")),
                        Err(_) => format!("HTTP {status}"),
                    };
                    return Err(message.into());
                }

                let body: AdminTokenResponse = res.json().await?;
                return Ok(Self {
                    http,
                    base_url,
                    token: Some(body.token),
                });
            }
        }

        let token = node.token.clone();

        Ok(Self {
            http: default_client(),
            base_url,
            token,
        })
    }

    /// Send a GET request to the given path.
    pub async fn get(&self, path: &str) -> Result<reqwest::Response, reqwest::Error> {
        let url = format!("{}{}", self.base_url, path);
        let mut req = self.http.get(&url);
        if let Some(ref token) = self.token {
            req = req.bearer_token(token);
        }
        req.send().await
    }

    /// Send a POST request with a JSON body.
    pub async fn post(
        &self,
        path: &str,
        body: &impl Serialize,
    ) -> Result<reqwest::Response, reqwest::Error> {
        let url = format!("{}{}", self.base_url, path);
        let mut req = self.http.post(&url).json(body);
        if let Some(ref token) = self.token {
            req = req.bearer_token(token);
        }
        req.send().await
    }

    /// Send a PATCH request with a JSON body.
    pub async fn patch(
        &self,
        path: &str,
        body: &impl Serialize,
    ) -> Result<reqwest::Response, reqwest::Error> {
        let url = format!("{}{}", self.base_url, path);
        let mut req = self.http.patch(&url).json(body);
        if let Some(ref token) = self.token {
            req = req.bearer_token(token);
        }
        req.send().await
    }

    /// Get the base URL this client targets.
    pub fn base_url(&self) -> &str {
        &self.base_url
    }

    /// Get the token this client uses for auth (if any).
    pub fn token(&self) -> Option<&str> {
        self.token.as_deref()
    }
}

/// Extension trait for adding bearer token to reqwest builders.
trait BearerToken {
    fn bearer_token(self, token: &str) -> Self;
}

impl BearerToken for reqwest::RequestBuilder {
    fn bearer_token(self, token: &str) -> Self {
        self.header("Authorization", format!("Bearer {token}"))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn new_constructs_correctly() {
        let client = TaskcastClient::new(
            "http://localhost:3721".to_string(),
            Some("my-token".to_string()),
        );
        assert_eq!(client.base_url(), "http://localhost:3721");
        assert_eq!(client.token(), Some("my-token"));
    }

    #[test]
    fn new_without_token() {
        let client = TaskcastClient::new("http://localhost:3721".to_string(), None);
        assert_eq!(client.base_url(), "http://localhost:3721");
        assert_eq!(client.token(), None);
    }

    #[test]
    fn new_strips_trailing_slash() {
        let client =
            TaskcastClient::new("http://localhost:3721/".to_string(), None);
        assert_eq!(client.base_url(), "http://localhost:3721");
    }

    #[tokio::test]
    async fn from_node_jwt_sets_token() {
        let node = NodeEntry {
            url: "http://localhost:3721".to_string(),
            token: Some("jwt-token".to_string()),
            token_type: Some(TokenType::Jwt),
        };
        let client = TaskcastClient::from_node(&node).await.unwrap();
        assert_eq!(client.base_url(), "http://localhost:3721");
        assert_eq!(client.token(), Some("jwt-token"));
    }

    #[tokio::test]
    async fn from_node_no_auth() {
        let node = NodeEntry {
            url: "http://localhost:3721".to_string(),
            token: None,
            token_type: None,
        };
        let client = TaskcastClient::from_node(&node).await.unwrap();
        assert_eq!(client.base_url(), "http://localhost:3721");
        assert_eq!(client.token(), None);
    }
}

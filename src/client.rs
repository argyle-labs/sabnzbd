//! SABnzbd API client (`/api?mode=...&output=json&apikey=...`).

use plugin_toolkit::http::Client as HttpClient;
use plugin_toolkit::prelude::*;
use plugin_toolkit::serde_json::Value;

pub struct SabClient {
    http: HttpClient,
    base: String,
    api_key: String,
}

fn encode(s: &str) -> String {
    s.bytes()
        .map(|b| match b {
            b'A'..=b'Z' | b'a'..=b'z' | b'0'..=b'9' | b'-' | b'_' | b'.' | b'~' => {
                (b as char).to_string()
            }
            b => format!("%{b:02X}"),
        })
        .collect()
}

impl SabClient {
    pub fn new(base_url: &str, api_key: &str) -> Self {
        Self {
            http: HttpClient::new(),
            base: base_url.trim_end_matches('/').to_string(),
            api_key: api_key.to_string(),
        }
    }

    /// One API call. SABnzbd reports failures as `{"status": false, "error": ...}`
    /// with HTTP 200, so that shape is an error too.
    pub async fn api(&self, mode: &str, params: &[(&str, &str)]) -> Result<Value> {
        let mut url = format!(
            "{}/api?mode={}&output=json&apikey={}",
            self.base,
            encode(mode),
            encode(&self.api_key)
        );
        for (k, v) in params {
            url.push_str(&format!("&{}={}", encode(k), encode(v)));
        }
        let resp = self
            .http
            .get(url)
            .send()
            .await
            .map_err(|e| anyhow!("sabnzbd {mode}: {e}"))?;
        let v: Value = resp
            .json()
            .map_err(|e| anyhow!("sabnzbd {mode}: decode: {e}"))?;
        if v.get("status") == Some(&Value::Bool(false)) {
            let err = v
                .get("error")
                .and_then(Value::as_str)
                .unwrap_or("unknown error");
            bail!("sabnzbd {mode}: {err}");
        }
        Ok(v)
    }

    pub async fn config_section(&self, section: &str) -> Result<Value> {
        let v = self.api("get_config", &[("section", section)]).await?;
        v.pointer(&format!("/config/{section}"))
            .cloned()
            .ok_or_else(|| anyhow!("sabnzbd get_config: no `{section}` section"))
    }

    /// Warning texts. Newer releases return objects with `text`, older plain strings.
    pub async fn warnings(&self) -> Result<Vec<String>> {
        let v = self.api("warnings", &[]).await?;
        Ok(v.get("warnings")
            .and_then(Value::as_array)
            .map(|a| {
                a.iter()
                    .filter_map(|w| {
                        w.as_str()
                            .or_else(|| w.get("text").and_then(Value::as_str))
                            .map(str::to_string)
                    })
                    .collect()
            })
            .unwrap_or_default())
    }

    pub async fn set_server_connections(&self, server: &str, connections: u32) -> Result<()> {
        let n = connections.to_string();
        self.api(
            "set_config",
            &[
                ("section", "servers"),
                ("keyword", server),
                ("connections", &n),
            ],
        )
        .await?;
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use plugin_toolkit::serde_json::json;
    use wiremock::matchers::{method, path, query_param};
    use wiremock::{Mock, MockServer, ResponseTemplate};

    #[tokio::test]
    async fn api_sends_key_and_params_and_surfaces_errors() {
        let s = MockServer::start().await;
        Mock::given(method("GET"))
            .and(path("/api"))
            .and(query_param("mode", "get_config"))
            .and(query_param("section", "misc"))
            .and(query_param("apikey", "k y"))
            .respond_with(
                ResponseTemplate::new(200)
                    .set_body_json(json!({"config": {"misc": {"download_dir": "/incomplete"}}})),
            )
            .mount(&s)
            .await;
        Mock::given(method("GET"))
            .and(path("/api"))
            .and(query_param("mode", "warnings"))
            .respond_with(ResponseTemplate::new(200).set_body_json(
                json!({"warnings": ["old style", {"text": "new style", "type": "WARNING"}]}),
            ))
            .mount(&s)
            .await;
        Mock::given(method("GET"))
            .and(path("/api"))
            .and(query_param("mode", "set_config"))
            .respond_with(
                ResponseTemplate::new(200)
                    .set_body_json(json!({"status": false, "error": "API Key Incorrect"})),
            )
            .mount(&s)
            .await;
        let c = SabClient::new(&format!("{}/", s.uri()), "k y");
        let misc = c.config_section("misc").await.unwrap();
        assert_eq!(misc["download_dir"], "/incomplete");
        assert_eq!(c.warnings().await.unwrap(), vec!["old style", "new style"]);
        let err = c.set_server_connections("eweka", 20).await.unwrap_err();
        assert!(err.to_string().contains("API Key Incorrect"));
    }
}

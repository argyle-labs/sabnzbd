//! SABnzbd API client: form-encoded POSTs to `/api`. The API key travels in
//! the body (SABnzbd reads `apikey` from query or form, never a header), so it
//! never appears in a URL or in an error that quotes one.

use plugin_toolkit::http::Client as HttpClient;
use plugin_toolkit::prelude::*;
use plugin_toolkit::serde_json::Value;

pub struct SabClient {
    http: HttpClient,
    base: String,
    api_key: String,
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
        let mut form: Vec<(String, String)> = vec![
            ("mode".into(), mode.into()),
            ("output".into(), "json".into()),
            ("apikey".into(), self.api_key.clone()),
        ];
        form.extend(params.iter().map(|(k, v)| (k.to_string(), v.to_string())));
        let resp = self
            .http
            .post(format!("{}/api", self.base))
            .form(form)
            .send()
            .await
            .map_err(|e| anyhow!("sabnzbd {mode}: {}", self.scrub(&e.to_string())))?;
        let v: Value = resp
            .json()
            .map_err(|e| anyhow!("sabnzbd {mode}: decode: {}", self.scrub(&e.to_string())))?;
        if v.get("status") == Some(&Value::Bool(false)) {
            let err = v
                .get("error")
                .and_then(Value::as_str)
                .unwrap_or("unknown error");
            bail!("sabnzbd {mode}: {}", self.scrub(err));
        }
        Ok(v)
    }

    /// `text` with the API key replaced, in case a server or transport error
    /// echoes the request.
    fn scrub(&self, text: &str) -> String {
        if self.api_key.is_empty() {
            return text.to_string();
        }
        text.replace(&self.api_key, "<redacted>")
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
    use wiremock::matchers::{body_string_contains, method, path};
    use wiremock::{Mock, MockServer, ResponseTemplate};

    #[tokio::test]
    async fn api_sends_key_and_params_and_surfaces_errors() {
        let s = MockServer::start().await;
        Mock::given(method("POST"))
            .and(path("/api"))
            .and(body_string_contains("mode=get_config"))
            .and(body_string_contains("section=misc"))
            .and(body_string_contains("apikey=k+y"))
            .respond_with(
                ResponseTemplate::new(200)
                    .set_body_json(json!({"config": {"misc": {"download_dir": "/incomplete"}}})),
            )
            .mount(&s)
            .await;
        Mock::given(method("POST"))
            .and(path("/api"))
            .and(body_string_contains("mode=warnings"))
            .respond_with(ResponseTemplate::new(200).set_body_json(
                json!({"warnings": ["old style", {"text": "new style", "type": "WARNING"}]}),
            ))
            .mount(&s)
            .await;
        Mock::given(method("POST"))
            .and(path("/api"))
            .and(body_string_contains("mode=set_config"))
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

    #[tokio::test]
    async fn errors_never_carry_the_api_key() {
        // Nothing listens on port 9 here: a refused connection.
        let c = SabClient::new("http://127.0.0.1:9", "s3cr3t-key");
        let err = c.warnings().await.unwrap_err().to_string();
        assert!(!err.contains("s3cr3t-key"), "{err}");
        let s = MockServer::start().await;
        Mock::given(method("POST"))
            .and(path("/api"))
            .respond_with(
                ResponseTemplate::new(200)
                    .set_body_json(json!({"status": false, "error": "bad key s3cr3t-key"})),
            )
            .mount(&s)
            .await;
        let err = SabClient::new(&s.uri(), "s3cr3t-key")
            .warnings()
            .await
            .unwrap_err()
            .to_string();
        assert!(
            !err.contains("s3cr3t-key") && err.contains("<redacted>"),
            "{err}"
        );
    }

    #[tokio::test]
    async fn the_key_goes_in_the_body_not_the_url() {
        let s = MockServer::start().await;
        Mock::given(method("POST"))
            .and(path("/api"))
            .respond_with(ResponseTemplate::new(200).set_body_json(json!({"warnings": []})))
            .mount(&s)
            .await;
        SabClient::new(&s.uri(), "k").warnings().await.unwrap();
        let req = &s.received_requests().await.unwrap()[0];
        assert!(req.url.query().is_none());
        assert!(String::from_utf8_lossy(&req.body).contains("apikey=k"));
    }
}

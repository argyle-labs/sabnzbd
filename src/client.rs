//! SABnzbd API client: form-encoded POSTs to `/api`. The API key travels in
//! the body (SABnzbd reads `apikey` from query or form, never a header), so it
//! never appears in a URL or in an error that quotes one.
//!
//! Every error text passes through [`SabClient::scrub`]: the API key and any
//! sensitive value seen in a `get_config` reply (server passwords) are replaced
//! by exact match, then the stack-wide pattern scrub runs.

use std::sync::Mutex;

use plugin_toolkit::http::Client as HttpClient;
use plugin_toolkit::prelude::*;
use plugin_toolkit::scrub;
use plugin_toolkit::serde_json::Value;

pub struct SabClient {
    http: HttpClient,
    base: String,
    api_key: String,
    /// Sensitive values seen in `get_config` replies.
    seen_secrets: Mutex<Vec<String>>,
}

impl SabClient {
    pub fn new(base_url: &str, api_key: &str) -> Self {
        Self {
            http: HttpClient::new(),
            base: base_url.trim_end_matches('/').to_string(),
            api_key: api_key.to_string(),
            seen_secrets: Mutex::new(Vec::new()),
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

    /// `text` with every known secret replaced, in case a server or transport
    /// error echoes the request or a config value.
    pub fn scrub(&self, text: &str) -> String {
        let mut out = text.to_string();
        let seen = self.seen_secrets.lock().unwrap_or_else(|e| e.into_inner());
        for secret in std::iter::once(&self.api_key).chain(seen.iter()) {
            if !secret.is_empty() {
                out = out.replace(secret.as_str(), scrub::REDACTED);
            }
        }
        plugin_toolkit::logging::scrub(&out).into_owned()
    }

    fn remember_secrets(&self, v: &Value) {
        let mut found = Vec::new();
        collect_secrets(v, &mut found);
        if found.is_empty() {
            return;
        }
        let mut seen = self.seen_secrets.lock().unwrap_or_else(|e| e.into_inner());
        for f in found {
            if !seen.contains(&f) {
                seen.push(f);
            }
        }
    }

    /// One config section (`misc`, `servers`, ...). Unredacted: callers pick
    /// the fields they report. Its sensitive values are scrubbed from every
    /// later error of this client.
    pub async fn get_config(&self, section: &str) -> Result<Value> {
        let v = self.api("get_config", &[("section", section)]).await?;
        self.remember_secrets(&v);
        v.pointer(&format!("/config/{section}"))
            .cloned()
            .ok_or_else(|| anyhow!("sabnzbd get_config: no `{section}` section"))
    }

    /// Set `values` on `section`, scoped to the entry `keyword` names (a
    /// server's name), or on the section itself when `keyword` is `None`
    /// (e.g. `misc` with `("download_dir", ..)`).
    pub async fn set_config(
        &self,
        section: &str,
        keyword: Option<&str>,
        values: &[(&str, &str)],
    ) -> Result<()> {
        let mut params = vec![("section", section)];
        if let Some(k) = keyword {
            params.push(("keyword", k));
        }
        params.extend_from_slice(values);
        self.api("set_config", &params).await?;
        Ok(())
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
        self.set_config("servers", Some(server), &[("connections", &n)])
            .await
    }
}

fn collect_secrets(v: &Value, out: &mut Vec<String>) {
    match v {
        Value::Object(m) => {
            for (k, val) in m {
                match val {
                    Value::String(s) if !s.is_empty() && scrub::is_sensitive_key(k) => {
                        out.push(s.clone())
                    }
                    _ => collect_secrets(val, out),
                }
            }
        }
        Value::Array(a) => a.iter().for_each(|x| collect_secrets(x, out)),
        _ => {}
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
        let misc = c.get_config("misc").await.unwrap();
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
    async fn server_passwords_from_get_config_never_reach_errors() {
        let s = MockServer::start().await;
        Mock::given(method("POST"))
            .and(path("/api"))
            .and(body_string_contains("mode=get_config"))
            .respond_with(
                ResponseTemplate::new(200).set_body_json(json!({"config": {"servers": [
                    {"name": "eweka", "username": "u", "password": "hunter2-pw", "connections": 30}
                ]}})),
            )
            .mount(&s)
            .await;
        Mock::given(method("POST"))
            .and(path("/api"))
            .and(body_string_contains("mode=set_config"))
            .respond_with(
                ResponseTemplate::new(200).set_body_json(
                    json!({"status": false, "error": "login failed with hunter2-pw"}),
                ),
            )
            .mount(&s)
            .await;
        let c = SabClient::new(&s.uri(), "k");
        let servers = c.get_config("servers").await.unwrap();
        assert_eq!(servers[0]["password"], "hunter2-pw");
        let err = c
            .set_config("servers", Some("eweka"), &[("connections", "20")])
            .await
            .unwrap_err()
            .to_string();
        assert!(
            !err.contains("hunter2-pw") && err.contains("<redacted>"),
            "{err}"
        );
        let req = s.received_requests().await.unwrap();
        let body = String::from_utf8_lossy(&req[1].body);
        assert!(body.contains("section=servers") && body.contains("keyword=eweka"));
    }

    #[tokio::test]
    async fn non_json_replies_are_decode_errors() {
        let s = MockServer::start().await;
        Mock::given(method("POST"))
            .and(path("/api"))
            .respond_with(ResponseTemplate::new(200).set_body_string("<html>login</html>"))
            .mount(&s)
            .await;
        let err = SabClient::new(&s.uri(), "k")
            .warnings()
            .await
            .unwrap_err()
            .to_string();
        assert!(err.starts_with("sabnzbd warnings: decode"), "{err}");
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

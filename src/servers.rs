//! News-server connection counts against each provider's account limit. Over
//! the limit the provider answers `502 Too many connections`, which SABnzbd
//! logs as warnings and retries, stalling downloads.

use std::collections::BTreeMap;

use plugin_toolkit::prelude::*;
use plugin_toolkit::serde_json::Value;

#[orca_struct]
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ServerStatus {
    /// SABnzbd's server keyword (its `name`).
    pub name: String,
    pub host: String,
    pub connections: u32,
    /// Declared account limit, when given.
    pub limit: Option<u32>,
    pub over_limit: bool,
    /// Warnings naming this host that report too many connections.
    pub limit_warnings: usize,
}

pub(crate) fn as_u32(v: &Value) -> Option<u32> {
    v.as_u64()
        .and_then(|n| u32::try_from(n).ok())
        .or_else(|| v.as_str().and_then(|s| s.trim().parse().ok()))
}

/// Whether a warning reports the provider's account connection limit.
pub(crate) fn is_limit_warning(text: &str) -> bool {
    let t = text.to_lowercase();
    t.contains("too many connections") || t.contains("connection limit")
}

/// Whether `text` names `host` as a whole token, optionally with `:port`.
fn names_host(text: &str, host: &str) -> bool {
    !host.is_empty()
        && text
            .split(|ch: char| !(ch.is_ascii_alphanumeric() || matches!(ch, '.' | '-' | ':')))
            .map(|t| t.trim_end_matches(['.', ':']))
            .any(|t| {
                let (h, port) = t.split_once(':').unwrap_or((t, ""));
                h.eq_ignore_ascii_case(host) && port.chars().all(|ch| ch.is_ascii_digit())
            })
}

/// `limits` is keyed by server name or host.
pub fn assess(
    servers: &Value,
    limits: &BTreeMap<String, u32>,
    warnings: &[String],
) -> Vec<ServerStatus> {
    servers
        .as_array()
        .into_iter()
        .flatten()
        .filter_map(|s| {
            let name = s.get("name")?.as_str()?.to_string();
            let host = s
                .get("host")
                .and_then(Value::as_str)
                .unwrap_or("")
                .to_string();
            let connections = s.get("connections").and_then(as_u32).unwrap_or(0);
            let limit = limits.get(&name).or_else(|| limits.get(&host)).copied();
            let limit_warnings = warnings
                .iter()
                .filter(|w| is_limit_warning(w) && names_host(w, &host))
                .count();
            Some(ServerStatus {
                over_limit: limit.is_some_and(|l| connections > l),
                name,
                host,
                connections,
                limit,
                limit_warnings,
            })
        })
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;
    use plugin_toolkit::serde_json::json;

    #[test]
    fn flags_servers_over_their_declared_limit() {
        let servers = json!([
            {"name": "eweka", "host": "news.eweka.nl", "connections": 30},
            {"name": "nh", "host": "news.newshosting.com", "connections": "60"}
        ]);
        let limits = BTreeMap::from([("news.eweka.nl".to_string(), 20), ("nh".to_string(), 60)]);
        let warnings = vec![
            "Too many connections to server news.eweka.nl [502 Too many connections]".to_string(),
            "unrelated".to_string(),
        ];
        let r = assess(&servers, &limits, &warnings);
        assert_eq!(
            (r[0].limit, r[0].over_limit, r[0].limit_warnings),
            (Some(20), true, 1)
        );
        assert_eq!(
            (r[1].connections, r[1].over_limit, r[1].limit_warnings),
            (60, false, 0)
        );
    }

    #[test]
    fn only_connection_limit_texts_count_as_limit_warnings() {
        assert!(is_limit_warning("Too many connections to server x"));
        assert!(is_limit_warning("server x [502 Connection limit reached]"));
        assert!(!is_limit_warning("Article 5021 missing from server x"));
        assert!(!is_limit_warning("server x [502 Bad Gateway]"));
        assert!(!is_limit_warning("server x [502 connection reset]"));
    }

    #[test]
    fn hosts_match_as_whole_tokens() {
        let h = "news.eweka.nl";
        assert!(names_host("Too many connections to NEWS.EWEKA.NL:563.", h));
        assert!(names_host("server news.eweka.nl [502]", h));
        assert!(!names_host("server xnews.eweka.nl [502]", h));
        assert!(!names_host("server news.eweka.nl.evil [502]", h));
        assert!(!names_host("server news.eweka.nl:abc", h));
        assert!(!names_host("anything", ""));
    }

    #[test]
    fn oversized_connection_counts_do_not_wrap() {
        assert_eq!(as_u32(&json!(4_294_967_296u64)), None);
        assert_eq!(as_u32(&json!(" 20 ")), Some(20));
    }
}

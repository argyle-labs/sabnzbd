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

fn as_u32(v: &Value) -> Option<u32> {
    v.as_u64()
        .map(|n| n as u32)
        .or_else(|| v.as_str().and_then(|s| s.trim().parse().ok()))
}

fn is_limit_warning(text: &str) -> bool {
    let t = text.to_lowercase();
    t.contains("too many connections") || t.contains("502")
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
                .filter(|w| is_limit_warning(w) && !host.is_empty() && w.contains(&host))
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
}

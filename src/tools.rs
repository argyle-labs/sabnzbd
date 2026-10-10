//! sabnzbd tool surface.
//!
//! Endpoint registry: `sabnzbd.{list, detail, create, update, delete}`.
//!
//!   - `sabnzbd.incomplete.status`  whether `download_dir` (incomplete) is on local disk
//!   - `sabnzbd.servers.status`     connections per news server against its account limit
//!   - `sabnzbd.servers.sync`       lower connections to the declared limit (dry run by default)
//!   - `sabnzbd.warnings.status`    current warnings, classified, with known false positives marked

use std::collections::BTreeMap;
use std::path::Path;

use plugin_toolkit::prelude::*;
use plugin_toolkit::serde_json::Value;

use crate::client::SabClient;
use crate::execute;
use crate::incomplete::{self, IncompleteStatus};
use crate::servers::{self, ServerStatus};
use crate::warnings::{self, ClassifiedWarning};

#[endpoint_resource(plugin = "sabnzbd")]
pub struct SabnzbdEndpoint {
    pub name: String,
    #[secret]
    pub api_key: String,
    pub enabled: bool,
}

async fn connect(name: &str) -> Result<SabClient> {
    let row = endpoint_db::require(name)?;
    if !row.enabled {
        bail!("sabnzbd endpoint '{name}' is disabled");
    }
    let api_key = plugin_toolkit::secrets::resolve_scoped(
        "sabnzbd",
        name,
        "api_key",
        (!row.api_key.is_empty()).then_some(row.api_key.as_str()),
    )?;
    let base_url = route::resolve_reachable(name, &row.routes, false).await?;
    Ok(SabClient::new(&base_url, &api_key))
}

#[orca_struct(args)]
pub struct IncompleteStatusArgs {
    /// Registered sabnzbd endpoint; its config is read over the API.
    #[arg(long)]
    #[serde(default)]
    pub name: Option<String>,
    /// Path to `sabnzbd.ini` on this host, instead of `name`. Relative dirs
    /// resolve against its directory.
    #[arg(long)]
    #[serde(default)]
    pub ini_path: Option<String>,
    /// Directory relative dirs resolve against with `name` (default `/config`).
    #[arg(long)]
    #[serde(default)]
    pub base: Option<String>,
    /// Container path of a network-backed mount (repeatable), e.g. `/downloads`.
    #[arg(long = "network-prefix")]
    #[serde(default)]
    pub network_prefixes: Vec<String>,
}

fn misc_dirs(misc: &Value) -> incomplete::Dirs {
    let get = |k: &str| {
        misc.get(k)
            .and_then(Value::as_str)
            .unwrap_or("")
            .to_string()
    };
    incomplete::Dirs {
        download_dir: get("download_dir"),
        complete_dir: get("complete_dir"),
    }
}

/// Report whether SABnzbd's incomplete `download_dir` is local. Flags one under
/// a `network_prefixes` mount or inside `complete_dir`. Takes exactly one of
/// `name` or `ini_path`; reading a host file makes it admin-only. Read-only.
#[orca_tool(domain = "sabnzbd", verb = "incomplete.status", role = "admin")]
async fn sabnzbd_incomplete_status(
    args: IncompleteStatusArgs,
    _ctx: &ToolCtx,
) -> Result<IncompleteStatus> {
    match (args.name, args.ini_path) {
        (Some(name), None) => {
            let misc = connect(&name).await?.config_section("misc").await?;
            let base = args
                .base
                .unwrap_or_else(|| incomplete::DEFAULT_BASE.to_string());
            Ok(incomplete::assess(
                &misc_dirs(&misc),
                &base,
                &args.network_prefixes,
            ))
        }
        (None, Some(path)) => incomplete_from_file(&path, &args.network_prefixes),
        _ => bail!("pass exactly one of name or ini_path"),
    }
}

fn incomplete_from_file(ini_path: &str, network_prefixes: &[String]) -> Result<IncompleteStatus> {
    let ini = std::fs::read_to_string(ini_path).with_context(|| format!("read {ini_path}"))?;
    let base = Path::new(ini_path)
        .parent()
        .map(|p| p.to_string_lossy().into_owned())
        .unwrap_or_default();
    Ok(incomplete::assess(
        &incomplete::parse_dirs(&ini),
        &base,
        network_prefixes,
    ))
}

fn parse_limit(s: &str) -> std::result::Result<(String, u32), String> {
    let (k, v) = s
        .split_once('=')
        .ok_or_else(|| format!("expected server=N, got `{s}`"))?;
    let n: u32 = v
        .trim()
        .parse()
        .map_err(|_| format!("bad limit in `{s}`"))?;
    if n == 0 {
        return Err(format!("limit must be at least 1 in `{s}`"));
    }
    Ok((k.trim().to_string(), n))
}

#[orca_struct(args)]
pub struct ServersStatusArgs {
    /// Registered sabnzbd endpoint name.
    #[arg(long)]
    pub name: String,
    /// Account connection limit as `server=N`, keyed by server name or host (repeatable).
    #[arg(long = "limit", value_parser = parse_limit)]
    #[serde(default)]
    pub limits: Vec<(String, u32)>,
}

#[orca_struct]
pub struct ServersStatusOutput {
    pub servers: Vec<ServerStatus>,
}

async fn server_statuses(c: &SabClient, limits: &[(String, u32)]) -> Result<Vec<ServerStatus>> {
    let limits: BTreeMap<String, u32> = limits.iter().cloned().collect();
    let cfg = c.config_section("servers").await?;
    let warns = c.warnings().await?;
    Ok(servers::assess(&cfg, &limits, &warns))
}

/// Connections configured per news server, the declared account limit, and how
/// many current warnings report too many connections to that host. Read-only.
#[orca_tool(domain = "sabnzbd", verb = "servers.status", role = "any")]
async fn sabnzbd_servers_status(
    args: ServersStatusArgs,
    _ctx: &ToolCtx,
) -> Result<ServersStatusOutput> {
    let c = connect(&args.name).await?;
    Ok(ServersStatusOutput {
        servers: server_statuses(&c, &args.limits).await?,
    })
}

#[orca_struct(args)]
pub struct ServersSyncArgs {
    /// Registered sabnzbd endpoint name.
    #[arg(long)]
    pub name: String,
    /// Account connection limit as `server=N`, keyed by server name or host (repeatable).
    #[arg(long = "limit", value_parser = parse_limit)]
    pub limits: Vec<(String, u32)>,
    /// Apply. Omitted, reports the plan and changes nothing.
    #[arg(long)]
    #[serde(default)]
    pub execute: bool,
}

#[orca_struct]
pub struct ServersSyncOutput {
    /// Servers lowered (or to lower, on a dry run) to their limit.
    pub lowered: Vec<ServerStatus>,
    pub changed: bool,
    pub dry_run: bool,
}

/// Lower each server's connections to its declared limit. Servers under their
/// limit are left alone. Verifies the write.
#[orca_tool(
    domain = "sabnzbd",
    verb = "servers.sync",
    role = "admin",
    execute_gated = false
)]
async fn sabnzbd_servers_sync(args: ServersSyncArgs, ctx: &ToolCtx) -> Result<ServersSyncOutput> {
    execute::require_admin("sabnzbd.servers.sync", ctx)?;
    let c = connect(&args.name).await?;
    servers_sync(&c, &args.limits, args.execute).await
}

async fn servers_sync(
    c: &SabClient,
    limits: &[(String, u32)],
    execute: bool,
) -> Result<ServersSyncOutput> {
    let lowered: Vec<ServerStatus> = server_statuses(c, limits)
        .await?
        .into_iter()
        .filter(|s| s.over_limit)
        .collect();
    let changed = execute && !lowered.is_empty();
    if changed {
        // Keep going past a failed server so one bad entry does not leave the
        // rest unapplied; the error then says exactly which ones took.
        let mut failed = Vec::new();
        for s in &lowered {
            if let Err(e) = c
                .set_server_connections(&s.name, s.limit.unwrap_or(s.connections))
                .await
            {
                failed.push(format!("{}: {e}", s.name));
            }
        }
        let still: Vec<String> = server_statuses(c, limits)
            .await?
            .into_iter()
            .filter(|s| s.over_limit)
            .map(|s| s.name)
            .collect();
        if !failed.is_empty() || !still.is_empty() {
            let applied: Vec<&str> = lowered
                .iter()
                .map(|s| s.name.as_str())
                .filter(|n| !still.iter().any(|x| x == n))
                .collect();
            bail!(
                "sabnzbd did not take the connection limit for {still:?} (applied: {applied:?}; errors: {})",
                if failed.is_empty() { "none".to_string() } else { failed.join("; ") }
            );
        }
    }
    Ok(ServersSyncOutput {
        lowered,
        changed,
        dry_run: !execute,
    })
}

#[orca_struct(args)]
pub struct WarningsStatusArgs {
    /// Registered sabnzbd endpoint name.
    #[arg(long)]
    pub name: String,
    /// Path verified to accept special-character filenames, e.g. a CIFS mount
    /// with `mapposix` (repeatable). The special-character warning on it is a
    /// false positive.
    #[arg(long = "special-chars-ok")]
    #[serde(default)]
    pub special_chars_ok: Vec<String>,
}

#[orca_struct]
pub struct WarningsStatusOutput {
    /// Warnings that need attention.
    pub actionable: usize,
    pub warnings: Vec<ClassifiedWarning>,
}

/// Current SABnzbd warnings, classified. Read-only.
#[orca_tool(domain = "sabnzbd", verb = "warnings.status", role = "any")]
async fn sabnzbd_warnings_status(
    args: WarningsStatusArgs,
    _ctx: &ToolCtx,
) -> Result<WarningsStatusOutput> {
    let warns = connect(&args.name).await?.warnings().await?;
    let warnings: Vec<ClassifiedWarning> = warns
        .iter()
        .map(|w| warnings::classify(w, &args.special_chars_ok))
        .collect();
    Ok(WarningsStatusOutput {
        actionable: warnings.iter().filter(|w| w.actionable).count(),
        warnings,
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use plugin_toolkit::contract::config::{Config, Model, Ports};
    use std::sync::Arc;

    fn ctx(home: &Path) -> ToolCtx {
        ToolCtx::new(Arc::new(Config {
            anthropic_api_key: None,
            lmstudio_url: String::new(),
            ollama_url: String::new(),
            default_model: Model::LMStudio {
                id: String::new(),
                url: String::new(),
            },
            app_dir: home.to_path_buf(),
            memory_root: home.to_path_buf(),
            db_path: home.join("orca.db"),
            ports: Ports::default(),
        }))
    }

    async fn sab(servers: Value, warnings: Value) -> (wiremock::MockServer, SabClient) {
        use wiremock::matchers::{method, path, query_param};
        use wiremock::{Mock, MockServer, ResponseTemplate};
        let s = MockServer::start().await;
        Mock::given(method("GET"))
            .and(path("/api"))
            .and(query_param("mode", "get_config"))
            .respond_with(
                ResponseTemplate::new(200).set_body_json(
                    plugin_toolkit::serde_json::json!({"config": {"servers": servers}}),
                ),
            )
            .mount(&s)
            .await;
        Mock::given(method("GET"))
            .and(path("/api"))
            .and(query_param("mode", "warnings"))
            .respond_with(
                ResponseTemplate::new(200)
                    .set_body_json(plugin_toolkit::serde_json::json!({"warnings": warnings})),
            )
            .mount(&s)
            .await;
        Mock::given(method("GET"))
            .and(path("/api"))
            .and(query_param("mode", "set_config"))
            .respond_with(
                ResponseTemplate::new(200)
                    .set_body_json(plugin_toolkit::serde_json::json!({"status": true})),
            )
            .mount(&s)
            .await;
        let c = SabClient::new(&s.uri(), "k");
        (s, c)
    }

    async fn set_calls(s: &wiremock::MockServer) -> usize {
        s.received_requests()
            .await
            .unwrap()
            .iter()
            .filter(|r| r.url.query().unwrap_or("").contains("mode=set_config"))
            .count()
    }

    #[tokio::test]
    async fn servers_sync_dry_run_plans_without_writing() {
        let (s, c) = sab(
            plugin_toolkit::serde_json::json!([{"name": "eweka", "host": "news.eweka.nl", "connections": 30}]),
            plugin_toolkit::serde_json::json!([]),
        )
        .await;
        let r = servers_sync(&c, &[("eweka".into(), 20)], false)
            .await
            .unwrap();
        assert_eq!(r.lowered.len(), 1);
        assert!(!r.changed && r.dry_run);
        assert_eq!(set_calls(&s).await, 0);
    }

    #[tokio::test]
    async fn servers_sync_execute_fails_when_the_limit_does_not_take() {
        let (s, c) = sab(
            plugin_toolkit::serde_json::json!([{"name": "eweka", "host": "news.eweka.nl", "connections": 30}]),
            plugin_toolkit::serde_json::json!([]),
        )
        .await;
        let err = servers_sync(&c, &[("eweka".into(), 20)], true)
            .await
            .err()
            .unwrap();
        assert!(err.to_string().contains("did not take"), "{err}");
        assert_eq!(set_calls(&s).await, 1);
    }

    #[tokio::test]
    async fn servers_sync_reports_a_partial_apply() {
        use wiremock::matchers::{method, path, query_param};
        use wiremock::{Mock, ResponseTemplate};
        let (s, c) = sab(
            plugin_toolkit::serde_json::json!([
                {"name": "a", "host": "a.example", "connections": 30},
                {"name": "b", "host": "b.example", "connections": 30}
            ]),
            plugin_toolkit::serde_json::json!([]),
        )
        .await;
        Mock::given(method("GET"))
            .and(path("/api"))
            .and(query_param("mode", "set_config"))
            .and(query_param("keyword", "a"))
            .respond_with(ResponseTemplate::new(200).set_body_json(
                plugin_toolkit::serde_json::json!({"status": false, "error": "nope"}),
            ))
            .with_priority(1)
            .mount(&s)
            .await;
        let err = servers_sync(&c, &[("a".into(), 20), ("b".into(), 20)], true)
            .await
            .err()
            .unwrap()
            .to_string();
        assert_eq!(set_calls(&s).await, 2, "kept going after a failed server");
        assert!(err.contains("a: sabnzbd set_config: nope"), "{err}");
    }

    #[tokio::test]
    async fn servers_sync_refuses_non_admins() {
        let dir = tempfile::tempdir().unwrap();
        let args = ServersSyncArgs {
            name: "sab".into(),
            limits: vec![],
            execute: false,
        };
        let err = sabnzbd_servers_sync(args, &ctx(dir.path()))
            .await
            .err()
            .unwrap();
        assert!(err.to_string().contains("no caller identity"), "{err}");
    }

    #[test]
    fn limit_args_parse() {
        assert_eq!(
            parse_limit("news.eweka.nl=20").unwrap(),
            ("news.eweka.nl".into(), 20)
        );
        assert!(parse_limit("x=0").is_err());
        assert!(parse_limit("x").is_err());
    }

    #[test]
    fn misc_dirs_come_from_the_api_section() {
        let d = misc_dirs(
            &plugin_toolkit::serde_json::json!({"download_dir": "Downloads/incomplete", "complete_dir": "/data"}),
        );
        assert_eq!(d.download_dir, "Downloads/incomplete");
    }

    #[test]
    fn reads_ini_path_and_resolves_relative_dirs_beside_it() {
        let dir = tempfile::tempdir().unwrap();
        let ini = dir.path().join("sabnzbd.ini");
        std::fs::write(&ini, "[misc]\ndownload_dir = Downloads/incomplete\n").unwrap();
        let base = dir.path().to_string_lossy().into_owned();
        let s = incomplete_from_file(&ini.to_string_lossy(), std::slice::from_ref(&base)).unwrap();
        assert_eq!(s.download_dir, format!("{base}/Downloads/incomplete"));
        assert!(!s.ok);
    }

    #[tokio::test]
    async fn incomplete_status_needs_exactly_one_source() {
        let dir = tempfile::tempdir().unwrap();
        let args = IncompleteStatusArgs {
            name: Some("sab".into()),
            ini_path: Some("/x".into()),
            base: None,
            network_prefixes: vec![],
        };
        assert!(sabnzbd_incomplete_status(args, &ctx(dir.path()))
            .await
            .is_err());
    }
}

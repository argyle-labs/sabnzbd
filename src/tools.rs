//! sabnzbd tools: the endpoint registry plus `incomplete`, `servers` and
//! `warnings` diagnostics, and `servers.sync`.

use std::collections::BTreeMap;
use std::path::Path;

use plugin_toolkit::prelude::*;
use plugin_toolkit::serde_json::Value;

use crate::client::{self, SabClient};
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
            let misc = connect(&name).await?.get_config("misc").await?;
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
    let cfg = c.get_config("servers").await?;
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
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ConnectionChange {
    pub name: String,
    pub host: String,
    pub from: u32,
    pub to: u32,
}

#[orca_struct]
#[derive(Debug)]
pub struct ServersSyncOutput {
    /// Servers lowered (or to lower, on a dry run) to their limit.
    pub changes: Vec<ConnectionChange>,
    /// Reasons the plan cannot be applied; non-empty refuses the execute.
    pub refusals: Vec<String>,
    pub changed: bool,
    pub dry_run: bool,
}

/// Lower each server's connections to its declared limit. Servers under their
/// limit are left alone. Admin-only, dry run included. After writing, reads
/// the servers back and fails unless each took its limit and no server was
/// added. The read-back checks SABnzbd's live config, not that it reached disk.
#[orca_tool(
    domain = "sabnzbd",
    verb = "servers.sync",
    role = "admin",
    execute_gated = false
)]
async fn sabnzbd_servers_sync(args: ServersSyncArgs, ctx: &ToolCtx) -> Result<ServersSyncOutput> {
    execute::require_admin("sabnzbd.servers.sync", ctx)?;
    // TODO(dry-run flag): becomes `ctx.dry_run()` and the `execute` arg goes.
    let dry_run = !args.execute;
    let c = connect(&args.name).await?;
    servers_sync(&c, &args.limits, dry_run).await
}

/// Changes that bring each over-limit server down to its limit, plus refusals:
/// a limit naming no configured server (a typo would otherwise apply nothing
/// silently), a key given twice, or a server limited by both name and host
/// with different values.
fn plan_servers(
    statuses: &[ServerStatus],
    limits: &[(String, u32)],
) -> (Vec<ConnectionChange>, Vec<String>) {
    let changes = statuses
        .iter()
        .filter(|s| s.over_limit)
        .filter_map(|s| {
            Some(ConnectionChange {
                name: s.name.clone(),
                host: s.host.clone(),
                from: s.connections,
                to: s.limit?,
            })
        })
        .collect();
    let lookup = |k: &str| -> Vec<u32> {
        limits
            .iter()
            .filter(|(key, _)| key == k)
            .map(|(_, n)| *n)
            .collect()
    };
    let mut refusals: Vec<String> = limits
        .iter()
        .filter(|(k, _)| !statuses.iter().any(|s| &s.name == k || &s.host == k))
        .map(|(k, _)| format!("limit `{k}` matches no configured server name or host"))
        .collect();
    let mut keys: Vec<&str> = limits.iter().map(|(k, _)| k.as_str()).collect();
    keys.sort_unstable();
    keys.dedup();
    for k in keys {
        let mut ns = lookup(k);
        ns.dedup();
        if ns.len() > 1 {
            refusals.push(format!("limit `{k}` is given more than once: {ns:?}"));
        }
    }
    for s in statuses {
        let (by_name, by_host) = (lookup(&s.name), lookup(&s.host));
        if let (Some(a), Some(b)) = (by_name.first(), by_host.first()) {
            if a != b && s.name != s.host {
                refusals.push(format!(
                    "server `{}` is limited to {a} by name and {b} by host `{}`",
                    s.name, s.host
                ));
            }
        }
    }
    (changes, refusals)
}

async fn servers_sync(
    c: &SabClient,
    limits: &[(String, u32)],
    dry_run: bool,
) -> Result<ServersSyncOutput> {
    let before = c.get_config("servers").await?;
    let limit_map: BTreeMap<String, u32> = limits.iter().cloned().collect();
    let (changes, refusals) = plan_servers(&servers::assess(&before, &limit_map, &[]), limits);
    if dry_run {
        return Ok(ServersSyncOutput {
            changes,
            refusals,
            changed: false,
            dry_run,
        });
    }
    if !refusals.is_empty() {
        bail!("sabnzbd.servers.sync refused: {}", refusals.join("; "));
    }
    if changes.is_empty() {
        return Ok(ServersSyncOutput {
            changes,
            refusals,
            changed: false,
            dry_run,
        });
    }
    // Keep going past a failed server so one bad entry does not leave the
    // rest unapplied; the error then says exactly which ones took.
    let mut written = Vec::new();
    let mut problems = Vec::new();
    for ch in &changes {
        match c.set_server_connections(&ch.name, ch.to).await {
            Ok(()) => written.push(ch.name.clone()),
            Err(e) => problems.push(format!("{}: {e}", ch.name)),
        }
    }
    match c.get_config("servers").await {
        Err(e) => problems.push(format!("read-back failed: {e}")),
        Ok(after) => {
            let known = client::server_names(&before);
            let added: Vec<String> = client::server_names(&after)
                .into_iter()
                .filter(|n| !known.contains(n))
                .collect();
            if !added.is_empty() {
                problems.push(format!(
                    "servers appeared that were not there before: {added:?}"
                ));
            }
            for ch in changes.iter().filter(|ch| written.contains(&ch.name)) {
                let now = after
                    .as_array()
                    .into_iter()
                    .flatten()
                    .find(|s| s.get("name").and_then(Value::as_str) == Some(ch.name.as_str()));
                match now
                    .and_then(|s| s.get("connections"))
                    .and_then(servers::as_u32)
                {
                    Some(n) if n == ch.to => {}
                    Some(n) => problems.push(format!(
                        "{}: reads back {n} connections, wrote {}",
                        ch.name, ch.to
                    )),
                    None => {
                        problems.push(format!("{}: missing or unreadable on read-back", ch.name))
                    }
                }
            }
        }
    }
    if !problems.is_empty() {
        bail!(
            "sabnzbd.servers.sync incomplete (written: {written:?}): {}",
            problems.join("; ")
        );
    }
    Ok(ServersSyncOutput {
        changes,
        refusals,
        changed: true,
        dry_run,
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
    let c = connect(&args.name).await?;
    warnings_status(&c, &args.special_chars_ok).await
}

async fn warnings_status(
    c: &SabClient,
    special_chars_ok: &[String],
) -> Result<WarningsStatusOutput> {
    let warnings: Vec<ClassifiedWarning> = c
        .warnings()
        .await?
        .iter()
        .map(|w| warnings::classify(&c.scrub(w), special_chars_ok))
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
    use plugin_toolkit::contract::CallerIdentity;
    use plugin_toolkit::serde_json::json;
    use std::sync::{Arc, Mutex};
    use wiremock::matchers::{body_string_contains, method, path};
    use wiremock::{Mock, MockServer, Request, Respond, ResponseTemplate};

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

    /// A SABnzbd whose `set_config` writes land in `servers`, served back by
    /// `get_config`.
    #[derive(Clone, Default)]
    struct Sab {
        servers: Arc<Mutex<Value>>,
        /// Ignore writes instead of applying them.
        ignore_writes: bool,
        /// Add a server on every write, as an upsert of a wrong name would.
        add_on_write: bool,
        /// Fail `get_config` once a write has landed.
        fail_after_write: bool,
        wrote: Arc<Mutex<bool>>,
    }

    fn form(req: &Request, key: &str) -> Option<String> {
        // Values in these tests need no percent-decoding.
        String::from_utf8_lossy(&req.body)
            .split('&')
            .find_map(|kv| kv.strip_prefix(key)?.strip_prefix('=').map(str::to_string))
    }

    impl Respond for Sab {
        fn respond(&self, req: &Request) -> ResponseTemplate {
            let ok = |v: Value| ResponseTemplate::new(200).set_body_json(v);
            match form(req, "mode").as_deref() {
                Some("get_config") if self.fail_after_write && *self.wrote.lock().unwrap() => {
                    ok(json!({"status": false, "error": "boom"}))
                }
                Some("get_config") => {
                    ok(json!({"config": {"servers": self.servers.lock().unwrap().clone()}}))
                }
                Some("warnings") => ok(json!({"warnings": []})),
                Some("set_config") => {
                    *self.wrote.lock().unwrap() = true;
                    let mut servers = self.servers.lock().unwrap();
                    if !self.ignore_writes {
                        let name = form(req, "keyword").unwrap();
                        let n: u32 = form(req, "connections").unwrap().parse().unwrap();
                        for s in servers.as_array_mut().unwrap() {
                            if s["name"] == name.as_str() {
                                s["connections"] = json!(n);
                            }
                        }
                    }
                    if self.add_on_write {
                        servers
                            .as_array_mut()
                            .unwrap()
                            .push(json!({"name": "ghost", "host": "", "connections": 0}));
                    }
                    ok(json!({"status": true}))
                }
                _ => ResponseTemplate::new(404),
            }
        }
    }

    async fn serve(sab: Sab) -> (MockServer, SabClient) {
        let s = MockServer::start().await;
        Mock::given(method("POST"))
            .and(path("/api"))
            .respond_with(sab)
            .mount(&s)
            .await;
        let c = SabClient::new(&s.uri(), "k");
        (s, c)
    }

    fn sab_with(servers: Value) -> Sab {
        Sab {
            servers: Arc::new(Mutex::new(servers)),
            ..Sab::default()
        }
    }

    fn eweka() -> Value {
        json!([{"name": "eweka", "host": "news.eweka.nl", "connections": 30}])
    }

    async fn set_calls(s: &MockServer) -> usize {
        s.received_requests()
            .await
            .unwrap()
            .iter()
            .filter(|r| String::from_utf8_lossy(&r.body).contains("mode=set_config"))
            .count()
    }

    #[tokio::test]
    async fn servers_sync_dry_run_plans_without_writing() {
        let (s, c) = serve(sab_with(eweka())).await;
        let r = servers_sync(&c, &[("eweka".into(), 20), ("typo".into(), 5)], true)
            .await
            .unwrap();
        assert_eq!(
            r.changes,
            vec![ConnectionChange {
                name: "eweka".into(),
                host: "news.eweka.nl".into(),
                from: 30,
                to: 20
            }]
        );
        assert_eq!(r.refusals.len(), 1, "{:?}", r.refusals);
        assert!(!r.changed && r.dry_run);
        assert_eq!(set_calls(&s).await, 0);
    }

    #[tokio::test]
    async fn servers_sync_plans_from_a_host_keyed_limit() {
        let (_s, c) = serve(sab_with(eweka())).await;
        let r = servers_sync(&c, &[("news.eweka.nl".into(), 20)], true)
            .await
            .unwrap();
        assert_eq!((r.changes[0].name.as_str(), r.changes[0].to), ("eweka", 20));
        assert!(r.refusals.is_empty(), "{:?}", r.refusals);
    }

    #[tokio::test]
    async fn servers_sync_refuses_conflicting_name_and_host_limits() {
        let (_s, c) = serve(sab_with(eweka())).await;
        let r = servers_sync(
            &c,
            &[("eweka".into(), 20), ("news.eweka.nl".into(), 10)],
            true,
        )
        .await
        .unwrap();
        assert_eq!(r.refusals.len(), 1, "{:?}", r.refusals);
        assert!(
            r.refusals[0].contains("by name and 10 by host"),
            "{:?}",
            r.refusals
        );
    }

    #[tokio::test]
    async fn servers_sync_execute_applies_and_reads_back() {
        let sab = sab_with(eweka());
        let (s, c) = serve(sab.clone()).await;
        let r = servers_sync(&c, &[("eweka".into(), 20)], false)
            .await
            .unwrap();
        assert!(r.changed && !r.dry_run);
        assert_eq!(set_calls(&s).await, 1);
        assert_eq!(sab.servers.lock().unwrap()[0]["connections"], 20);
    }

    #[tokio::test]
    async fn servers_sync_execute_fails_when_the_limit_does_not_take() {
        let (s, c) = serve(Sab {
            ignore_writes: true,
            ..sab_with(eweka())
        })
        .await;
        let err = servers_sync(&c, &[("eweka".into(), 20)], false)
            .await
            .unwrap_err()
            .to_string();
        assert!(
            err.contains("eweka: reads back 30 connections, wrote 20"),
            "{err}"
        );
        assert_eq!(set_calls(&s).await, 1);
    }

    #[tokio::test]
    async fn servers_sync_execute_reports_a_server_that_appeared() {
        let (_s, c) = serve(Sab {
            add_on_write: true,
            ..sab_with(eweka())
        })
        .await;
        let err = servers_sync(&c, &[("eweka".into(), 20)], false)
            .await
            .unwrap_err()
            .to_string();
        assert!(err.contains("appeared") && err.contains("ghost"), "{err}");
    }

    #[tokio::test]
    async fn servers_sync_read_back_failure_still_reports_the_writes() {
        let servers = json!([
            {"name": "a", "host": "a.example", "connections": 30},
            {"name": "b", "host": "b.example", "connections": 30}
        ]);
        let (s, c) = serve(Sab {
            fail_after_write: true,
            ..sab_with(servers)
        })
        .await;
        let err = servers_sync(&c, &[("a".into(), 20), ("b".into(), 20)], false)
            .await
            .unwrap_err()
            .to_string();
        assert_eq!(set_calls(&s).await, 1, "b's existence check failed");
        assert!(err.contains(r#"written: ["a"]"#), "{err}");
        assert!(err.contains("b: sabnzbd get_config: boom"), "{err}");
        assert!(err.contains("read-back failed"), "{err}");
    }

    #[tokio::test]
    async fn servers_sync_reports_a_partial_apply() {
        let servers = json!([
            {"name": "a", "host": "a.example", "connections": 30},
            {"name": "b", "host": "b.example", "connections": 30}
        ]);
        let (s, c) = serve(sab_with(servers)).await;
        Mock::given(method("POST"))
            .and(path("/api"))
            .and(body_string_contains("mode=set_config"))
            .and(body_string_contains("keyword=a"))
            .respond_with(
                ResponseTemplate::new(200).set_body_json(json!({"status": false, "error": "nope"})),
            )
            .with_priority(1)
            .mount(&s)
            .await;
        let err = servers_sync(&c, &[("a".into(), 20), ("b".into(), 20)], false)
            .await
            .unwrap_err()
            .to_string();
        assert_eq!(set_calls(&s).await, 2, "kept going after a failed server");
        assert!(err.contains("a: sabnzbd set_config: nope"), "{err}");
        assert!(err.contains(r#"written: ["b"]"#), "{err}");
    }

    #[tokio::test]
    async fn servers_sync_execute_is_refused_when_a_limit_matches_nothing() {
        let (s, c) = serve(sab_with(eweka())).await;
        let err = servers_sync(&c, &[("eweka".into(), 20), ("typo".into(), 5)], false)
            .await
            .unwrap_err();
        assert!(err.to_string().contains("refused"), "{err}");
        assert_eq!(set_calls(&s).await, 0);
    }

    #[tokio::test]
    async fn servers_sync_execute_is_refused_even_with_nothing_to_change() {
        let (s, c) = serve(sab_with(eweka())).await;
        let err = servers_sync(&c, &[("eweka".into(), 40), ("typo".into(), 5)], false)
            .await
            .unwrap_err();
        assert!(err.to_string().contains("refused"), "{err}");
        assert_eq!(set_calls(&s).await, 0);
    }

    #[tokio::test]
    async fn warnings_status_scrubs_each_warning() {
        let s = MockServer::start().await;
        Mock::given(method("POST"))
            .and(path("/api"))
            .respond_with(
                ResponseTemplate::new(200)
                    .set_body_json(json!({"warnings": ["login with s3cr3t-key failed"]})),
            )
            .mount(&s)
            .await;
        let c = SabClient::new(&s.uri(), "s3cr3t-key");
        let r = warnings_status(&c, &[]).await.unwrap();
        assert_eq!(r.warnings[0].text, "login with <redacted> failed");
        assert_eq!(r.actionable, 1);
    }

    #[tokio::test]
    async fn servers_sync_refuses_non_admins_on_the_dry_run() {
        let dir = tempfile::tempdir().unwrap();
        let mut ctx = ctx(dir.path());
        ctx.set_caller(Some(CallerIdentity {
            user_id: "u".into(),
            username: "op".into(),
            role: "user".into(),
            can_mutate: true,
        }));
        let args = ServersSyncArgs {
            name: "sab".into(),
            limits: vec![],
            execute: false,
        };
        let err = sabnzbd_servers_sync(args, &ctx).await.err().unwrap();
        assert!(err.to_string().contains("requires role 'admin'"), "{err}");
    }

    #[tokio::test]
    async fn servers_sync_refuses_a_call_without_caller() {
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
        let d =
            misc_dirs(&json!({"download_dir": "Downloads/incomplete", "complete_dir": "/data"}));
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

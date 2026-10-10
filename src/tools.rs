//! sabnzbd tool surface.
//!
//!   - `sabnzbd.incomplete.status`  whether `download_dir` (incomplete) is on local disk

use std::path::Path;

use plugin_toolkit::prelude::*;

use crate::incomplete::{self, IncompleteStatus};

#[orca_struct(args)]
pub struct IncompleteStatusArgs {
    /// Path to `sabnzbd.ini` on this host. Relative dirs resolve against its directory.
    #[arg(long)]
    #[serde(default)]
    pub ini_path: Option<String>,
    /// `sabnzbd.ini` contents, instead of `ini_path`.
    #[arg(long)]
    #[serde(default)]
    pub ini: Option<String>,
    /// Directory relative dirs resolve against when `ini` is given (default `/config`).
    #[arg(long)]
    #[serde(default)]
    pub base: Option<String>,
    /// Container path of a network-backed mount (repeatable), e.g. `/downloads`.
    #[arg(long = "network-prefix")]
    #[serde(default)]
    pub network_prefixes: Vec<String>,
}

/// Report whether SABnzbd's incomplete `download_dir` is local. Flags one under
/// a `network_prefixes` mount or inside `complete_dir`. Read-only.
#[orca_tool(domain = "sabnzbd", verb = "incomplete.status", role = "any")]
async fn sabnzbd_incomplete_status(
    args: IncompleteStatusArgs,
    _ctx: &ToolCtx,
) -> Result<IncompleteStatus> {
    let (ini, base) = match (args.ini, args.ini_path) {
        (Some(_), Some(_)) => bail!("pass either ini or ini_path, not both"),
        (Some(ini), None) => (
            ini,
            args.base
                .unwrap_or_else(|| incomplete::DEFAULT_BASE.to_string()),
        ),
        (None, Some(path)) => {
            let ini = std::fs::read_to_string(&path).with_context(|| format!("read {path}"))?;
            let dir = Path::new(&path)
                .parent()
                .map(|p| p.to_string_lossy().into_owned())
                .unwrap_or_default();
            (ini, args.base.unwrap_or(dir))
        }
        (None, None) => bail!("ini or ini_path is required"),
    };
    Ok(incomplete::assess(
        &incomplete::parse_dirs(&ini),
        &base,
        &args.network_prefixes,
    ))
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

    #[tokio::test]
    async fn reads_ini_path_and_resolves_relative_dirs_beside_it() {
        let dir = tempfile::tempdir().unwrap();
        let ini = dir.path().join("sabnzbd.ini");
        std::fs::write(&ini, "[misc]\ndownload_dir = Downloads/incomplete\n").unwrap();
        let base = dir.path().to_string_lossy().into_owned();
        let args = IncompleteStatusArgs {
            ini_path: Some(ini.to_string_lossy().into_owned()),
            ini: None,
            base: None,
            network_prefixes: vec![base.clone()],
        };
        let s = sabnzbd_incomplete_status(args, &ctx(dir.path()))
            .await
            .unwrap();
        assert_eq!(s.download_dir, format!("{base}/Downloads/incomplete"));
        assert!(!s.ok);
    }

    #[tokio::test]
    async fn inline_ini_defaults_base_and_rejects_ambiguous_input() {
        let dir = tempfile::tempdir().unwrap();
        let args = IncompleteStatusArgs {
            ini_path: None,
            ini: Some("[misc]\ndownload_dir = /incomplete\n".into()),
            base: None,
            network_prefixes: vec!["/downloads".into()],
        };
        assert!(
            sabnzbd_incomplete_status(args, &ctx(dir.path()))
                .await
                .unwrap()
                .ok
        );
        let both = IncompleteStatusArgs {
            ini_path: Some("/x".into()),
            ini: Some(String::new()),
            base: None,
            network_prefixes: vec![],
        };
        assert!(sabnzbd_incomplete_status(both, &ctx(dir.path()))
            .await
            .is_err());
    }
}

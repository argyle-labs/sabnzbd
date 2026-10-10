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
    pub ini_path: String,
    /// Container path of a network-backed mount (repeatable), e.g. `/downloads`.
    #[arg(long = "network-prefix")]
    #[serde(default)]
    pub network_prefixes: Vec<String>,
}

/// Report whether SABnzbd's incomplete `download_dir` is local. Flags one under
/// a `network_prefixes` mount or inside `complete_dir`. Reads a file on this
/// host, so it is admin-only. Read-only.
#[orca_tool(domain = "sabnzbd", verb = "incomplete.status", role = "admin")]
async fn sabnzbd_incomplete_status(
    args: IncompleteStatusArgs,
    _ctx: &ToolCtx,
) -> Result<IncompleteStatus> {
    incomplete_from_file(&args.ini_path, &args.network_prefixes)
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

#[cfg(test)]
mod tests {
    use super::*;

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

    #[test]
    fn missing_ini_is_an_error() {
        assert!(incomplete_from_file("/nonexistent/sabnzbd.ini", &[]).is_err());
    }
}

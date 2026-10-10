//! Incomplete-download placement from `sabnzbd.ini`. SABnzbd writes thousands
//! of small `.part`/unrar files to `download_dir`; on a network share these fail
//! (`mkstemp` errors, auto-pause) when the NAS is I/O-starved. `download_dir`
//! belongs on local disk; only `complete_dir` should be on the library share.

use plugin_toolkit::prelude::*;

/// SABnzbd's config dir in the standard container image; relative dirs in
/// `sabnzbd.ini` resolve against the directory holding the ini.
pub const DEFAULT_BASE: &str = "/config";
/// SABnzbd's values when `download_dir` / `complete_dir` are unset.
pub const DEFAULT_DOWNLOAD_DIR: &str = "Downloads/incomplete";
pub const DEFAULT_COMPLETE_DIR: &str = "Downloads/complete";

/// `[misc]` directories from `sabnzbd.ini`, as written.
#[derive(Debug, Default, Clone, PartialEq, Eq)]
pub struct Dirs {
    pub download_dir: String,
    pub complete_dir: String,
}

/// configobj value: surrounding quotes removed, or an unquoted value cut at an
/// inline `#` comment.
fn ini_value(raw: &str) -> String {
    let v = raw.trim();
    for q in ['"', '\''] {
        if let Some(end) = v.strip_prefix(q).and_then(|rest| rest.find(q)) {
            return v[1..=end].to_string();
        }
    }
    v.split_once('#').map_or(v, |(v, _)| v).trim().to_string()
}

/// Top-level `[misc]` keys only; `[[server]]` subsections are skipped.
pub fn parse_dirs(ini: &str) -> Dirs {
    let mut dirs = Dirs::default();
    let mut in_misc = false;
    for line in ini.lines().map(str::trim) {
        if line.is_empty() || line.starts_with('#') {
            continue;
        }
        if let Some(header) = line.strip_prefix('[') {
            in_misc = !header.starts_with('[')
                && header
                    .split_once(']')
                    .is_some_and(|(name, _)| name.trim() == "misc");
            continue;
        }
        if !in_misc {
            continue;
        }
        let Some((k, v)) = line.split_once('=') else {
            continue;
        };
        match k.trim() {
            "download_dir" => dirs.download_dir = ini_value(v),
            "complete_dir" => dirs.complete_dir = ini_value(v),
            _ => {}
        }
    }
    dirs
}

/// Lexical normalization: collapses `//`, `.` and `..`; never touches the fs.
/// Returns `None` for a relative path.
fn normalize(path: &str) -> Option<Vec<&str>> {
    if !path.starts_with('/') {
        return None;
    }
    let mut out: Vec<&str> = Vec::new();
    for c in path.split('/') {
        match c {
            "" | "." => {}
            ".." => {
                out.pop();
            }
            c => out.push(c),
        }
    }
    Some(out)
}

/// `path` equals `prefix` or sits beneath it, compared component-wise after
/// normalization. Kept in step with the copy in the qbittorrent plugin until it
/// moves to plugin-toolkit.
fn under(path: &str, prefix: &str) -> bool {
    match (normalize(path), normalize(prefix)) {
        (Some(p), Some(pre)) => p.starts_with(&pre),
        _ => false,
    }
}

/// Absolute, normalized form of a configured dir.
pub fn resolve(dir: &str, base: &str) -> String {
    let joined = if dir.starts_with('/') {
        dir.to_string()
    } else {
        format!("{}/{dir}", base.trim_end_matches('/'))
    };
    match normalize(&joined) {
        Some(c) => format!("/{}", c.join("/")),
        None => joined,
    }
}

#[orca_struct]
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct IncompleteStatus {
    /// `download_dir`, resolved to an absolute path.
    pub download_dir: String,
    /// `complete_dir`, resolved to an absolute path.
    pub complete_dir: String,
    /// Whether `download_dir` is local.
    pub ok: bool,
    pub problem: Option<String>,
}

/// Paths are container-side, so network mounts are supplied by the caller.
/// Unset dirs take SABnzbd's defaults.
pub fn assess(dirs: &Dirs, base: &str, network_prefixes: &[String]) -> IncompleteStatus {
    let or_default = |v: &str, d: &'static str| {
        if v.trim().is_empty() {
            d.to_string()
        } else {
            v.to_string()
        }
    };
    let dl = resolve(&or_default(&dirs.download_dir, DEFAULT_DOWNLOAD_DIR), base);
    let complete = resolve(&or_default(&dirs.complete_dir, DEFAULT_COMPLETE_DIR), base);
    let problem = if let Some(net) = network_prefixes.iter().find(|n| under(&dl, n)) {
        Some(format!("download_dir {dl} is on network mount {net}"))
    } else if under(&dl, &complete) {
        Some(format!(
            "download_dir {dl} is inside complete_dir {complete}"
        ))
    } else {
        None
    };
    IncompleteStatus {
        ok: problem.is_none(),
        problem,
        download_dir: dl,
        complete_dir: complete,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const INI: &str = "\
__version__ = 19
[ misc ]
# comment
download_dir = /downloads/incomplete   # inline
complete_dir = \"/downloads/complete # not a comment\"
[servers]
[[news.example.com]]
download_dir = /bogus
";

    #[test]
    fn parses_misc_dirs_tolerating_spacing_and_comments() {
        assert_eq!(
            parse_dirs(INI),
            Dirs {
                download_dir: "/downloads/incomplete".into(),
                complete_dir: "/downloads/complete # not a comment".into(),
            }
        );
    }

    #[test]
    fn subsection_keys_are_ignored() {
        assert_eq!(
            parse_dirs("[misc]\n[[misc]]\ndownload_dir = /x\n").download_dir,
            ""
        );
    }

    #[test]
    fn incomplete_on_network_mount_is_flagged() {
        let s = assess(&parse_dirs(INI), DEFAULT_BASE, &["/downloads".into()]);
        assert!(s.problem.unwrap().contains("network mount /downloads"));
    }

    #[test]
    fn local_incomplete_is_ok() {
        let dirs = Dirs {
            download_dir: "/incomplete".into(),
            complete_dir: "/downloads/complete".into(),
        };
        assert!(assess(&dirs, DEFAULT_BASE, &["/downloads".into()]).ok);
    }

    #[test]
    fn relative_dirs_resolve_against_one_base_before_comparing() {
        let dirs = Dirs {
            download_dir: "Downloads/complete/tmp".into(),
            complete_dir: "Downloads/complete".into(),
        };
        let s = assess(&dirs, "/config", &[]);
        assert_eq!(s.download_dir, "/config/Downloads/complete/tmp");
        assert!(s
            .problem
            .unwrap()
            .contains("inside complete_dir /config/Downloads/complete"));
        let s = assess(&dirs, "/mnt/share/sab", &["/mnt/share".into()]);
        assert!(s.problem.unwrap().contains("network mount"));
    }

    #[test]
    fn lookalike_and_dotted_paths_compare_by_component() {
        let look = Dirs {
            download_dir: "/downloads2/inc".into(),
            ..Dirs::default()
        };
        assert!(assess(&look, DEFAULT_BASE, &["/downloads".into()]).ok);
        let dotted = Dirs {
            download_dir: "/local/../downloads//inc".into(),
            ..Dirs::default()
        };
        assert!(!assess(&dotted, DEFAULT_BASE, &["/downloads".into()]).ok);
    }

    #[test]
    fn unset_dirs_take_sabnzbds_defaults() {
        let s = assess(&parse_dirs("[misc]\ndownload_dir = \n"), "/config", &[]);
        assert_eq!(s.download_dir, "/config/Downloads/incomplete");
        assert_eq!(s.complete_dir, "/config/Downloads/complete");
        assert!(s.ok);
        let s = assess(&Dirs::default(), "/mnt/share/sab", &["/mnt/share".into()]);
        assert!(s.problem.unwrap().contains("network mount"));
    }
}

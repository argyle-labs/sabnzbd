//! Warning classification. SABnzbd's `helpful_warnings` probe reports a
//! folder "not writable with special character filenames" whenever its test
//! write of such a name fails; on a CIFS mount with `mapposix` those names do
//! write, so the warning is a false positive there.

use plugin_toolkit::prelude::*;

const SPECIAL_CHARS: &str = "not writable with special character filenames";

#[orca_struct]
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ClassifiedWarning {
    pub text: String,
    /// `special-chars-false-positive`, `special-chars`, `connection-limit` or `other`.
    pub class: String,
    /// Whether this warning needs attention.
    pub actionable: bool,
}

/// Path components with `.` dropped and `..` resolved lexically.
fn components(path: &str) -> Vec<&str> {
    let mut out = Vec::new();
    for c in path.split('/') {
        match c {
            "" | "." => {}
            ".." => {
                out.pop();
            }
            c => out.push(c),
        }
    }
    out
}

fn component_prefix(path: &str, prefix: &str) -> bool {
    let (p, pre) = (components(path), components(prefix));
    !pre.is_empty() && p.starts_with(&pre)
}

/// Absolute paths in `text`, stripped of surrounding quotes and trailing punctuation.
fn paths(text: &str) -> impl Iterator<Item = &str> {
    text.split_whitespace()
        .map(|t| {
            t.trim_start_matches(['"', '\'', '(', '['])
                .trim_end_matches(['"', '\'', ')', ']', '.', ',', ';', ':', '!', '?'])
        })
        .filter(|t| t.starts_with('/'))
}

/// `special_chars_ok` lists paths verified to accept special-character names
/// (e.g. a CIFS mount with `mapposix`).
pub fn classify(text: &str, special_chars_ok: &[String]) -> ClassifiedWarning {
    let lower = text.to_lowercase();
    let (class, actionable) = if lower.contains(SPECIAL_CHARS) {
        let verified =
            paths(text).any(|p| special_chars_ok.iter().any(|ok| component_prefix(p, ok)));
        if verified {
            ("special-chars-false-positive", false)
        } else {
            ("special-chars", true)
        }
    } else if crate::servers::is_limit_warning(text) {
        ("connection-limit", true)
    } else {
        ("other", true)
    };
    ClassifiedWarning {
        text: text.to_string(),
        class: class.to_string(),
        actionable,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const W: &str = "/downloads/incomplete is not writable with special character filenames. This can cause problems.";

    #[test]
    fn special_char_warning_on_a_verified_path_is_not_actionable() {
        let c = classify(W, &["/downloads".into()]);
        assert_eq!(
            (c.class.as_str(), c.actionable),
            ("special-chars-false-positive", false)
        );
        let c = classify(W, &["/downloads2".into()]);
        assert_eq!((c.class.as_str(), c.actionable), ("special-chars", true));
    }

    #[test]
    fn paths_are_unquoted_trimmed_and_normalized() {
        let ok = ["/downloads".to_string()];
        let class = |t: &str| classify(t, &ok).class;
        assert_eq!(
            class(
                "Folder '/downloads/incomplete' is not writable with special character filenames"
            ),
            "special-chars-false-positive"
        );
        assert_eq!(
            class(
                "Folder \"/downloads/incomplete\": not writable with special character filenames"
            ),
            "special-chars-false-positive"
        );
        assert_eq!(
            class("/downloads/../etc is not writable with special character filenames"),
            "special-chars"
        );
        assert_eq!(
            class("/downloads/./a/../b. is not writable with special character filenames"),
            "special-chars-false-positive"
        );
    }

    #[test]
    fn other_classes() {
        assert_eq!(
            classify("Too many connections to server x", &[]).class,
            "connection-limit"
        );
        assert_eq!(classify("Disk error on creating file", &[]).class, "other");
    }
}

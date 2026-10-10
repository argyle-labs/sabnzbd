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

fn component_prefix(path: &str, prefix: &str) -> bool {
    let comps = |p: &str| -> Vec<String> {
        p.split('/')
            .filter(|c| !c.is_empty() && *c != ".")
            .map(str::to_string)
            .collect()
    };
    let (p, pre) = (comps(path), comps(prefix));
    !pre.is_empty() && p.starts_with(&pre)
}

/// `special_chars_ok` lists paths verified to accept special-character names
/// (e.g. a CIFS mount with `mapposix`).
pub fn classify(text: &str, special_chars_ok: &[String]) -> ClassifiedWarning {
    let lower = text.to_lowercase();
    let (class, actionable) = if lower.contains(SPECIAL_CHARS) {
        let verified = text
            .split_whitespace()
            .filter(|t| t.starts_with('/'))
            .any(|p| special_chars_ok.iter().any(|ok| component_prefix(p, ok)));
        if verified {
            ("special-chars-false-positive", false)
        } else {
            ("special-chars", true)
        }
    } else if lower.contains("too many connections")
        || (lower.contains("502 ") && lower.contains("connection"))
    {
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
    fn other_classes() {
        assert_eq!(
            classify("Too many connections to server x", &[]).class,
            "connection-limit"
        );
        assert_eq!(classify("Disk error on creating file", &[]).class, "other");
    }
}

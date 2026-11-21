//! Credential stripping for anything that came from git or a remote URL.
//!
//! Principle 6: secrets stay local. Every string read from git is passed through here
//! before it is stored, so a token embedded in a remote URL never reaches a scan file,
//! the terminal or a log.

/// Remove embedded credentials from one URL.
///
/// For `http`, `https`, `ftp` and any URL with a `user:password` part, the whole userinfo is
/// removed. A bare user name on an `ssh` URL (`ssh://git@host/x`) is not a credential and is kept.
/// Text that is not a `scheme://` URL (scp-style `git@host:path`, local paths) is returned as is.
pub fn strip_url_credentials(url: &str) -> String {
    let idx = match url.find("://") {
        Some(i) => i,
        None => return url.to_string(),
    };
    let (scheme_part, rest) = url.split_at(idx + 3);
    // A password may contain '/', '?' or '#' unescaped, so everything up to the last '@' is
    // treated as userinfo. This can over-strip a path that contains '@'; leaking is worse.
    let (userinfo, remainder) = match rest.rfind('@') {
        Some(at) => (&rest[..at], &rest[at + 1..]),
        None => ("", rest),
    };
    // Query strings and fragments are never stored: they can carry tokens.
    let end = remainder.find(['?', '#']).unwrap_or(remainder.len());
    let remainder = &remainder[..end];
    let scheme = scheme_part
        .trim_end_matches("://")
        .rsplit(|c: char| !c.is_ascii_alphanumeric() && c != '+')
        .next()
        .unwrap_or("")
        .to_ascii_lowercase();
    let ssh_like = matches!(scheme.as_str(), "ssh" | "git+ssh" | "ssh+git");
    let plain_user = !userinfo.is_empty()
        && !userinfo.contains([':', '?', '#', '/'])
        && !userinfo.chars().any(char::is_whitespace);
    if ssh_like && plain_user {
        return format!("{scheme_part}{userinfo}@{remainder}");
    }
    format!("{scheme_part}{remainder}")
}

/// Strip credentials from every URL-looking token in free text (git error output).
pub fn redact_text(text: &str) -> String {
    let mut out = String::with_capacity(text.len());
    for piece in text.split_inclusive(char::is_whitespace) {
        let token_end = piece.trim_end_matches(char::is_whitespace).len();
        let (token, ws) = piece.split_at(token_end);
        if token.contains("://") {
            out.push_str(&strip_url_credentials(token));
        } else {
            out.push_str(token);
        }
        out.push_str(ws);
    }
    out
}

/// The one place text that may carry repository-controlled content (config keys, remote names,
/// file names, git error output) passes through before it is stored in a note, reason or error.
///
/// URL credentials are stripped ([`redact_text`]) and control characters become spaces, so a
/// value can neither carry a token into the output nor forge extra lines in it.
pub fn diag(text: &str) -> String {
    redact_text(text)
        .chars()
        .map(|c| if c.is_control() && c != '\n' { ' ' } else { c })
        .collect()
}

/// A config key reduced to what is safe to print: the section and the variable name.
///
/// A key is `section.name` or `section.<subsection>.name`, and the subsection is free text that
/// can be a URL with a token in it (`credential.https://user:TOKEN@host.helper`). It is never
/// printed: `credential.<redacted>.helper`. Section and variable names are limited to letters,
/// digits and `-` by git; anything else is replaced.
pub fn config_key_label(key: &str) -> String {
    let clean = |s: &str| -> String {
        s.chars()
            .take(40)
            .map(|c| {
                if c.is_ascii_alphanumeric() || c == '-' {
                    c
                } else {
                    '?'
                }
            })
            .collect()
    };
    match (key.find('.'), key.rfind('.')) {
        (Some(first), Some(last)) if first < last => {
            format!(
                "{}.<redacted>.{}",
                clean(&key[..first]),
                clean(&key[last + 1..])
            )
        }
        (Some(first), _) => format!("{}.{}", clean(&key[..first]), clean(&key[first + 1..])),
        (None, _) => "<unparsed key>".to_string(),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn config_key_label_never_prints_the_subsection() {
        assert_eq!(
            config_key_label("credential.https://user:SYNTHETIC_TOKEN@example.invalid.helper"),
            "credential.<redacted>.helper"
        );
        assert_eq!(
            config_key_label("filter.lfs.clean"),
            "filter.<redacted>.clean"
        );
        assert_eq!(config_key_label("core.fsmonitor"), "core.fsmonitor");
        assert_eq!(config_key_label("nodots"), "<unparsed key>");
        assert!(!config_key_label("a.b.c.d.name\u{1b}[0m").contains('\u{1b}'));
    }

    #[test]
    fn diag_strips_credentials_and_control_characters() {
        let d = diag("see https://u:tok@host.invalid/x\u{1b}[31m red");
        assert!(!d.contains("tok") && !d.contains('\u{1b}'), "{d}");
    }

    #[test]
    fn strips_user_and_token_from_https() {
        let s = strip_url_credentials("https://user:s3cr3t-token@example.invalid/x.git");
        assert_eq!(s, "https://example.invalid/x.git");
    }

    #[test]
    fn strips_token_only_userinfo_on_https() {
        let s = strip_url_credentials("https://ghp_abcdef@example.invalid/x.git");
        assert_eq!(s, "https://example.invalid/x.git");
    }

    #[test]
    fn keeps_plain_ssh_user() {
        let s = strip_url_credentials("ssh://git@example.invalid/x.git");
        assert_eq!(s, "ssh://git@example.invalid/x.git");
    }

    #[test]
    fn strips_ssh_password() {
        let s = strip_url_credentials("ssh://git:pw@example.invalid/x.git");
        assert_eq!(s, "ssh://example.invalid/x.git");
    }

    #[test]
    fn password_with_slash_does_not_leak_tail() {
        let s = strip_url_credentials("https://user:pa/ss@example.invalid/x.git");
        assert!(!s.contains("pa/ss") && !s.contains("user"));
    }

    #[test]
    fn f10_userinfo_containing_query_or_fragment_characters_is_stripped() {
        for url in [
            "https://user:pa?ss@host.invalid/x.git",
            "https://user:pa#ss@host.invalid/x.git",
            "https://tok?en@host.invalid/x.git",
        ] {
            let s = strip_url_credentials(url);
            assert_eq!(s, "https://host.invalid/x.git", "{url}");
        }
    }

    #[test]
    fn f10_query_strings_and_fragments_are_dropped_from_stored_urls() {
        let s = strip_url_credentials("https://host.invalid/x.git?access_token=abc123");
        assert_eq!(s, "https://host.invalid/x.git");
        let s = strip_url_credentials("https://host.invalid/x.git#secret-frag");
        assert_eq!(s, "https://host.invalid/x.git");
    }

    #[test]
    fn leaves_scp_and_paths_alone() {
        assert_eq!(
            strip_url_credentials("git@host:o/r.git"),
            "git@host:o/r.git"
        );
        assert_eq!(strip_url_credentials("/srv/r.git"), "/srv/r.git");
    }

    #[test]
    fn redacts_inside_error_text() {
        let t = "fatal: unable to access 'https://u:tok@example.invalid/r.git/': failed\nnext";
        let r = redact_text(t);
        assert!(!r.contains("tok"), "{r}");
        assert!(r.contains("next"));
    }
}

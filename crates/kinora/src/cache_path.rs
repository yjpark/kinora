use crate::hash::SHORTHASH_LEN;

/// Cache path derivation for a kinora repo, keyed off `config.styx → repo-url`.
///
/// Two repos with distinct URLs always derive distinct cache paths (via the
/// BLAKE3-of-normalized-url shorthash). The human-readable `name` suffix keeps
/// the directory name legible on disk.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct CachePath {
    pub shorthash: String,
    pub name: String,
}

impl CachePath {
    /// Derive a cache path from a repo URL.
    ///
    /// Normalization rules (matched by the tests):
    /// - strip the scheme (`https://`, `ssh://`, `git+ssh://`, `git@`)
    /// - drop userinfo (`user@host` → `host`)
    /// - normalize SSH `host:path` to `host/path`
    /// - strip a trailing `.git` and trailing `/`
    /// - lowercase the host segment (path is preserved verbatim)
    pub fn from_repo_url(url: &str) -> Self {
        let normalized = normalize_url(url);
        let shorthash = blake3::hash(normalized.as_bytes())
            .to_hex()
            .to_string()
            .chars()
            .take(SHORTHASH_LEN)
            .collect();
        let name = sanitize_name(last_segment(&normalized));
        Self { shorthash, name }
    }

    /// Directory basename: `<shorthash>-<name>`.
    pub fn subdir(&self) -> String {
        if self.name.is_empty() {
            self.shorthash.clone()
        } else {
            format!("{}-{}", self.shorthash, self.name)
        }
    }
}

fn normalize_url(url: &str) -> String {
    let trimmed = url.trim();

    let (after_scheme, had_scheme) = match trimmed.find("://") {
        Some(idx) => (&trimmed[idx + 3..], true),
        None => (trimmed, false),
    };
    let after_user = match after_scheme.rfind('@') {
        Some(idx) => &after_scheme[idx + 1..],
        None => after_scheme,
    };

    let (host, path) = if had_scheme {
        split_on_first(after_user, '/')
    } else {
        // scp-like: `host:path` has no scheme; the first `:` is the host/path
        // separator. Fall back to `/` for bare hosts.
        match after_user.find(':') {
            Some(_) => split_on_first(after_user, ':'),
            None => split_on_first(after_user, '/'),
        }
    };
    let host_lc = host.to_ascii_lowercase();
    let path_clean = path.trim_end_matches('/').trim_end_matches(".git");

    if path_clean.is_empty() {
        host_lc
    } else {
        format!("{host_lc}/{path_clean}")
    }
}

fn split_on_first(s: &str, delim: char) -> (&str, &str) {
    match s.find(delim) {
        Some(idx) => (&s[..idx], &s[idx + 1..]),
        None => (s, ""),
    }
}

fn last_segment(normalized: &str) -> &str {
    match normalized.rsplit_once('/') {
        Some((_, last)) => last,
        None => "",
    }
}

fn sanitize_name(raw: &str) -> String {
    let mut out = String::with_capacity(raw.len());
    let mut prev_dash = false;
    for ch in raw.chars() {
        let mapped = match ch {
            'A'..='Z' => Some(ch.to_ascii_lowercase()),
            'a'..='z' | '0'..='9' | '_' | '-' => Some(ch),
            _ => None,
        };
        match mapped {
            Some(c) => {
                out.push(c);
                prev_dash = c == '-';
            }
            None => {
                if !prev_dash && !out.is_empty() {
                    out.push('-');
                    prev_dash = true;
                }
            }
        }
    }
    while out.ends_with('-') {
        out.pop();
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn https_url_derives_consistent_shorthash_and_name() {
        let a = CachePath::from_repo_url("https://github.com/edger-dev/kinora");
        let b = CachePath::from_repo_url("https://github.com/edger-dev/kinora");
        assert_eq!(a, b);
        assert_eq!(a.name, "kinora");
        assert_eq!(a.shorthash.len(), SHORTHASH_LEN);
    }

    #[test]
    fn strip_dot_git_suffix() {
        let a = CachePath::from_repo_url("https://github.com/edger-dev/kinora.git");
        let b = CachePath::from_repo_url("https://github.com/edger-dev/kinora");
        assert_eq!(a, b);
    }

    #[test]
    fn strip_trailing_slash() {
        let a = CachePath::from_repo_url("https://github.com/edger-dev/kinora/");
        let b = CachePath::from_repo_url("https://github.com/edger-dev/kinora");
        assert_eq!(a, b);
    }

    #[test]
    fn ssh_and_https_with_same_host_path_collide() {
        // Different transports to the same logical repo should produce the
        // same cache path — otherwise switching `git remote set-url` between
        // ssh and https would spawn a parallel cache tree.
        let https = CachePath::from_repo_url("https://github.com/edger-dev/kinora");
        let ssh = CachePath::from_repo_url("git@github.com:edger-dev/kinora.git");
        assert_eq!(https, ssh);
    }

    #[test]
    fn host_case_ignored() {
        let a = CachePath::from_repo_url("https://GitHub.com/edger-dev/kinora");
        let b = CachePath::from_repo_url("https://github.com/edger-dev/kinora");
        assert_eq!(a, b);
    }

    #[test]
    fn path_case_preserved() {
        let a = CachePath::from_repo_url("https://github.com/Edger-Dev/Kinora");
        let b = CachePath::from_repo_url("https://github.com/edger-dev/kinora");
        assert_ne!(a, b);
    }

    #[test]
    fn distinct_repos_get_distinct_shorthashes() {
        let a = CachePath::from_repo_url("https://github.com/edger-dev/kinora");
        let b = CachePath::from_repo_url("https://github.com/edger-dev/other");
        assert_ne!(a.shorthash, b.shorthash);
    }

    #[test]
    fn subdir_format_is_shorthash_dash_name() {
        let c = CachePath::from_repo_url("https://example.com/foo/bar");
        assert_eq!(c.subdir(), format!("{}-bar", c.shorthash));
    }

    #[test]
    fn subdir_without_name_falls_back_to_shorthash() {
        let c = CachePath::from_repo_url("https://example.com");
        assert_eq!(c.subdir(), c.shorthash);
    }

    #[test]
    fn name_sanitizes_unusual_characters() {
        let c = CachePath::from_repo_url("https://example.com/My Repo!");
        assert_eq!(c.name, "my-repo");
    }

    #[test]
    fn name_collapses_runs_and_trims_trailing_dash() {
        let c = CachePath::from_repo_url("https://example.com/foo---bar!!!");
        assert_eq!(c.name, "foo---bar");
    }

    #[test]
    fn name_is_last_path_segment() {
        let c = CachePath::from_repo_url("https://example.com/org/team/repo");
        assert_eq!(c.name, "repo");
    }

    #[test]
    fn git_plus_ssh_scheme_handled() {
        let a = CachePath::from_repo_url("git+ssh://git@github.com/edger-dev/kinora.git");
        let b = CachePath::from_repo_url("https://github.com/edger-dev/kinora");
        assert_eq!(a, b);
    }
}

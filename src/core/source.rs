use super::error::{Error, Result};
use std::path::PathBuf;

#[derive(Clone, PartialEq, Debug)]
pub struct SourceSpec {
    pub key: String,
    pub url: Option<String>,
    pub local_path: Option<PathBuf>,
}

pub fn parse_source(input: &str) -> Result<SourceSpec> {
    let input = input.trim().trim_end_matches('/');
    if input.is_empty() {
        return Err(Error::Msg("source 为空".into()));
    }
    // 本地绝对路径
    let p = PathBuf::from(input);
    if p.is_absolute() {
        let name = p
            .file_name()
            .map(|n| n.to_string_lossy().into_owned())
            .unwrap_or_else(|| "unnamed".into());
        return Ok(SourceSpec {
            key: format!("local/{name}"),
            url: None,
            local_path: Some(p),
        });
    }
    // SSH 形式 git@host:owner/repo[.git]
    if let Some(rest) = input.strip_prefix("git@") {
        if let Some((host, path)) = rest.split_once(':') {
            let path = path.trim_end_matches(".git");
            let short = if host == "github.com" { "github" } else { host };
            return Ok(SourceSpec {
                key: format!("{short}/{path}"),
                url: Some(input.into()),
                local_path: None,
            });
        }
        return Err(Error::Msg(format!("无法解析 source: {input}")));
    } // file:// URL（本地 bare 仓库，测试与离线场景）
    if let Some(rest) = input.strip_prefix("file://") {
        let trimmed = rest.trim_end_matches('/').trim_end_matches(".git");
        let parts: Vec<&str> = trimmed.rsplitn(3, '/').collect();
        let name = parts[0];
        let parent = parts.get(1).copied().unwrap_or("root");
        if name.is_empty() {
            return Err(Error::Msg(format!("无法解析 source: {input}")));
        }
        return Ok(SourceSpec {
            key: format!("file/{parent}/{name}"),
            url: Some(input.into()),
            local_path: None,
        });
    }
    // https://host/owner/repo[.git]
    if let Some(rest) = input
        .strip_prefix("https://")
        .or_else(|| input.strip_prefix("http://"))
    {
        let mut parts = rest.splitn(2, '/');
        let host = parts.next().unwrap_or_default();
        let path = parts
            .next()
            .map(|p| p.trim_end_matches('/').trim_end_matches(".git"));
        match (host, path) {
            (h, Some(p)) if !h.is_empty() && p.matches('/').count() == 1 => {
                let short = if h == "github.com" { "github" } else { h };
                return Ok(SourceSpec {
                    key: format!("{short}/{p}"),
                    url: Some(format!("https://{h}/{p}")),
                    local_path: None,
                });
            }
            _ => return Err(Error::Msg(format!("无法解析 source: {input}"))),
        }
    }
    // GitHub 简写 owner/repo
    if input.matches('/').count() == 1
        && input
            .chars()
            .all(|c| c.is_ascii_alphanumeric() || "-_./".contains(c))
    {
        return Ok(SourceSpec {
            key: format!("github/{input}"),
            url: Some(format!("https://github.com/{input}")),
            local_path: None,
        });
    }
    Err(Error::Msg(format!("无法解析 source: {input}")))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn github_shorthand_expands() {
        let s = parse_source("mattpocock/skills").unwrap();
        assert_eq!(s.key, "github/mattpocock/skills");
        assert_eq!(
            s.url.as_deref(),
            Some("https://github.com/mattpocock/skills")
        );
        assert!(s.local_path.is_none());
    }

    #[test]
    fn github_https_url() {
        let s = parse_source("https://github.com/mattpocock/skills").unwrap();
        assert_eq!(s.key, "github/mattpocock/skills");
    }

    #[test]
    fn github_https_url_with_git_suffix_and_trailing_slash() {
        let s = parse_source("https://github.com/a/b.git/").unwrap();
        assert_eq!(s.key, "github/a/b");
        assert_eq!(s.url.as_deref(), Some("https://github.com/a/b"));
    }

    #[test]
    fn ssh_url() {
        let s = parse_source("git@github.com:a/b.git").unwrap();
        assert_eq!(s.key, "github/a/b");
        assert_eq!(s.url.as_deref(), Some("git@github.com:a/b.git"));
    }

    #[test]
    fn non_github_host() {
        let s = parse_source("https://gitlab.com/org/repo").unwrap();
        assert_eq!(s.key, "gitlab.com/org/repo");
    }

    #[test]
    fn local_absolute_path() {
        let p = if cfg!(windows) {
            "C:\\tmp\\myskill"
        } else {
            "/tmp/myskill"
        };
        let s = parse_source(p).unwrap();
        assert!(s.key.starts_with("local/"));
        assert!(s.url.is_none());
        assert_eq!(s.local_path.as_deref(), Some(std::path::Path::new(p)));
    }

    #[test]
    fn file_url_supported_for_local_bare_repos() {
        let s = parse_source("file:///tmp/repos/bare.git").unwrap();
        assert_eq!(s.key, "file/repos/bare");
        assert_eq!(s.url.as_deref(), Some("file:///tmp/repos/bare.git"));
        assert!(s.local_path.is_none());
    }

    #[test]
    fn rejects_empty_and_single_word() {
        assert!(parse_source("").is_err());
        assert!(parse_source("noslash").is_err());
    }
}

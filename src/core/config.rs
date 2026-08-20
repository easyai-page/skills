use serde::Deserialize;
use std::collections::BTreeMap;
use std::path::PathBuf;

use super::error::Result;
use super::paths::Layout;
pub use super::registry::Method;

#[derive(Clone, Debug)]
pub struct Config {
    pub targets: BTreeMap<String, PathBuf>,
    pub default_method: Method,
    pub web_port: u16,
}

#[derive(Deserialize, Default)]
struct FileConfig {
    defaults: Option<FileDefaults>,
    web: Option<FileWeb>,
    targets: Option<BTreeMap<String, String>>,
}
#[derive(Deserialize)]
struct FileDefaults {
    method: Option<Method>,
}
#[derive(Deserialize)]
struct FileWeb {
    port: Option<u16>,
}

fn expand_tilde(s: &str) -> PathBuf {
    if let Some(rest) = s.strip_prefix("~/") {
        if let Some(home) = dirs::home_dir() {
            return home.join(rest);
        }
    }
    PathBuf::from(s)
}

impl Default for Config {
    fn default() -> Self {
        let mut targets = BTreeMap::new();
        if let Some(home) = dirs::home_dir() {
            targets.insert("agents".into(), home.join(".agents").join("skills"));
            targets.insert("claude".into(), home.join(".claude").join("skills"));
            targets.insert("codex".into(), home.join(".codex").join("skills"));
        }
        Config {
            targets,
            default_method: Method::Symlink,
            web_port: 7823,
        }
    }
}

impl Config {
    pub fn load(layout: &Layout) -> Result<Config> {
        let mut cfg = Config::default();
        let path = layout.config_path();
        if !path.exists() {
            return Ok(cfg); // 无配置文件也能工作
        }
        let fc: FileConfig = toml::from_str(&std::fs::read_to_string(&path)?)?;
        if let Some(d) = fc.defaults {
            if let Some(m) = d.method {
                cfg.default_method = m;
            }
        }
        if let Some(w) = fc.web {
            if let Some(p) = w.port {
                cfg.web_port = p;
            }
        }
        if let Some(t) = fc.targets {
            for (name, p) in t {
                cfg.targets.insert(name, expand_tilde(&p));
            }
        }
        Ok(cfg)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::core::paths::Layout;

    #[test]
    fn no_config_file_uses_builtin_defaults() {
        let tmp = tempfile::tempdir().unwrap();
        let cfg = Config::load(&Layout::at(tmp.path().to_path_buf())).unwrap();
        assert!(cfg.targets.contains_key("agents"));
        assert_eq!(cfg.web_port, 7823);
        assert_eq!(cfg.default_method, Method::Symlink);
    }

    #[test]
    fn config_file_overrides_and_extends() {
        let tmp = tempfile::tempdir().unwrap();
        std::fs::write(tmp.path().join("config.toml"), r#"
[defaults]
method = "copy"
[web]
port = 9000
[targets]
cursor = "~/.cursor/skills"
"#).unwrap();
        let cfg = Config::load(&Layout::at(tmp.path().to_path_buf())).unwrap();
        assert_eq!(cfg.default_method, Method::Copy);
        assert_eq!(cfg.web_port, 9000);
        assert!(cfg.targets.contains_key("cursor"));
        assert!(cfg.targets.contains_key("agents")); // 内置的还在
    }

    #[test]
    fn tilde_expands_in_targets() {
        let tmp = tempfile::tempdir().unwrap();
        std::fs::write(tmp.path().join("config.toml"),
            "[targets]\ncursor = \"~/.cursor/skills\"\n").unwrap();
        let cfg = Config::load(&Layout::at(tmp.path().to_path_buf())).unwrap();
        let p = &cfg.targets["cursor"];
        assert!(!p.to_string_lossy().contains('~'));
        assert!(p.is_absolute());
    }
}

use std::collections::BTreeMap;
use std::path::PathBuf;

use super::registry::Method;

pub struct Config {
    pub targets: BTreeMap<String, PathBuf>,
    pub default_method: Method,
    pub web_port: u16,
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

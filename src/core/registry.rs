use serde::{Deserialize, Serialize};
use std::collections::BTreeMap;
use std::path::PathBuf;

use super::error::Result;
use super::paths::Layout;

#[derive(Serialize, Deserialize, Clone, PartialEq, Debug)]
#[serde(tag = "kind", rename_all = "lowercase")]
pub enum TargetRec {
    Global { name: String },
    Project { root: PathBuf },
}

#[derive(Serialize, Deserialize, Clone, Copy, PartialEq, Eq, Debug)]
#[serde(rename_all = "lowercase")]
pub enum Method {
    Symlink,
    Copy,
}

#[derive(Serialize, Deserialize, Clone, Debug)]
pub struct SourceRecord {
    pub url: String,
    pub commit: String,
    pub fetched_at: String,
    #[serde(default)]
    pub auto_update: Option<bool>,
}

#[derive(Serialize, Deserialize, Clone, Debug)]
pub struct Install {
    pub skill: String,
    pub source: String,
    pub source_path: PathBuf,
    pub target: TargetRec,
    pub method: Method,
    pub commit: String,
    #[serde(default)]
    pub tags: Vec<String>,
    #[serde(default)]
    pub auto_update: Option<bool>,
    pub installed_at: String,
}

#[derive(Serialize, Deserialize, Default, Debug)]
pub struct Registry {
    pub version: u32,
    #[serde(default)]
    pub sources: BTreeMap<String, SourceRecord>,
    #[serde(default)]
    pub installs: Vec<Install>,
}

impl Registry {
    pub fn load(layout: &Layout) -> Result<Registry> {
        let path = layout.registry_path();
        if !path.exists() {
            return Ok(Registry {
                version: 1,
                ..Default::default()
            });
        }
        let raw = std::fs::read_to_string(&path)?;
        Ok(serde_json::from_str(&raw)?)
    }

    pub fn save(&self, layout: &Layout) -> Result<()> {
        let path = layout.registry_path();
        if let Some(parent) = path.parent() {
            std::fs::create_dir_all(parent)?;
        }
        let tmp = path.with_extension("json.tmp");
        std::fs::write(&tmp, serde_json::to_string_pretty(self)?)?;
        std::fs::rename(&tmp, &path)?; // 同目录 rename，原子替换
        Ok(())
    }

    pub fn find(&self, skill: &str, target: &TargetRec) -> Option<&Install> {
        self.installs
            .iter()
            .find(|i| i.skill == skill && &i.target == target)
    }

    pub fn remove(&mut self, skill: &str, target: &TargetRec) -> Option<Install> {
        let pos = self
            .installs
            .iter()
            .position(|i| i.skill == skill && &i.target == target)?;
        Some(self.installs.remove(pos))
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::core::paths::Layout;

    fn sample_install() -> Install {
        Install {
            skill: "web-design".into(),
            source: "github/mattpocock/skills".into(),
            source_path: "skills/web-design".into(),
            target: TargetRec::Global {
                name: "agents".into(),
            },
            method: Method::Copy,
            commit: "a1b2c3d".into(),
            tags: vec!["frontend".into()],
            auto_update: None,
            installed_at: "2026-08-20T10:00:00Z".into(),
        }
    }

    #[test]
    fn load_missing_returns_empty() {
        let tmp = tempfile::tempdir().unwrap();
        let reg = Registry::load(&Layout::at(tmp.path().to_path_buf())).unwrap();
        assert!(reg.installs.is_empty());
    }

    #[test]
    fn save_then_load_roundtrip() {
        let tmp = tempfile::tempdir().unwrap();
        let layout = Layout::at(tmp.path().to_path_buf());
        let mut reg = Registry {
            version: 1,
            ..Default::default()
        };
        reg.sources.insert(
            "github/a/b".into(),
            SourceRecord {
                url: "https://github.com/a/b".into(),
                commit: "deadbeef".into(),
                fetched_at: "2026-08-20T10:00:00Z".into(),
                auto_update: Some(true),
            },
        );
        reg.installs.push(sample_install());
        reg.save(&layout).unwrap();
        let loaded = Registry::load(&layout).unwrap();
        assert_eq!(loaded.installs.len(), 1);
        assert_eq!(loaded.installs[0].skill, "web-design");
        assert_eq!(loaded.sources["github/a/b"].commit, "deadbeef");
        // 落盘 JSON 与规格字段一致
        let raw = std::fs::read_to_string(layout.registry_path()).unwrap();
        assert!(raw.contains("\"kind\": \"global\""));
        assert!(raw.contains("\"method\": \"copy\""));
    }

    #[test]
    fn find_and_remove_by_skill_and_target() {
        let mut reg = Registry {
            version: 1,
            ..Default::default()
        };
        reg.installs.push(sample_install());
        let t = TargetRec::Global {
            name: "agents".into(),
        };
        assert!(reg.find("web-design", &t).is_some());
        assert!(
            reg.find(
                "web-design",
                &TargetRec::Global {
                    name: "claude".into()
                }
            )
            .is_none()
        );
        let removed = reg.remove("web-design", &t);
        assert!(removed.is_some());
        assert!(reg.installs.is_empty());
    }

    #[test]
    fn target_rec_serde_shape() {
        let g = serde_json::to_string(&TargetRec::Global {
            name: "agents".into(),
        })
        .unwrap();
        assert_eq!(g, r#"{"kind":"global","name":"agents"}"#);
        let p = serde_json::to_string(&TargetRec::Project { root: "/x".into() }).unwrap();
        assert_eq!(p, r#"{"kind":"project","root":"/x"}"#);
    }
}

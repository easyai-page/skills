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

#[derive(Serialize, Deserialize, Clone, Debug)]
pub struct FavSkill {
    pub name: String,
    pub description: String,
    pub source_path: PathBuf, // 相对缓存根；fav install 直接喂给 install_skill
}

#[derive(Serialize, Deserialize, Clone, Debug)]
pub struct Favorite {
    #[serde(default)]
    pub url: Option<String>, // git 源有；本地源为 None
    #[serde(default)]
    pub local_path: Option<PathBuf>, // 本地源有；fav install 据此重建 SourceSpec
    pub commit: String,        // 收藏时缓存 HEAD 快照（本地源为空串）
    pub bookmarked_at: String, // RFC3339
    #[serde(default)]
    pub skills: Vec<FavSkill>,
}

impl TargetRec {
    /// TargetRec 是持久化形态（serde 落盘），Target 是运行时形态（install/remove/update 用）；
    /// 转换逻辑收敛到这里，避免各调用点重复手写 match。
    pub fn to_target(&self) -> crate::core::paths::Target {
        match self {
            TargetRec::Global { name } => crate::core::paths::Target::Global { name: name.clone() },
            TargetRec::Project { root } => {
                crate::core::paths::Target::Project { root: root.clone() }
            }
        }
    }
}

#[derive(Serialize, Deserialize, Default, Debug)]
pub struct Registry {
    pub version: u32,
    #[serde(default)]
    pub sources: BTreeMap<String, SourceRecord>,
    #[serde(default)]
    pub installs: Vec<Install>,
    // 收藏夹：key 与 sources 同命名空间（github/o/r 或本地路径），与 installs 解耦——
    // 收藏不代表已安装，后续 bookmark/fav install 只读写这一段
    #[serde(default)]
    pub favorites: BTreeMap<String, Favorite>,
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

    #[test]
    fn legacy_registry_without_favorites_loads() {
        // 向后兼容：收藏功能上线前的旧版 registry.json 没有 favorites 段
        let tmp = tempfile::tempdir().unwrap();
        let layout = Layout::at(tmp.path().to_path_buf());
        std::fs::write(
            layout.registry_path(),
            r#"{"version":1,"sources":{},"installs":[]}"#,
        )
        .unwrap();
        let reg = Registry::load(&layout).unwrap();
        assert!(reg.favorites.is_empty());
    }

    #[test]
    fn favorites_roundtrip() {
        let tmp = tempfile::tempdir().unwrap();
        let layout = Layout::at(tmp.path().to_path_buf());
        let mut reg = Registry {
            version: 1,
            ..Default::default()
        };
        reg.favorites.insert(
            "github/o/r".into(),
            Favorite {
                url: Some("https://github.com/o/r".into()),
                local_path: None,
                commit: "deadbeef".into(),
                bookmarked_at: "2026-08-25T10:00:00Z".into(),
                skills: vec![FavSkill {
                    name: "a".into(),
                    description: "A".into(),
                    source_path: "skills/a".into(),
                }],
            },
        );
        reg.save(&layout).unwrap();
        let loaded = Registry::load(&layout).unwrap();
        let fav = &loaded.favorites["github/o/r"];
        assert_eq!(fav.skills[0].name, "a");
        assert_eq!(fav.skills[0].source_path, PathBuf::from("skills/a"));
        let raw = std::fs::read_to_string(layout.registry_path()).unwrap();
        assert!(raw.contains("\"favorites\""));
    }

    #[test]
    fn target_rec_to_target_maps_both_kinds() {
        assert_eq!(
            TargetRec::Global {
                name: "agents".into()
            }
            .to_target(),
            crate::core::paths::Target::Global {
                name: "agents".into()
            }
        );
        assert_eq!(
            TargetRec::Project {
                root: PathBuf::from("/x")
            }
            .to_target(),
            crate::core::paths::Target::Project {
                root: PathBuf::from("/x")
            }
        );
    }
}

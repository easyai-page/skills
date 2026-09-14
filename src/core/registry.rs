use serde::{Deserialize, Serialize};
use std::collections::BTreeMap;
use std::path::PathBuf;

use super::error::{Error, Result};
use super::paths::Layout;

#[derive(Serialize, Deserialize, Clone, PartialEq, Debug)]
#[serde(tag = "scope", rename_all = "lowercase")]
pub enum TargetRec {
    Global { agent: String },
    Project { agent: String, root: PathBuf },
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

impl Install {
    /// 判定该安装项在给定工作目录下是否可见（全局有效，或属于当前项目目录/子目录）
    pub fn is_visible_in_dir(&self, cwd: &std::path::Path) -> bool {
        self.target.is_visible_in_dir(cwd)
    }
}

impl TargetRec {
    /// TargetRec 是持久化形态（serde 落盘），Target 是运行时形态（install/remove/update 用）；
    /// 转换逻辑收敛到这里，避免各调用点重复手写 match。
    pub fn to_target(&self) -> crate::core::paths::Target {
        match self {
            TargetRec::Global { agent } => crate::core::paths::Target {
                agent: agent.clone(),
                scope: crate::core::paths::Scope::Global,
            },
            TargetRec::Project { agent, root } => crate::core::paths::Target {
                agent: agent.clone(),
                scope: crate::core::paths::Scope::Project(root.clone()),
            },
        }
    }

    /// 判定目标在给定工作目录下是否有效：
    /// - 全局目标：对所有路径均有效（true）
    /// - 项目目标：当且仅当当前工作目录处于项目的根目录或其子目录下时有效
    pub fn is_visible_in_dir(&self, cwd: &std::path::Path) -> bool {
        match self {
            TargetRec::Global { .. } => true,
            TargetRec::Project { root, .. } => {
                let norm_cwd = cwd.canonicalize().unwrap_or_else(|_| cwd.to_path_buf());
                let norm_root = root.canonicalize().unwrap_or_else(|_| root.clone());
                norm_cwd.starts_with(&norm_root) || cwd.starts_with(root)
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
    // 技能级标签：source → skill → tags。标签是技能的固有属性，与装到哪个目标
    // 无关，不存 install 记录
    #[serde(default)]
    pub skill_tags: BTreeMap<String, BTreeMap<String, Vec<String>>>,
}

impl Registry {
    pub fn load(layout: &Layout) -> Result<Registry> {
        let path = layout.registry_path();
        if !path.exists() {
            return Ok(Registry {
                version: 3,
                ..Default::default()
            });
        }
        let raw = std::fs::read_to_string(&path)?;
        let value: serde_json::Value = serde_json::from_str(&raw)?;
        // v3 起 target 改为 agent×scope 格式，旧版文件不做迁移，直接报错给出处置指引
        let version = value.get("version").and_then(|v| v.as_u64()).unwrap_or(0);
        if version != 3 {
            return Err(Error::Msg(format!(
                "registry.json 版本过旧（v{version}）：v3 起 target 改为 agent×scope 格式不做迁移，\
                 请备份后删除 {} 再重装技能",
                path.display()
            )));
        }
        let mut reg: Registry = serde_json::from_value(value)?;
        reg.version = 3;
        Ok(reg)
    }

    /// 技能级标签查询：绑定 (source, skill)，与安装目标无关；无标签返回空切片
    pub fn tags_of(&self, source: &str, skill: &str) -> &[String] {
        self.skill_tags
            .get(source)
            .and_then(|m| m.get(skill))
            .map(|v| v.as_slice())
            .unwrap_or(&[])
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

    #[allow(dead_code)]
    pub fn find_mut(&mut self, skill: &str, target: &TargetRec) -> Option<&mut Install> {
        self.installs
            .iter_mut()
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
                agent: "agents".into(),
            },
            method: Method::Copy,
            commit: "a1b2c3d".into(),
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
            version: 3,
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
        assert!(raw.contains("\"scope\": \"global\""));
        assert!(raw.contains("\"method\": \"copy\""));
    }

    #[test]
    fn find_and_remove_by_skill_and_target() {
        let mut reg = Registry {
            version: 3,
            ..Default::default()
        };
        reg.installs.push(sample_install());
        let t = TargetRec::Global {
            agent: "agents".into(),
        };
        assert!(reg.find("web-design", &t).is_some());
        assert!(
            reg.find(
                "web-design",
                &TargetRec::Global {
                    agent: "claude".into()
                }
            )
            .is_none()
        );
        let removed = reg.remove("web-design", &t);
        assert!(removed.is_some());
        assert!(reg.installs.is_empty());
    }

    #[test]
    fn skill_tags_roundtrip() {
        let tmp = tempfile::tempdir().unwrap();
        let layout = Layout::at(tmp.path().to_path_buf());
        let mut reg = Registry {
            version: 3,
            ..Default::default()
        };
        reg.skill_tags
            .entry("github/o/r".into())
            .or_default()
            .insert("a".into(), vec!["frontend".into()]);
        reg.save(&layout).unwrap();
        let loaded = Registry::load(&layout).unwrap();
        assert_eq!(loaded.tags_of("github/o/r", "a"), &["frontend"]);
        assert!(loaded.tags_of("github/o/r", "b").is_empty());
        assert!(loaded.tags_of("github/x/y", "a").is_empty());
    }

    #[test]
    fn target_rec_v3_roundtrip() {
        let rec = TargetRec::Global {
            agent: "claude-code".into(),
        };
        let v = serde_json::to_value(&rec).unwrap();
        assert_eq!(
            v,
            serde_json::json!({"scope": "global", "agent": "claude-code"})
        );
        let back: TargetRec = serde_json::from_value(v).unwrap();
        assert_eq!(back, rec);
        let p = TargetRec::Project {
            agent: "codex".into(),
            root: PathBuf::from("/x"),
        };
        let back: TargetRec = serde_json::from_str(&serde_json::to_string(&p).unwrap()).unwrap();
        assert_eq!(back, p);
    }

    #[test]
    fn load_rejects_legacy_version() {
        let tmp = tempfile::tempdir().unwrap();
        let layout = Layout::at(tmp.path().to_path_buf());
        // v2 旧格式（target 带 kind 标签）必须被拒绝，不得静默吞掉
        std::fs::write(layout.registry_path(), r#"{"version":2,"sources":{},"installs":[{"skill":"a","source":"s","source_path":"s","target":{"kind":"global","name":"agents"},"method":"copy","commit":"c","installed_at":"t"}]}"#).unwrap();
        let err = Registry::load(&layout).unwrap_err();
        assert!(err.to_string().contains("registry.json"), "{err}");
    }

    #[test]
    fn favorites_roundtrip() {
        let tmp = tempfile::tempdir().unwrap();
        let layout = Layout::at(tmp.path().to_path_buf());
        let mut reg = Registry {
            version: 3,
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
                agent: "agents".into()
            }
            .to_target(),
            crate::core::paths::Target {
                agent: "agents".into(),
                scope: crate::core::paths::Scope::Global,
            }
        );
        assert_eq!(
            TargetRec::Project {
                agent: "agents".into(),
                root: PathBuf::from("/x")
            }
            .to_target(),
            crate::core::paths::Target {
                agent: "agents".into(),
                scope: crate::core::paths::Scope::Project(PathBuf::from("/x")),
            }
        );
    }

    #[test]
    fn is_visible_in_dir_global_and_project() {
        let global = TargetRec::Global {
            agent: "agents".into(),
        };
        assert!(global.is_visible_in_dir(std::path::Path::new("/any/path")));

        let proj = TargetRec::Project {
            agent: "agents".into(),
            root: PathBuf::from("/home/user/projectA"),
        };
        // 相同目录
        assert!(proj.is_visible_in_dir(std::path::Path::new("/home/user/projectA")));
        // 子目录
        assert!(proj.is_visible_in_dir(std::path::Path::new("/home/user/projectA/src/components")));
        // 外部路径 / 父目录
        assert!(!proj.is_visible_in_dir(std::path::Path::new("/home/user")));
        // 另一项目
        assert!(!proj.is_visible_in_dir(std::path::Path::new("/home/user/projectB")));
        // 类似前缀但不是子目录组件
        assert!(!proj.is_visible_in_dir(std::path::Path::new("/home/user/projectA_backup")));
    }
}

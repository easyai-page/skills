use std::path::{Path, PathBuf};

use super::cache::copy_dir;
use super::config::Config;
use super::error::{Error, Result};
use super::paths::{Layout, Target};
use super::registry::{Install, Method, Registry, TargetRec};

/// 把技能从缓存安装到一个目标；目标已有同名目录时返回 Error::Conflict 交由前端决策。
pub fn install_skill(
    layout: &Layout,
    cfg: &Config,
    reg: &mut Registry,
    source_key: &str,
    skill: &str,
    source_path: &str,
    target: &Target,
    method: Method,
    commit: &str,
) -> Result<Install> {
    let src_dir = layout.cache_dir(source_key).join(source_path);
    let dest_root = target.install_dir(cfg)?;
    let dest = dest_root.join(skill);
    if dest.exists() || dest.symlink_metadata().is_ok() {
        return Err(Error::Conflict(dest));
    }
    std::fs::create_dir_all(&dest_root)?;
    match method {
        Method::Copy => copy_dir(&src_dir, &dest)?,
        Method::Symlink => make_symlink(&src_dir, &dest)?,
    }
    let rec = Install {
        skill: skill.into(),
        source: source_key.into(),
        source_path: PathBuf::from(source_path),
        target: to_rec(target),
        method,
        commit: commit.into(),
        tags: vec![],
        auto_update: None,
        installed_at: chrono::Utc::now().to_rfc3339(),
    };
    reg.installs.push(rec.clone());
    Ok(rec)
}

pub fn to_rec(target: &Target) -> TargetRec {
    match target {
        Target::Global { name } => TargetRec::Global { name: name.clone() },
        Target::Project { root } => TargetRec::Project { root: root.clone() },
    }
}

#[cfg(unix)]
fn make_symlink(src: &Path, dst: &Path) -> Result<()> {
    std::os::unix::fs::symlink(src, dst)?;
    Ok(())
}

#[cfg(windows)]
fn make_symlink(src: &Path, dst: &Path) -> Result<()> {
    if std::os::windows::fs::symlink_dir(src, dst).is_ok() {
        return Ok(());
    }
    junction::create(src, dst).map_err(|err| {
        Error::Msg(format!(
            "创建链接失败（{}）：请用 --method copy，或开启 Windows 开发者模式",
            err
        ))
    })?;
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::core::config::Config;
    use crate::core::error::Error;
    use crate::core::paths::{Layout, Target};
    use crate::core::registry::{Method, Registry, TargetRec};

    /// 构造：缓存里一个技能包（技能 alpha），Config 的 agents target 指向临时目录
    fn setup() -> (tempfile::TempDir, Layout, Config, Registry) {
        let tmp = tempfile::tempdir().unwrap();
        let layout = Layout::at(tmp.path().join(".skills"));
        let cache = layout.cache_dir("github/o/r");
        std::fs::create_dir_all(cache.join("skills/alpha")).unwrap();
        std::fs::create_dir_all(cache.join(".git")).unwrap();
        std::fs::write(
            cache.join("skills/alpha/SKILL.md"),
            "---\nname: alpha\ndescription: A\n---\n",
        )
        .unwrap();
        let mut cfg = Config::default();
        cfg.targets
            .insert("agents".into(), tmp.path().join("global/agents"));
        (
            tmp,
            layout,
            cfg,
            Registry {
                version: 1,
                ..Default::default()
            },
        )
    }

    #[test]
    fn copy_install_creates_independent_copy_and_record() {
        let (_t, layout, cfg, mut reg) = setup();
        let target = Target::Global {
            name: "agents".into(),
        };
        let recs = install_skill(
            &layout,
            &cfg,
            &mut reg,
            "github/o/r",
            "alpha",
            "skills/alpha",
            &target,
            Method::Copy,
            "c1",
        )
        .unwrap();
        let dest = cfg.targets["agents"].join("alpha");
        assert!(dest.join("SKILL.md").exists());
        assert!(!dest.symlink_metadata().unwrap().file_type().is_symlink());
        assert_eq!(recs.method, Method::Copy);
        assert_eq!(reg.installs.len(), 1);
        assert_eq!(
            reg.installs[0].target,
            TargetRec::Global {
                name: "agents".into()
            }
        );
    }

    #[cfg(unix)]
    #[test]
    fn symlink_install_points_into_cache() {
        let (_t, layout, cfg, mut reg) = setup();
        let target = Target::Global {
            name: "agents".into(),
        };
        install_skill(
            &layout,
            &cfg,
            &mut reg,
            "github/o/r",
            "alpha",
            "skills/alpha",
            &target,
            Method::Symlink,
            "c1",
        )
        .unwrap();
        let dest = cfg.targets["agents"].join("alpha");
        let link = std::fs::read_link(&dest).unwrap();
        assert_eq!(link, layout.cache_dir("github/o/r").join("skills/alpha"));
        assert_eq!(reg.installs[0].method, Method::Symlink);
    }

    #[test]
    fn conflict_returns_decision_request() {
        let (_t, layout, cfg, mut reg) = setup();
        let target = Target::Global {
            name: "agents".into(),
        };
        install_skill(
            &layout,
            &cfg,
            &mut reg,
            "github/o/r",
            "alpha",
            "skills/alpha",
            &target,
            Method::Copy,
            "c1",
        )
        .unwrap();
        // 再装同名技能 -> 返回冲突，由调用方决定
        let err = install_skill(
            &layout,
            &cfg,
            &mut reg,
            "github/o/r",
            "alpha",
            "skills/alpha",
            &target,
            Method::Copy,
            "c1",
        )
        .unwrap_err();
        assert!(matches!(err, Error::Conflict(_)));
    }

    #[test]
    fn same_skill_to_two_targets_creates_two_records() {
        let (_t, layout, mut cfg, mut reg) = setup();
        cfg.targets
            .insert("claude".into(), _t.path().join("global/claude"));
        for name in ["agents", "claude"] {
            install_skill(
                &layout,
                &cfg,
                &mut reg,
                "github/o/r",
                "alpha",
                "skills/alpha",
                &Target::Global { name: name.into() },
                Method::Copy,
                "c1",
            )
            .unwrap();
        }
        assert_eq!(reg.installs.len(), 2);
    }
}

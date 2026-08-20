use super::config::Config;
use super::error::{Error, Result};
use super::paths::{Layout, Target};
use super::registry::{Method, Registry, TargetRec};

#[derive(PartialEq, Debug)]
pub enum RemoveOutcome {
    Removed,
    RecordOnly,
}

/// 按记录删除：先查 registry，再核实磁盘实况，一致才删。
pub fn remove_install(
    layout: &Layout,
    cfg: &Config,
    reg: &mut Registry,
    skill: &str,
    target: &TargetRec,
) -> Result<RemoveOutcome> {
    let rec = reg
        .find(skill, target)
        .ok_or_else(|| Error::NotInstalled(format!("{skill} @ {target:?}")))?
        .clone();
    let target = match target {
        TargetRec::Global { name } => Target::Global { name: name.clone() },
        TargetRec::Project { root } => Target::Project { root: root.clone() },
    };
    let dest = target.install_dir(cfg)?.join(skill);

    match (rec.method, dest.symlink_metadata()) {
        (_, Err(_)) => {
            reg.remove(skill, &rec.target);
            Ok(RemoveOutcome::RecordOnly)
        }
        (Method::Symlink, Ok(metadata)) if metadata.file_type().is_symlink() => {
            let link = std::fs::read_link(&dest)?;
            let expected = layout.cache_dir(&rec.source).join(&rec.source_path);
            if link != expected {
                return Err(Error::Mismatch(format!(
                    "{dest:?} 指向 {link:?}，与记录不符，已保留"
                )));
            }
            std::fs::remove_file(&dest)?;
            reg.remove(skill, &rec.target);
            Ok(RemoveOutcome::Removed)
        }
        (Method::Copy, Ok(metadata)) if metadata.is_dir() && !metadata.file_type().is_symlink() => {
            std::fs::remove_dir_all(&dest)?;
            reg.remove(skill, &rec.target);
            Ok(RemoveOutcome::Removed)
        }
        (_, Ok(_)) => Err(Error::Mismatch(format!(
            "{dest:?} 实况与安装方式 {:?} 不符，已保留",
            rec.method
        ))),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::core::paths::Target;

    fn setup_installed(method: Method) -> (tempfile::TempDir, Layout, Config, Registry) {
        let tmp = tempfile::tempdir().unwrap();
        let layout = Layout::at(tmp.path().join(".skills"));
        let cache = layout.cache_dir("github/o/r");
        std::fs::create_dir_all(cache.join("skills/alpha")).unwrap();
        std::fs::write(
            cache.join("skills/alpha/SKILL.md"),
            "---\nname: alpha\n---\n",
        )
        .unwrap();
        let mut cfg = Config::default();
        cfg.targets
            .insert("agents".into(), tmp.path().join("g/agents"));
        let mut reg = Registry {
            version: 1,
            ..Default::default()
        };
        crate::core::install::install_skill(
            &layout,
            &cfg,
            &mut reg,
            "github/o/r",
            "alpha",
            "skills/alpha",
            &Target::Global {
                name: "agents".into(),
            },
            method,
            "c1",
        )
        .unwrap();
        (tmp, layout, cfg, reg)
    }

    #[test]
    fn remove_copy_deletes_dir_and_record() {
        let (_t, layout, cfg, mut reg) = setup_installed(Method::Copy);
        let dest = cfg.targets["agents"].join("alpha");
        let outcome = remove_install(
            &layout,
            &cfg,
            &mut reg,
            "alpha",
            &TargetRec::Global {
                name: "agents".into(),
            },
        )
        .unwrap();
        assert_eq!(outcome, RemoveOutcome::Removed);
        assert!(!dest.exists());
        assert!(reg.installs.is_empty());
    }

    #[cfg(unix)]
    #[test]
    fn remove_symlink_only_removes_link_not_cache() {
        let (_t, layout, cfg, mut reg) = setup_installed(Method::Symlink);
        let dest = cfg.targets["agents"].join("alpha");
        remove_install(
            &layout,
            &cfg,
            &mut reg,
            "alpha",
            &TargetRec::Global {
                name: "agents".into(),
            },
        )
        .unwrap();
        assert!(dest.symlink_metadata().is_err());
        assert!(
            layout
                .cache_dir("github/o/r/skills/alpha/SKILL.md")
                .exists()
        );
    }

    #[test]
    fn remove_when_dir_manually_deleted_cleans_record_only() {
        let (_t, layout, cfg, mut reg) = setup_installed(Method::Copy);
        std::fs::remove_dir_all(cfg.targets["agents"].join("alpha")).unwrap();
        let outcome = remove_install(
            &layout,
            &cfg,
            &mut reg,
            "alpha",
            &TargetRec::Global {
                name: "agents".into(),
            },
        )
        .unwrap();
        assert_eq!(outcome, RemoveOutcome::RecordOnly);
        assert!(reg.installs.is_empty());
    }

    #[test]
    fn remove_unknown_returns_not_installed() {
        let (_t, layout, cfg, mut reg) = setup_installed(Method::Copy);
        let err = remove_install(
            &layout,
            &cfg,
            &mut reg,
            "nope",
            &TargetRec::Global {
                name: "agents".into(),
            },
        )
        .unwrap_err();
        assert!(matches!(err, Error::NotInstalled(_)));
    }

    #[test]
    fn remove_verifies_symlink_points_to_cache() {
        let (_t, layout, cfg, mut reg) = setup_installed(Method::Symlink);
        let dest = cfg.targets["agents"].join("alpha");
        #[cfg(unix)]
        {
            std::fs::remove_file(&dest).unwrap();
            std::fs::create_dir_all(&dest).unwrap();
            let err = remove_install(
                &layout,
                &cfg,
                &mut reg,
                "alpha",
                &TargetRec::Global {
                    name: "agents".into(),
                },
            )
            .unwrap_err();
            assert!(matches!(err, Error::Mismatch(_)));
            assert!(dest.exists());
            assert_eq!(reg.installs.len(), 1);
        }
    }
}

use std::io::ErrorKind;
use std::path::{Path, PathBuf};

use super::config::Config;
use super::error::{Error, Result};
use super::install::{self, COPY_MANIFEST};
use super::paths::{Layout, Target};
use super::registry::{Install, Method, Registry, TargetRec};

#[derive(PartialEq, Debug)]
pub enum RemoveOutcome {
    Removed,
    RecordOnly,
}

/// 按记录删除：先查 registry 并校验记录合法性，再核实磁盘实况，一致才删。
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
    validate_record(&rec)?;

    let t = match &rec.target {
        TargetRec::Global { name } => Target::Global { name: name.clone() },
        TargetRec::Project { root } => Target::Project { root: root.clone() },
    };
    let dest = t.install_dir(cfg)?.join(&rec.skill);

    let meta = match std::fs::symlink_metadata(&dest) {
        Ok(meta) => meta,
        Err(err) if err.kind() == ErrorKind::NotFound => {
            // 仅 NotFound 视为“磁盘已不存在”→ 只清记录
            reg.remove(&rec.skill, &rec.target);
            return Ok(RemoveOutcome::RecordOnly);
        }
        // 权限/I/O 等其他错误：保留记录，不动磁盘
        Err(err) => return Err(Error::Io(err)),
    };

    match rec.method {
        Method::Symlink => remove_recorded_link(layout, &dest, &meta, &rec)?,
        Method::Copy => remove_recorded_copy(&dest, &meta, &rec)?,
    }
    reg.remove(&rec.skill, &rec.target);
    Ok(RemoveOutcome::Removed)
}

/// 删除/更新前校验记录：技能名必须是单一 Normal 组件（复用 install 的校验函数），
/// project root 必须是绝对路径。非法记录视为损坏，返回 Mismatch，不执行任何磁盘删除。
pub(crate) fn validate_record(rec: &Install) -> Result<()> {
    install::validate_skill_name(&rec.skill).map_err(|_| {
        Error::Mismatch(format!(
            "安装记录损坏：技能名 {:?} 非法，未执行磁盘删除",
            rec.skill
        ))
    })?;
    if let TargetRec::Project { root } = &rec.target {
        if !root.is_absolute() {
            return Err(Error::Mismatch(format!(
                "安装记录损坏：project root {root:?} 不是绝对路径，未执行磁盘删除"
            )));
        }
    }
    Ok(())
}

fn remove_recorded_link(
    layout: &Layout,
    dest: &Path,
    meta: &std::fs::Metadata,
    rec: &Install,
) -> Result<()> {
    if !meta.file_type().is_symlink() && !is_junction(dest) {
        return Err(Error::Mismatch(format!(
            "{dest:?} 不是链接，与安装方式 {:?} 不符，已保留",
            rec.method
        )));
    }
    let actual = read_link_target(dest, meta)?;
    let expected = layout.cache_dir(&rec.source).join(&rec.source_path);
    if !same_link_target(&actual, &expected) {
        return Err(Error::Mismatch(format!(
            "{dest:?} 指向 {actual:?}，与记录不符，已保留"
        )));
    }
    remove_link_itself(dest, meta)
}

fn remove_recorded_copy(dest: &Path, meta: &std::fs::Metadata, rec: &Install) -> Result<()> {
    verify_copy_ownership(dest, meta, rec)?;
    std::fs::remove_dir_all(dest)?;
    Ok(())
}

/// 核验 copy 副本实况与记录一致（真目录、非链接）且带所有权标识；
/// 不符返回 Mismatch。remove 与 update 共用此前置校验，无法确认归属时不得动磁盘。
pub(crate) fn verify_copy_ownership(
    dest: &Path,
    meta: &std::fs::Metadata,
    rec: &Install,
) -> Result<()> {
    if !meta.is_dir() || meta.file_type().is_symlink() || is_junction(dest) {
        return Err(Error::Mismatch(format!(
            "{dest:?} 实况与安装方式 {:?} 不符，已保留",
            rec.method
        )));
    }
    // 只删除带所有权标识的副本：用户删掉副本后他人在同路径重建的目录不得误删
    match std::fs::symlink_metadata(dest.join(COPY_MANIFEST)) {
        Ok(marker) if marker.is_file() => {}
        Ok(_) => {
            return Err(Error::Mismatch(format!(
                "{dest:?} 中的 {COPY_MANIFEST} 不是文件，无法确认副本归属，已保留"
            )));
        }
        Err(err) if err.kind() == ErrorKind::NotFound => {
            return Err(Error::Mismatch(format!(
                "{dest:?} 缺少所有权标识 {COPY_MANIFEST}，非本工具安装的副本（或为旧版安装），已保留"
            )));
        }
        Err(err) => return Err(Error::Io(err)),
    }
    Ok(())
}

/// Windows junction 是挂载点型 reparse point，按链接语义识别。
#[cfg(windows)]
fn is_junction(dest: &Path) -> bool {
    junction::exists(dest).unwrap_or(false)
}

#[cfg(not(windows))]
fn is_junction(_dest: &Path) -> bool {
    false
}

#[cfg(windows)]
fn read_link_target(dest: &Path, meta: &std::fs::Metadata) -> Result<PathBuf> {
    if meta.file_type().is_symlink() && !is_junction(dest) {
        Ok(std::fs::read_link(dest)?)
    } else {
        Ok(junction::get_target(dest)?)
    }
}

#[cfg(not(windows))]
fn read_link_target(dest: &Path, _meta: &std::fs::Metadata) -> Result<PathBuf> {
    Ok(std::fs::read_link(dest)?)
}

/// junction 的 read_link/get_target 可能带 `\\?\` 前缀，规范化后比较。
#[cfg(windows)]
fn same_link_target(actual: &Path, expected: &Path) -> bool {
    let norm = |p: &Path| std::fs::canonicalize(p).unwrap_or_else(|_| p.to_path_buf());
    norm(actual) == norm(expected)
}

#[cfg(not(windows))]
fn same_link_target(actual: &Path, expected: &Path) -> bool {
    actual == expected
}

#[cfg(windows)]
fn remove_link_itself(dest: &Path, _meta: &std::fs::Metadata) -> Result<()> {
    // 我们的链接只指向缓存目录（目录软链接或 junction）：
    // RemoveDirectory 只删除 reparse point 本身，不会递归进缓存源。
    std::fs::remove_dir(dest)?;
    Ok(())
}

#[cfg(not(windows))]
fn remove_link_itself(dest: &Path, _meta: &std::fs::Metadata) -> Result<()> {
    std::fs::remove_file(dest)?;
    Ok(())
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

    fn corrupt_install(skill: &str, target: TargetRec) -> Install {
        Install {
            skill: skill.into(),
            source: "github/o/r".into(),
            source_path: "skills/alpha".into(),
            target,
            method: Method::Copy,
            commit: "c1".into(),
            tags: vec![],
            auto_update: None,
            installed_at: "2026-08-20T10:00:00Z".into(),
        }
    }

    #[test]
    fn remove_copy_refuses_foreign_dir_recreated_without_marker() {
        // 用户删掉副本后，同路径被重建为外部目录 → 不得误删
        let (_t, layout, cfg, mut reg) = setup_installed(Method::Copy);
        let dest = cfg.targets["agents"].join("alpha");
        std::fs::remove_dir_all(&dest).unwrap();
        std::fs::create_dir_all(&dest).unwrap();
        std::fs::write(dest.join("user-file"), "mine").unwrap();

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
        assert_eq!(
            std::fs::read_to_string(dest.join("user-file")).unwrap(),
            "mine"
        );
        assert_eq!(reg.installs.len(), 1);
    }

    #[test]
    fn remove_preserves_record_when_metadata_fails() {
        // symlink_metadata 的非 NotFound 错误（这里是 ENOTDIR）→ Error::Io，记录保留
        let (t, layout, mut cfg, mut reg) = setup_installed(Method::Copy);
        let blocker = t.path().join("blocker");
        std::fs::write(&blocker, "not a dir").unwrap();
        cfg.targets.insert("agents".into(), blocker);

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

        assert!(matches!(err, Error::Io(_)));
        assert_eq!(reg.installs.len(), 1);
    }

    #[test]
    fn remove_rejects_invalid_skill_record_without_disk_delete() {
        let (_t, layout, cfg, mut reg) = setup_installed(Method::Copy);
        let escaped = cfg.targets["agents"].parent().unwrap().join("escaped");
        std::fs::create_dir_all(&escaped).unwrap();
        std::fs::write(escaped.join("keep"), "keep").unwrap();
        reg.installs.push(corrupt_install(
            "../escaped",
            TargetRec::Global {
                name: "agents".into(),
            },
        ));

        let err = remove_install(
            &layout,
            &cfg,
            &mut reg,
            "../escaped",
            &TargetRec::Global {
                name: "agents".into(),
            },
        )
        .unwrap_err();

        assert!(matches!(err, Error::Mismatch(_)));
        assert_eq!(reg.installs.len(), 2);
        assert!(escaped.join("keep").exists());
        assert!(cfg.targets["agents"].join("alpha").exists());
    }

    #[test]
    fn remove_rejects_relative_project_root_record() {
        let (_t, layout, cfg, mut reg) = setup_installed(Method::Copy);
        reg.installs.push(corrupt_install(
            "alpha",
            TargetRec::Project {
                root: "relative/root".into(),
            },
        ));

        let err = remove_install(
            &layout,
            &cfg,
            &mut reg,
            "alpha",
            &TargetRec::Project {
                root: "relative/root".into(),
            },
        )
        .unwrap_err();

        assert!(matches!(err, Error::Mismatch(_)));
        assert_eq!(reg.installs.len(), 2);
        assert!(cfg.targets["agents"].join("alpha").exists());
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

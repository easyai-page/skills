use std::path::{Component, Path, PathBuf};
use std::sync::atomic::{AtomicU64, Ordering};

use super::cache::copy_dir;
use super::config::Config;
use super::error::{Error, Result};
use super::paths::{Layout, Target};
use super::registry::{Install, Method, Registry, TargetRec};

static STAGE_COUNTER: AtomicU64 = AtomicU64::new(0);

/// copy 副本内的所有权标识文件名。与计划任务 15 的 `.skills-manifest` 约定同名：
/// 任务 15 会扩展该文件写入文件名+sha256 清单；remove 只依赖其存在性确认副本归属。
pub(crate) const COPY_MANIFEST: &str = ".skills-manifest";

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
    validate_skill_name(skill)?;
    let src_dir = resolve_source_dir(layout, source_key, source_path)?;
    let dest_root = target.install_dir(cfg)?;
    let dest = dest_root.join(skill);
    ensure_destination_absent(&dest)?;
    std::fs::create_dir_all(&dest_root)?;

    match method {
        Method::Copy => copy_install(&src_dir, &dest_root, &dest)?,
        Method::Symlink => {
            ensure_destination_absent(&dest)?;
            make_symlink(&src_dir, &dest).map_err(|err| match err {
                Error::Io(io) if io.kind() == std::io::ErrorKind::AlreadyExists => {
                    Error::Conflict(dest.clone())
                }
                err => err,
            })?;
        }
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

pub(crate) fn validate_skill_name(skill: &str) -> Result<()> {
    let path = Path::new(skill);
    let mut components = path.components();
    if skill.contains('/')
        || skill.contains('\\')
        || !matches!(
            (components.next(), components.next()),
            (Some(Component::Normal(_)), None)
        )
    {
        return Err(Error::InvalidSkillName(skill.into()));
    }
    Ok(())
}

fn resolve_source_dir(layout: &Layout, source_key: &str, source_path: &str) -> Result<PathBuf> {
    let relative = Path::new(source_path);
    if relative.is_absolute()
        || relative.components().any(|component| {
            matches!(
                component,
                Component::RootDir | Component::Prefix(_) | Component::ParentDir
            )
        })
    {
        return Err(Error::InvalidSourcePath(relative.to_path_buf()));
    }

    let cache_path = layout.cache_dir(source_key);
    let cache_root = canonicalize_source_path(&cache_path)?;
    let candidate = cache_path.join(relative);
    let resolved = canonicalize_source_path(&candidate)?;
    if !resolved.starts_with(&cache_root) {
        return Err(Error::InvalidSourcePath(relative.to_path_buf()));
    }
    if !resolved.is_dir() {
        return Err(Error::SourceNotDirectory(candidate));
    }
    Ok(candidate)
}

fn canonicalize_source_path(path: &Path) -> Result<PathBuf> {
    match std::fs::canonicalize(path) {
        Ok(path) => Ok(path),
        Err(err) if err.kind() == std::io::ErrorKind::NotFound => {
            Err(Error::SourceNotDirectory(path.to_path_buf()))
        }
        Err(err) => Err(Error::Io(err)),
    }
}

fn ensure_destination_absent(dest: &Path) -> Result<()> {
    match std::fs::symlink_metadata(dest) {
        Ok(_) => Err(Error::Conflict(dest.to_path_buf())),
        Err(err) if err.kind() == std::io::ErrorKind::NotFound => Ok(()),
        Err(err) => Err(Error::Io(err)),
    }
}

fn copy_install(src: &Path, dest_root: &Path, dest: &Path) -> Result<()> {
    let stage = create_staging_dir(dest_root, dest.file_name().unwrap_or_default())?;
    let result = (|| {
        copy_dir(src, &stage)?;
        write_copy_manifest(&stage)?;
        ensure_destination_absent(dest)?;
        commit_staging_dir(&stage, dest)
    })();

    if result.is_err() {
        let _ = remove_install_path(&stage);
    }
    result
}

/// 在暂存副本内写入所有权标识，随 rename 原子生效。
fn write_copy_manifest(stage: &Path) -> Result<()> {
    let body = serde_json::json!({ "version": 1, "manager": "skills" });
    std::fs::write(
        stage.join(COPY_MANIFEST),
        serde_json::to_string_pretty(&body)?,
    )?;
    Ok(())
}

#[cfg(unix)]
fn commit_staging_dir(stage: &Path, dest: &Path) -> Result<()> {
    match std::fs::create_dir(dest) {
        Ok(()) => {}
        Err(err) if err.kind() == std::io::ErrorKind::AlreadyExists => {
            return Err(Error::Conflict(dest.to_path_buf()));
        }
        Err(err) => return Err(Error::Io(err)),
    }

    match std::fs::rename(stage, dest) {
        Ok(()) => Ok(()),
        Err(err) => {
            let _ = std::fs::remove_dir(dest);
            Err(Error::Io(err))
        }
    }
}

#[cfg(not(unix))]
fn commit_staging_dir(stage: &Path, dest: &Path) -> Result<()> {
    match std::fs::rename(stage, dest) {
        Ok(()) => Ok(()),
        Err(err) if std::fs::symlink_metadata(dest).is_ok() => {
            Err(Error::Conflict(dest.to_path_buf()))
        }
        Err(err) => Err(Error::Io(err)),
    }
}

fn create_staging_dir(dest_root: &Path, skill: &std::ffi::OsStr) -> Result<PathBuf> {
    let pid = std::process::id();
    loop {
        let sequence = STAGE_COUNTER.fetch_add(1, Ordering::Relaxed);
        let name = format!(
            ".{}-install-{}-{}.tmp",
            skill.to_string_lossy(),
            pid,
            sequence
        );
        let stage = dest_root.join(name);
        match std::fs::create_dir(&stage) {
            Ok(()) => return Ok(stage),
            Err(err) if err.kind() == std::io::ErrorKind::AlreadyExists => continue,
            Err(err) => return Err(Error::Io(err)),
        }
    }
}

fn remove_install_path(path: &Path) -> std::io::Result<()> {
    let metadata = match std::fs::symlink_metadata(path) {
        Ok(metadata) => metadata,
        Err(err) if err.kind() == std::io::ErrorKind::NotFound => return Ok(()),
        Err(err) => return Err(err),
    };
    if metadata.is_dir() {
        std::fs::remove_dir_all(path)
    } else {
        std::fs::remove_file(path)
    }
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
    if std::fs::symlink_metadata(dst).is_ok() {
        return Err(Error::Conflict(dst.to_path_buf()));
    }
    junction::create(src, dst).map_err(|err| {
        if std::fs::symlink_metadata(dst).is_ok() {
            Error::Conflict(dst.to_path_buf())
        } else {
            Error::Msg(format!(
                "创建链接失败（{}）：请用 --method copy，或开启 Windows 开发者模式",
                err
            ))
        }
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

    #[test]
    fn project_target_installs_under_dot_agents_skills() {
        let (tmp, layout, cfg, mut reg) = setup();
        let project_root = tmp.path().join("project");
        let target = Target::Project {
            root: project_root.clone(),
        };

        let rec = install_skill(
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

        assert!(project_root.join(".agents/skills/alpha/SKILL.md").is_file());
        assert_eq!(rec.target, TargetRec::Project { root: project_root });
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

    #[test]
    fn commit_does_not_replace_a_competing_destination() {
        let root = tempfile::tempdir().unwrap();
        let stage = root.path().join("stage");
        let dest = root.path().join("alpha");
        std::fs::create_dir(&stage).unwrap();
        std::fs::write(stage.join("SKILL.md"), "staged").unwrap();
        std::fs::create_dir(&dest).unwrap();
        std::fs::write(dest.join("owned-by-other"), "keep").unwrap();

        let err = commit_staging_dir(&stage, &dest).unwrap_err();

        assert!(matches!(err, Error::Conflict(_)));
        assert_eq!(
            std::fs::read_to_string(dest.join("owned-by-other")).unwrap(),
            "keep"
        );
        assert!(stage.join("SKILL.md").is_file());
    }

    #[test]
    fn rejects_skill_names_that_are_not_one_normal_component() {
        let (tmp, layout, cfg, mut reg) = setup();
        let target = Target::Global {
            name: "agents".into(),
        };
        let absolute = tmp.path().join("escaped").to_string_lossy().into_owned();

        for skill in ["", ".", "..", "nested/alpha", r"nested\alpha", &absolute] {
            let err = install_skill(
                &layout,
                &cfg,
                &mut reg,
                "github/o/r",
                skill,
                "skills/alpha",
                &target,
                Method::Copy,
                "c1",
            )
            .unwrap_err();
            assert!(matches!(err, Error::InvalidSkillName(_)), "{skill}: {err}");
        }

        assert!(reg.installs.is_empty());
        assert!(!cfg.targets["agents"].exists());
    }

    #[test]
    fn rejects_source_paths_outside_cache_root() {
        let (tmp, layout, cfg, mut reg) = setup();
        let outside = tmp.path().join("outside");
        std::fs::create_dir_all(&outside).unwrap();
        let absolute = outside.to_string_lossy().into_owned();
        let target = Target::Global {
            name: "agents".into(),
        };

        for source_path in ["../outside", &absolute] {
            let err = install_skill(
                &layout,
                &cfg,
                &mut reg,
                "github/o/r",
                "alpha",
                source_path,
                &target,
                Method::Copy,
                "c1",
            )
            .unwrap_err();
            assert!(
                matches!(err, Error::InvalidSourcePath(_)),
                "{source_path}: {err}"
            );
        }

        assert!(reg.installs.is_empty());
        assert!(!cfg.targets["agents"].exists());
    }

    #[cfg(unix)]
    #[test]
    fn rejects_source_symlink_that_resolves_outside_cache_root() {
        let (tmp, layout, cfg, mut reg) = setup();
        let outside = tmp.path().join("outside");
        std::fs::create_dir_all(&outside).unwrap();
        let cache = layout.cache_dir("github/o/r");
        std::os::unix::fs::symlink(&outside, cache.join("escape")).unwrap();

        let err = install_skill(
            &layout,
            &cfg,
            &mut reg,
            "github/o/r",
            "alpha",
            "escape",
            &Target::Global {
                name: "agents".into(),
            },
            Method::Copy,
            "c1",
        )
        .unwrap_err();

        assert!(matches!(err, Error::InvalidSourcePath(_)));
        assert!(reg.installs.is_empty());
        assert!(!cfg.targets["agents"].exists());
    }

    #[test]
    fn rejects_missing_source_without_creating_destination() {
        let (_tmp, layout, cfg, mut reg) = setup();
        let err = install_skill(
            &layout,
            &cfg,
            &mut reg,
            "github/o/r",
            "missing",
            "skills/missing",
            &Target::Global {
                name: "agents".into(),
            },
            Method::Symlink,
            "c1",
        )
        .unwrap_err();

        assert!(matches!(err, Error::SourceNotDirectory(_)));
        assert!(reg.installs.is_empty());
        assert!(!cfg.targets["agents"].exists());
    }

    #[test]
    fn rejects_file_source_without_creating_destination() {
        let (_tmp, layout, cfg, mut reg) = setup();
        let cache = layout.cache_dir("github/o/r");
        std::fs::write(cache.join("not-a-directory"), "file").unwrap();

        let err = install_skill(
            &layout,
            &cfg,
            &mut reg,
            "github/o/r",
            "not-a-directory",
            "not-a-directory",
            &Target::Global {
                name: "agents".into(),
            },
            Method::Copy,
            "c1",
        )
        .unwrap_err();

        assert!(matches!(err, Error::SourceNotDirectory(_)));
        assert!(reg.installs.is_empty());
        assert!(!cfg.targets["agents"].exists());
    }

    #[cfg(unix)]
    #[test]
    fn rejects_dangling_source_symlink_without_creating_destination() {
        let (_tmp, layout, cfg, mut reg) = setup();
        let cache = layout.cache_dir("github/o/r");
        std::os::unix::fs::symlink("missing", cache.join("dangling")).unwrap();

        let err = install_skill(
            &layout,
            &cfg,
            &mut reg,
            "github/o/r",
            "dangling",
            "dangling",
            &Target::Global {
                name: "agents".into(),
            },
            Method::Symlink,
            "c1",
        )
        .unwrap_err();

        assert!(matches!(err, Error::SourceNotDirectory(_)));
        assert!(reg.installs.is_empty());
        assert!(!cfg.targets["agents"].exists());
    }

    #[test]
    fn copy_install_writes_manifest_marker() {
        let (_t, layout, cfg, mut reg) = setup();
        install_skill(
            &layout,
            &cfg,
            &mut reg,
            "github/o/r",
            "alpha",
            "skills/alpha",
            &Target::Global {
                name: "agents".into(),
            },
            Method::Copy,
            "c1",
        )
        .unwrap();
        let marker = cfg.targets["agents"].join("alpha").join(COPY_MANIFEST);
        assert!(marker.is_file());
        let body: serde_json::Value =
            serde_json::from_str(&std::fs::read_to_string(marker).unwrap()).unwrap();
        assert_eq!(body["version"], 1);
    }

    #[cfg(unix)]
    #[test]
    fn symlink_install_does_not_write_manifest_marker() {
        let (_t, layout, cfg, mut reg) = setup();
        install_skill(
            &layout,
            &cfg,
            &mut reg,
            "github/o/r",
            "alpha",
            "skills/alpha",
            &Target::Global {
                name: "agents".into(),
            },
            Method::Symlink,
            "c1",
        )
        .unwrap();
        let dest = cfg.targets["agents"].join("alpha");
        // 链接本身和缓存源都没有标识文件
        assert!(!dest.join(COPY_MANIFEST).exists());
        assert!(
            !layout
                .cache_dir("github/o/r")
                .join("skills/alpha")
                .join(COPY_MANIFEST)
                .exists()
        );
    }

    #[cfg(unix)]
    #[test]
    fn copy_failure_cleans_staging_and_destination() {
        let (_tmp, layout, cfg, mut reg) = setup();
        let source = layout.cache_dir("github/o/r").join("skills/alpha");
        std::os::unix::fs::symlink("missing", source.join("broken-link")).unwrap();
        let dest_root = &cfg.targets["agents"];

        install_skill(
            &layout,
            &cfg,
            &mut reg,
            "github/o/r",
            "alpha",
            "skills/alpha",
            &Target::Global {
                name: "agents".into(),
            },
            Method::Copy,
            "c1",
        )
        .unwrap_err();

        assert!(reg.installs.is_empty());
        assert!(!dest_root.join("alpha").exists());
        assert_eq!(std::fs::read_dir(dest_root).unwrap().count(), 0);
    }
}

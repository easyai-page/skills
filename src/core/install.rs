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
    let stage = stage_copy(src, dest_root, dest.file_name().unwrap_or_default())?;
    let result = ensure_destination_absent(dest).and_then(|()| commit_staging_dir(&stage, dest));

    if result.is_err() {
        let _ = remove_install_path(&stage);
    }
    result
}

/// 把 src 复制到 dest_root 下的暂存目录并写入所有权标识，返回暂存路径；失败清理暂存。
/// install 与 update 共用同一份 staging+manifest 流程，避免两套复制逻辑漂移。
pub(crate) fn stage_copy(src: &Path, dest_root: &Path, name: &std::ffi::OsStr) -> Result<PathBuf> {
    let stage = create_staging_dir(dest_root, name)?;
    let staged = copy_dir(src, &stage).and_then(|()| write_copy_manifest(&stage));
    if let Err(err) = staged {
        let _ = remove_install_path(&stage);
        return Err(err);
    }
    Ok(stage)
}

/// update 专用：staging 完成后把旧副本 rename 到备份位置，暂存副本 rename 提交，
/// 成功后删除备份；中途失败回滚恢复原副本。与 install 的原子语义一致，不先清后拷。
pub(crate) fn replace_copy_install(src: &Path, dest_root: &Path, dest: &Path) -> Result<()> {
    let stage = stage_copy(src, dest_root, dest.file_name().unwrap_or_default())?;
    let result = commit_replacement(&stage, dest);
    if result.is_err() {
        let _ = remove_install_path(&stage);
    }
    result
}

fn commit_replacement(stage: &Path, dest: &Path) -> Result<()> {
    let dest_root = dest
        .parent()
        .ok_or_else(|| Error::Msg(format!("目标路径没有父目录: {}", dest.display())))?;
    match std::fs::symlink_metadata(dest) {
        Ok(_) => {}
        // 旧副本已被手动删除：无需备份，直接提交暂存副本
        Err(err) if err.kind() == std::io::ErrorKind::NotFound => {
            std::fs::rename(stage, dest)?;
            return Ok(());
        }
        Err(err) => return Err(Error::Io(err)),
    }
    let backup = unique_backup_path(dest_root, dest.file_name().unwrap_or_default())?;
    checked_rename(dest, &backup)?;
    match checked_rename(stage, dest) {
        Ok(()) => {
            // 替换已生效：备份清理失败不否决成功（否则 execute_plan 中止、registry 不落盘、
            // 用户看到误报失败）。降级为警告并给出备份路径，便于手动清理。
            if let Err(err) = remove_backup_dir(&backup) {
                eprintln!(
                    "warning: 副本已更新，但旧副本备份 {} 清理失败（{err}），可手动删除",
                    backup.display()
                );
            }
            Ok(())
        }
        Err(err) => {
            // 回滚：恢复原副本，保证“要么完整保留、要么完整替换”
            match checked_rename(&backup, dest) {
                Ok(()) => Err(Error::Io(err)),
                // 回滚也失败：错误必须带上备份路径与失败原因，让用户能找回原副本
                Err(rollback_err) => Err(Error::Msg(format!(
                    "更新提交失败（{err}），且回滚恢复原副本也失败（{rollback_err}）；\
                     原副本保留在备份 {}，请手动重命名回 {}",
                    backup.display(),
                    dest.display()
                ))),
            }
        }
    }
}

/// commit_replacement 内部的 rename 封装。
/// 测试构建下支持按调用序号注入失败（线程本地位掩码，不影响并行测试与其他线程）。
fn checked_rename(from: &Path, to: &Path) -> std::io::Result<()> {
    #[cfg(test)]
    {
        let seq = RENAME_SEQ.with(|seq| {
            let next = seq.get() + 1;
            seq.set(next);
            next
        });
        let mask = RENAME_FAIL_MASK.with(|mask| mask.get());
        if seq <= 64 && mask & (1u64 << (seq - 1)) != 0 {
            return Err(std::io::Error::other("测试注入的 rename 失败"));
        }
    }
    std::fs::rename(from, to)
}

/// 备份目录清理。单独成函数是为了测试构建下可注入失败（线程本地开关）。
fn remove_backup_dir(backup: &Path) -> std::io::Result<()> {
    #[cfg(test)]
    if BACKUP_CLEANUP_FAILS.with(|fails| fails.get()) {
        return Err(std::io::Error::new(
            std::io::ErrorKind::PermissionDenied,
            "测试注入的备份清理失败",
        ));
    }
    std::fs::remove_dir_all(backup)
}

#[cfg(test)]
thread_local! {
    /// 当前线程内 checked_rename 的调用计数（1 起计），配合 RENAME_FAIL_MASK 使用
    static RENAME_SEQ: std::cell::Cell<u64> = const { std::cell::Cell::new(0) };
    /// 位掩码：第 n 次调用对应位 (n-1) 置位时注入失败；0 = 不注入
    static RENAME_FAIL_MASK: std::cell::Cell<u64> = const { std::cell::Cell::new(0) };
    /// 置位时 remove_backup_dir 注入失败
    static BACKUP_CLEANUP_FAILS: std::cell::Cell<bool> = const { std::cell::Cell::new(false) };
}

fn unique_backup_path(dest_root: &Path, skill: &std::ffi::OsStr) -> Result<PathBuf> {
    let pid = std::process::id();
    loop {
        let sequence = STAGE_COUNTER.fetch_add(1, Ordering::Relaxed);
        let name = format!(
            ".{}-update-backup-{}-{}.tmp",
            skill.to_string_lossy(),
            pid,
            sequence
        );
        let path = dest_root.join(name);
        match std::fs::symlink_metadata(&path) {
            // 与 create_staging_dir 的 AlreadyExists 重试风格对齐：
            // 仅在确认不存在时采用；已占用换下一个序号；其他错误如实上报而非当作不存在
            Ok(_) => continue,
            Err(err) if err.kind() == std::io::ErrorKind::NotFound => return Ok(path),
            Err(err) => return Err(Error::Io(err)),
        }
    }
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

    /// 构造 commit_replacement 的输入：暂存副本（v2）与已存在的旧副本 dest（v1）。
    fn setup_replacement(root: &Path) -> (PathBuf, PathBuf) {
        let stage = root.join("stage");
        let dest = root.join("alpha");
        std::fs::create_dir(&stage).unwrap();
        std::fs::write(stage.join("SKILL.md"), "v2").unwrap();
        std::fs::create_dir(&dest).unwrap();
        std::fs::write(dest.join("SKILL.md"), "v1").unwrap();
        (stage, dest)
    }

    fn dir_names(root: &Path) -> Vec<String> {
        std::fs::read_dir(root)
            .unwrap()
            .map(|entry| entry.unwrap().file_name().to_string_lossy().into_owned())
            .collect()
    }

    #[test]
    fn replacement_succeeds_even_when_backup_cleanup_fails() {
        // 回归（任务 9 第 2 轮）：替换已生效后，备份删除失败不得传播 Err——
        // 否则 execute_plan 中止、reg.save 不执行，磁盘与 registry 脱节且误报失败。
        let root = tempfile::tempdir().unwrap();
        let (stage, dest) = setup_replacement(root.path());

        BACKUP_CLEANUP_FAILS.with(|fails| fails.set(true));
        let result = commit_replacement(&stage, &dest);
        BACKUP_CLEANUP_FAILS.with(|fails| fails.set(false));

        assert!(
            result.is_ok(),
            "备份清理失败不应否决已成功的替换: {result:?}"
        );
        assert_eq!(
            std::fs::read_to_string(dest.join("SKILL.md")).unwrap(),
            "v2"
        );
        // 备份残留，内容仍是旧副本，留给用户按警告提示手动清理
        let backup = dir_names(root.path())
            .into_iter()
            .find(|name| name.contains("-update-backup-"))
            .expect("清理失败时应残留备份目录");
        assert_eq!(
            std::fs::read_to_string(root.path().join(&backup).join("SKILL.md")).unwrap(),
            "v1"
        );
        std::fs::remove_dir_all(root.path().join(backup)).unwrap();
    }

    #[test]
    fn commit_failure_rolls_back_to_original_copy() {
        // 提交 rename（本次第 2 次调用）失败时，回滚恢复原副本，无备份残留。
        let root = tempfile::tempdir().unwrap();
        let (stage, dest) = setup_replacement(root.path());

        RENAME_SEQ.with(|seq| seq.set(0));
        RENAME_FAIL_MASK.with(|mask| mask.set(0b10));
        let err = commit_replacement(&stage, &dest).unwrap_err();
        RENAME_FAIL_MASK.with(|mask| mask.set(0));

        assert!(matches!(err, Error::Io(_)), "{err}");
        assert_eq!(
            std::fs::read_to_string(dest.join("SKILL.md")).unwrap(),
            "v1"
        );
        assert!(
            !dir_names(root.path())
                .iter()
                .any(|name| name.contains("-update-backup-")),
            "回滚成功后不应有备份残留"
        );
    }

    #[test]
    fn rollback_failure_reports_backup_path_for_manual_recovery() {
        // 回归（任务 9 第 2 轮）：回滚失败不得静默吞错，错误信息必须带备份路径与原因。
        let root = tempfile::tempdir().unwrap();
        let (stage, dest) = setup_replacement(root.path());

        // 第 2 次调用（提交）与第 3 次调用（回滚）均注入失败
        RENAME_SEQ.with(|seq| seq.set(0));
        RENAME_FAIL_MASK.with(|mask| mask.set(0b110));
        let err = commit_replacement(&stage, &dest).unwrap_err();
        RENAME_FAIL_MASK.with(|mask| mask.set(0));

        let msg = format!("{err}");
        assert!(matches!(err, Error::Msg(_)), "{err}");
        assert!(msg.contains("回滚"), "{msg}");
        assert!(msg.contains("测试注入"), "{msg}");
        // 备份仍在原地（回滚失败），里面是旧副本；错误信息含备份路径可定位
        let backup = dir_names(root.path())
            .into_iter()
            .find(|name| name.contains("-update-backup-"))
            .expect("回滚失败时备份应仍在原地");
        assert!(msg.contains(&backup), "{msg}");
        assert_eq!(
            std::fs::read_to_string(root.path().join(&backup).join("SKILL.md")).unwrap(),
            "v1"
        );
        assert!(!dest.exists());
        // 模拟用户按错误提示手动恢复：原副本可完整找回
        std::fs::rename(root.path().join(backup), &dest).unwrap();
        assert_eq!(
            std::fs::read_to_string(dest.join("SKILL.md")).unwrap(),
            "v1"
        );
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

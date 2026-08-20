use std::num::NonZeroU32;
use std::path::Path;
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};

use super::error::{Error, Result};

fn gerr<E: std::fmt::Display>(e: E) -> Error {
    Error::Git(e.to_string())
}

/// 保持缓存为 depth=1 浅克隆，fetch 时浅边界随远端前移。
fn depth_one() -> gix::remote::fetch::Shallow {
    gix::remote::fetch::Shallow::DepthAtRemote(NonZeroU32::new(1).expect("非零"))
}

/// 浅克隆 url 到 dest（含工作区 checkout），返回 HEAD commit 全 hash。
pub fn shallow_clone(url: &str, dest: &Path) -> Result<String> {
    let url = gix::url::parse(url.into()).map_err(gerr)?;
    let mut prep = gix::prepare_clone(url, dest)
        .map_err(gerr)?
        .with_shallow(depth_one());
    let (mut checkout, _) = prep
        .fetch_then_checkout(gix::progress::Discard, &AtomicBool::new(false))
        .map_err(gerr)?;
    let (repo, _) = checkout
        .main_worktree(gix::progress::Discard, &AtomicBool::new(false))
        .map_err(gerr)?;
    Ok(repo.head_id().map_err(gerr)?.to_string())
}

/// fetch 远端并 hard reset（分支 ref + 索引 + 工作区）到 origin/<当前分支>。
/// HEAD 有新 commit 返回 Some(hash)，否则 None。
pub fn fetch_and_reset(path: &Path) -> Result<Option<String>> {
    let repo = gix::open(path).map_err(gerr)?;
    let remote = repo
        .find_default_remote(gix::remote::Direction::Fetch)
        .ok_or_else(|| Error::Git("无默认 remote".into()))?
        .map_err(gerr)?;
    let remote_name = match remote
        .name()
        .ok_or_else(|| Error::Git("默认 remote 无名称".into()))?
    {
        gix::remote::Name::Symbol(name) => name.to_owned(),
        gix::remote::Name::Url(_) => return Err(Error::Git("默认 remote 名称为 URL".into())),
    };
    remote
        .connect(gix::remote::Direction::Fetch)
        .map_err(gerr)?
        .prepare_fetch(gix::progress::Discard, Default::default())
        .map_err(gerr)?
        .with_shallow(depth_one())
        .receive(gix::progress::Discard, &AtomicBool::new(false))
        .map_err(gerr)?;

    // fetch 后重新打开，解析本地分支对应的上游 ref
    let repo = gix::open(path).map_err(gerr)?;
    let branch = repo
        .head()
        .map_err(gerr)?
        .referent_name()
        .map(|n| n.as_bstr().to_string())
        .ok_or_else(|| Error::Git("HEAD 为 detached 状态，不支持 reset".into()))?;
    let short = branch.trim_start_matches("refs/heads/");
    let oid = repo
        .find_reference(&format!("refs/remotes/{remote_name}/{short}"))
        .map_err(gerr)?
        .id()
        .detach();
    let old_oid = repo.head_id().map_err(gerr)?.detach();
    if oid == old_oid {
        return Ok(None);
    }

    // 工作区、index 和分支 ref 在同一个事务中切换，任一阶段失败都会恢复旧状态。
    checkout_tree_with_ref(&repo, branch.as_str(), old_oid, oid)?;
    Ok(Some(oid.to_string()))
}

/// 返回工作区 HEAD commit 全 hash。
pub fn head_commit(path: &Path) -> Result<String> {
    let repo = gix::open(path).map_err(gerr)?;
    Ok(repo.head_id().map_err(gerr)?.to_string())
}

static NEXT_TRANSACTION_ID: AtomicU64 = AtomicU64::new(0);

#[derive(Clone, Copy)]
#[allow(dead_code)]
enum FailurePoint {
    AfterWorktreeInstall,
    AfterIndexInstall,
    AfterRefUpdate,
}

struct PreparedCheckout {
    staging: std::path::PathBuf,
    staged_workdir: std::path::PathBuf,
    staged_index: std::path::PathBuf,
    workdir: std::path::PathBuf,
    index: std::path::PathBuf,
}

#[derive(Debug, PartialEq, Eq)]
enum SnapshotEntry {
    Directory(std::path::PathBuf),
    File(std::path::PathBuf, Vec<u8>),
    Symlink(std::path::PathBuf, std::path::PathBuf),
}

struct CheckoutTransaction {
    prepared: PreparedCheckout,
    backup_workdir: std::path::PathBuf,
    old_index: std::path::PathBuf,
    old_index_bytes: Option<Vec<u8>>,
    old_worktree: Vec<SnapshotEntry>,
    backed_up_entries: Vec<std::ffi::OsString>,
    installed_entries: Vec<std::ffi::OsString>,
}

/// 先在临时目录中准备目标 tree、工作区和 index，不触碰真实仓库。
fn prepare_checkout(repo: &gix::Repository, oid: gix::ObjectId) -> Result<PreparedCheckout> {
    let workdir = repo
        .work_dir()
        .ok_or_else(|| Error::Git("bare 仓库无工作区".into()))?
        .to_path_buf();
    let parent = workdir.parent().unwrap_or(&workdir);
    let staging = loop {
        let serial = NEXT_TRANSACTION_ID.fetch_add(1, Ordering::Relaxed);
        let path = parent.join(format!(
            ".skills-checkout-{oid}-{}-{serial}",
            std::process::id()
        ));
        match std::fs::create_dir(&path) {
            Ok(()) => break path,
            Err(err) if err.kind() == std::io::ErrorKind::AlreadyExists => continue,
            Err(err) => return Err(err.into()),
        }
    };
    let result = (|| {
        let staged_workdir = staging.join("worktree");
        let backup_workdir = staging.join("backup");
        std::fs::create_dir(&staged_workdir)?;
        std::fs::create_dir(&backup_workdir)?;
        let tree_id = repo
            .find_commit(oid)
            .map_err(gerr)?
            .tree_id()
            .map_err(gerr)?
            .detach();
        let state =
            gix::index::State::from_tree(&tree_id, repo.objects.clone(), Default::default())
                .map_err(gerr)?;
        let staged_index = staging.join("index");
        let mut index = gix::index::File::from_state(state, staged_index.clone());
        let opts = gix_worktree_state::checkout::Options {
            fs: gix::fs::Capabilities::probe(repo.git_dir()),
            validate: Default::default(),
            thread_limit: None,
            destination_is_initially_empty: true,
            overwrite_existing: false,
            keep_going: false,
            stat_options: Default::default(),
            attributes: gix_worktree::stack::state::Attributes::new(
                Default::default(),
                None,
                gix_worktree::stack::state::attributes::Source::IdMapping,
                Default::default(),
            ),
            filters: gix_filter::Pipeline::default(),
            filter_process_delay: gix_filter::driver::apply::Delay::Forbid,
        };
        gix_worktree_state::checkout(
            &mut index,
            &staged_workdir,
            repo.objects.clone().into_arc()?,
            &gix::progress::Discard,
            &gix::progress::Discard,
            &AtomicBool::new(false),
            opts,
        )
        .map_err(gerr)?;
        index.write(Default::default()).map_err(gerr)?;
        Ok(PreparedCheckout {
            staging: staging.clone(),
            staged_workdir,
            staged_index,
            workdir,
            index: repo.index_path().to_path_buf(),
        })
    })();
    if result.is_err() {
        let _ = std::fs::remove_dir_all(&staging);
    }
    result
}

impl CheckoutTransaction {
    fn prepare(repo: &gix::Repository, oid: gix::ObjectId) -> Result<Self> {
        let prepared = prepare_checkout(repo, oid)?;
        let staging = prepared.staging.clone();
        let result = (|| {
            let backup_workdir = prepared.staging.join("backup");
            let old_index = prepared.staging.join("old-index");
            let old_index_bytes = match std::fs::read(&prepared.index) {
                Ok(bytes) => Some(bytes),
                Err(err) if err.kind() == std::io::ErrorKind::NotFound => None,
                Err(err) => return Err(Error::from(err)),
            };
            Ok(Self {
                old_worktree: snapshot_worktree(&prepared.workdir)?,
                prepared,
                backup_workdir,
                old_index,
                old_index_bytes,
                backed_up_entries: Vec::new(),
                installed_entries: Vec::new(),
            })
        })();
        if result.is_err() {
            let _ = std::fs::remove_dir_all(&staging);
        }
        result
    }

    fn install(
        &mut self,
        repo: &gix::Repository,
        branch: Option<(&str, gix::ObjectId, gix::ObjectId)>,
        failure: Option<FailurePoint>,
    ) -> Result<()> {
        for entry in std::fs::read_dir(&self.prepared.workdir)? {
            let entry = entry?;
            if entry.file_name() != ".git" {
                let name = entry.file_name();
                std::fs::rename(entry.path(), self.backup_workdir.join(&name))?;
                self.backed_up_entries.push(name);
            }
        }
        for entry in std::fs::read_dir(&self.prepared.staged_workdir)? {
            let entry = entry?;
            let name = entry.file_name();
            std::fs::rename(entry.path(), self.prepared.workdir.join(&name))?;
            self.installed_entries.push(name);
        }
        if matches!(failure, Some(FailurePoint::AfterWorktreeInstall)) {
            return Err(Error::Git("注入：工作区切换后失败".into()));
        }

        if self.prepared.index.exists() {
            std::fs::rename(&self.prepared.index, &self.old_index)?;
        }
        std::fs::rename(&self.prepared.staged_index, &self.prepared.index)?;
        if matches!(failure, Some(FailurePoint::AfterIndexInstall)) {
            return Err(Error::Git("注入：index 切换后失败".into()));
        }

        if let Some((branch, old_oid, new_oid)) = branch {
            repo.reference(
                branch,
                new_oid,
                gix::refs::transaction::PreviousValue::Any,
                "skills update",
            )
            .map_err(gerr)?;
            if matches!(failure, Some(FailurePoint::AfterRefUpdate)) {
                // 让测试覆盖“ref 已切换但后续阶段失败”的完整回滚。
                let _ = old_oid;
                return Err(Error::Git("注入：ref 切换后失败".into()));
            }
        }
        Ok(())
    }

    fn rollback(
        &self,
        repo: &gix::Repository,
        branch: Option<(&str, gix::ObjectId, gix::ObjectId)>,
    ) -> Result<()> {
        let mut failures = Vec::new();

        // install 的逆序：ref、index、工作区。每一步都验证旧值，避免返回假成功。
        if let Some((branch, old_oid, new_oid)) = branch {
            if let Err(err) = restore_ref(repo, branch, old_oid, new_oid) {
                failures.push(format!("恢复 ref 失败: {err}"));
            }
        }
        if let Err(err) = self.restore_index() {
            failures.push(format!("恢复 index 失败: {err}"));
        }
        if let Err(err) = self.restore_worktree() {
            failures.push(format!("恢复工作区失败: {err}"));
        }

        if failures.is_empty() {
            Ok(())
        } else {
            Err(Error::GitRecovery(failures.join("; ")))
        }
    }

    fn restore_index(&self) -> std::io::Result<()> {
        match &self.old_index_bytes {
            Some(old_bytes) => {
                if self.prepared.index.exists() {
                    if std::fs::read(&self.prepared.index)? == *old_bytes {
                        return Ok(());
                    }
                    std::fs::remove_file(&self.prepared.index)?;
                }
                std::fs::rename(&self.old_index, &self.prepared.index)?;
                if std::fs::read(&self.prepared.index)? != *old_bytes {
                    return Err(std::io::Error::new(
                        std::io::ErrorKind::InvalidData,
                        "恢复后的 index 字节不匹配",
                    ));
                }
            }
            None => {
                if self.prepared.index.exists() {
                    std::fs::remove_file(&self.prepared.index)?;
                }
            }
        }
        Ok(())
    }

    fn restore_worktree(&self) -> std::io::Result<()> {
        remove_worktree_entries(&self.prepared.workdir, &self.installed_entries)?;
        restore_worktree_entries_reverse(
            &self.backup_workdir,
            &self.prepared.workdir,
            &self.backed_up_entries,
        )?;
        if snapshot_worktree(&self.prepared.workdir)? != self.old_worktree {
            return Err(std::io::Error::new(
                std::io::ErrorKind::InvalidData,
                "恢复后的工作区与旧快照不匹配",
            ));
        }
        Ok(())
    }

    fn finish(self) {
        let _ = std::fs::remove_dir_all(&self.prepared.staging);
    }
}

fn remove_worktree_entries(
    workdir: &std::path::Path,
    names: &[std::ffi::OsString],
) -> std::io::Result<()> {
    for name in names {
        let path = workdir.join(name);
        let metadata = match std::fs::symlink_metadata(&path) {
            Ok(metadata) => metadata,
            Err(err) if err.kind() == std::io::ErrorKind::NotFound => continue,
            Err(err) => return Err(err),
        };
        if metadata.is_dir() {
            std::fs::remove_dir_all(path)?;
        } else {
            std::fs::remove_file(path)?;
        }
    }
    Ok(())
}

fn restore_worktree_entries_reverse(
    backup: &std::path::Path,
    workdir: &std::path::Path,
    names: &[std::ffi::OsString],
) -> std::io::Result<()> {
    for name in names.iter().rev() {
        std::fs::rename(backup.join(name), workdir.join(name))?;
    }
    Ok(())
}

fn restore_ref(
    repo: &gix::Repository,
    branch: &str,
    old_oid: gix::ObjectId,
    new_oid: gix::ObjectId,
) -> Result<()> {
    let current = repo.find_reference(branch).map_err(gerr)?.id().detach();
    if current == old_oid {
        return Ok(());
    }
    if current != new_oid {
        return Err(Error::Git(format!(
            "ref 当前值 {current} 既非旧值 {old_oid} 也非新值 {new_oid}"
        )));
    }
    repo.reference(
        branch,
        old_oid,
        gix::refs::transaction::PreviousValue::MustExistAndMatch(new_oid.into()),
        "skills rollback",
    )
    .map_err(gerr)?;
    Ok(())
}

fn snapshot_worktree(root: &std::path::Path) -> std::io::Result<Vec<SnapshotEntry>> {
    fn visit(
        root: &std::path::Path,
        dir: &std::path::Path,
        out: &mut Vec<SnapshotEntry>,
    ) -> std::io::Result<()> {
        let mut entries = std::fs::read_dir(dir)?.collect::<std::io::Result<Vec<_>>>()?;
        entries.sort_by_key(|entry| entry.file_name());
        for entry in entries {
            if dir == root && entry.file_name() == ".git" {
                continue;
            }
            let path = entry.path();
            let relative = path
                .strip_prefix(root)
                .expect("快照路径在工作区内")
                .to_path_buf();
            let metadata = std::fs::symlink_metadata(&path)?;
            if metadata.file_type().is_symlink() {
                out.push(SnapshotEntry::Symlink(relative, std::fs::read_link(path)?));
            } else if metadata.is_dir() {
                out.push(SnapshotEntry::Directory(relative));
                visit(root, &path, out)?;
            } else {
                out.push(SnapshotEntry::File(relative, std::fs::read(path)?));
            }
        }
        Ok(())
    }

    let mut out = Vec::new();
    visit(root, root, &mut out)?;
    Ok(out)
}

fn checkout_tree(repo: &gix::Repository, oid: gix::ObjectId) -> Result<()> {
    checkout_transaction(repo, oid, None, None)
}

fn checkout_tree_with_ref(
    repo: &gix::Repository,
    branch: &str,
    old_oid: gix::ObjectId,
    new_oid: gix::ObjectId,
) -> Result<()> {
    checkout_transaction(repo, new_oid, Some((branch, old_oid, new_oid)), None)
}

#[cfg(test)]
fn checkout_tree_with_failure(
    repo: &gix::Repository,
    branch: &str,
    old_oid: gix::ObjectId,
    new_oid: gix::ObjectId,
    failure: FailurePoint,
) -> Result<()> {
    checkout_transaction(
        repo,
        new_oid,
        Some((branch, old_oid, new_oid)),
        Some(failure),
    )
}

fn checkout_transaction(
    repo: &gix::Repository,
    oid: gix::ObjectId,
    branch: Option<(&str, gix::ObjectId, gix::ObjectId)>,
    failure: Option<FailurePoint>,
) -> Result<()> {
    let mut transaction = CheckoutTransaction::prepare(repo, oid)?;
    match transaction.install(repo, branch, failure) {
        Ok(()) => {
            transaction.finish();
            Ok(())
        }
        Err(err) => match transaction.rollback(repo, branch) {
            Ok(()) => {
                transaction.finish();
                Err(err)
            }
            Err(recovery_err) => Err(Error::GitRecovery(format!(
                "切换失败且缓存不可用：{err}; {recovery_err}"
            ))),
        },
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::path::{Path, PathBuf};
    use std::process::Command;

    fn git(dir: &Path, args: &[&str]) {
        let st = Command::new("git")
            .args(args)
            .current_dir(dir)
            .status()
            .unwrap();
        assert!(st.success(), "git {:?} 失败", args);
    }

    fn git_out(dir: &Path, args: &[&str]) -> String {
        let out = Command::new("git")
            .args(args)
            .current_dir(dir)
            .output()
            .unwrap();
        assert!(out.status.success(), "git {:?} 失败", args);
        String::from_utf8(out.stdout).unwrap().trim().to_string()
    }

    /// 造一个含真实文件的 bare 仓库，返回 (临时目录, work clone, bare repo)。
    /// c1: skills/alpha/SKILL.md = "v1\n"，另有 stale.txt。
    fn make_bare_repo() -> (tempfile::TempDir, PathBuf, PathBuf) {
        let tmp = tempfile::tempdir().unwrap();
        let work = tmp.path().join("work");
        let bare = tmp.path().join("bare.git");
        std::fs::create_dir_all(work.join("skills/alpha")).unwrap();
        std::fs::write(work.join("skills/alpha/SKILL.md"), "v1\n").unwrap();
        std::fs::write(work.join("stale.txt"), "stale\n").unwrap();
        git(&work, &["init", "-b", "main"]);
        git(&work, &["add", "."]);
        git(
            &work,
            &[
                "-c",
                "user.email=t@t",
                "-c",
                "user.name=t",
                "commit",
                "-m",
                "c1",
            ],
        );
        git(&work, &["clone", "--bare", ".", bare.to_str().unwrap()]);
        (tmp, work, bare)
    }

    #[test]
    fn shallow_clone_returns_head_commit() {
        let (_tmp, _work, bare) = make_bare_repo();
        let dst = tempfile::tempdir().unwrap();
        let dest = dst.path().join("clone");
        let commit = shallow_clone(&format!("file://{}", bare.display()), &dest).unwrap();
        assert_eq!(commit.len(), 40, "应为完整 sha1");
        // 与 git CLI 看到的 HEAD 一致
        assert_eq!(git_out(&bare, &["rev-parse", "main"]), commit);
        assert_eq!(head_commit(&dest).unwrap(), commit);
        assert!(dest.join(".git").exists());
        assert!(dest.join(".git/shallow").exists(), "应为 depth=1 浅克隆");
        // 工作区真正 checkout 出文件
        assert_eq!(
            std::fs::read_to_string(dest.join("skills/alpha/SKILL.md")).unwrap(),
            "v1\n"
        );
    }

    #[test]
    fn fetch_and_reset_reports_change_only_when_moved() {
        let (tmp, work, bare) = make_bare_repo();
        let dest = tmp.path().join("clone");
        let c1 = shallow_clone(&format!("file://{}", bare.display()), &dest).unwrap();
        // 无新提交 → None，工作区不动
        assert_eq!(fetch_and_reset(&dest).unwrap(), None);
        assert_eq!(
            std::fs::read_to_string(dest.join("skills/alpha/SKILL.md")).unwrap(),
            "v1\n"
        );
        // 推一个真实改动：改内容、删文件、加文件
        std::fs::write(work.join("skills/alpha/SKILL.md"), "v2\n").unwrap();
        std::fs::remove_file(work.join("stale.txt")).unwrap();
        std::fs::write(work.join("new.txt"), "new\n").unwrap();
        git(&work, &["add", "-A"]);
        git(
            &work,
            &[
                "-c",
                "user.email=t@t",
                "-c",
                "user.name=t",
                "commit",
                "-m",
                "c2",
            ],
        );
        git(&work, &["push", bare.to_str().unwrap(), "main"]);
        let c2_remote = git_out(&bare, &["rev-parse", "main"]);

        let c2 = fetch_and_reset(&dest).unwrap().expect("应有新 commit");
        assert_eq!(c2, c2_remote);
        assert_ne!(c1, c2);
        assert_eq!(head_commit(&dest).unwrap(), c2);
        // gix 0.66 会保留旧浅边界并追加新边界（其公开实现明确不做 Git CLI 的 pruning）。
        let shallow = std::fs::read_to_string(dest.join(".git/shallow")).unwrap();
        assert!(
            shallow.lines().any(|line| line == c2),
            "新 HEAD 应位于浅边界中: {shallow}"
        );
        // hard reset 语义：工作区/索引跟随远端
        assert_eq!(
            std::fs::read_to_string(dest.join("skills/alpha/SKILL.md")).unwrap(),
            "v2\n"
        );
        assert!(!dest.join("stale.txt").exists(), "远端删除的文件应被清除");
        assert_eq!(
            std::fs::read_to_string(dest.join("new.txt")).unwrap(),
            "new\n"
        );
        assert!(
            git_out(&dest, &["status", "--porcelain"]).is_empty(),
            "reset 后索引和工作区应干净"
        );
        // 再跑一次应回到 None（幂等）
        assert_eq!(fetch_and_reset(&dest).unwrap(), None);
    }

    #[test]
    fn fetch_and_reset_uses_non_origin_default_remote() {
        let (tmp, work, bare) = make_bare_repo();
        let dest = tmp.path().join("clone");
        let c1 = shallow_clone(&format!("file://{}", bare.display()), &dest).unwrap();
        git(&dest, &["remote", "rename", "origin", "upstream"]);

        std::fs::write(work.join("new.txt"), "new\n").unwrap();
        git(&work, &["add", "new.txt"]);
        git(
            &work,
            &[
                "-c",
                "user.email=t@t",
                "-c",
                "user.name=t",
                "commit",
                "-m",
                "c2",
            ],
        );
        git(&work, &["push", bare.to_str().unwrap(), "main"]);

        let c2 = fetch_and_reset(&dest).unwrap().expect("应有新 commit");
        assert_ne!(c1, c2);
        assert_eq!(head_commit(&dest).unwrap(), c2);
        assert_eq!(
            std::fs::read_to_string(dest.join("new.txt")).unwrap(),
            "new\n"
        );
    }

    #[test]
    fn checkout_tree_failure_preserves_existing_worktree() {
        let (_tmp, _work, bare) = make_bare_repo();
        let dst = tempfile::tempdir().unwrap();
        let dest = dst.path().join("clone");
        shallow_clone(&format!("file://{}", bare.display()), &dest).unwrap();
        let existing = dest.join("stale.txt");
        let repo = gix::open(&dest).unwrap();

        assert!(checkout_tree(&repo, gix::ObjectId::null(gix::hash::Kind::Sha1)).is_err());
        assert_eq!(std::fs::read_to_string(existing).unwrap(), "stale\n");
    }

    #[test]
    fn checkout_transaction_rolls_back_after_switch_failures() {
        let (tmp, work, bare) = make_bare_repo();
        let dest = tmp.path().join("clone");
        let c1 = shallow_clone(&format!("file://{}", bare.display()), &dest).unwrap();
        let old_index = std::fs::read(dest.join(".git/index")).unwrap();

        std::fs::write(work.join("skills/alpha/SKILL.md"), "v2\n").unwrap();
        std::fs::remove_file(work.join("stale.txt")).unwrap();
        std::fs::write(work.join("new.txt"), "new\n").unwrap();
        git(&work, &["add", "-A"]);
        git(
            &work,
            &[
                "-c",
                "user.email=t@t",
                "-c",
                "user.name=t",
                "commit",
                "-m",
                "c2",
            ],
        );
        git(&work, &["push", bare.to_str().unwrap(), "main"]);
        git(&dest, &["fetch", "origin", "main"]);
        let c2 = git_out(&bare, &["rev-parse", "main"]);
        let old_oid = gix::ObjectId::from_hex(c1.as_bytes()).unwrap();
        let new_oid = gix::ObjectId::from_hex(c2.as_bytes()).unwrap();
        let branch = "refs/heads/main";

        let assert_old_state = || {
            assert_eq!(
                std::fs::read_to_string(dest.join("skills/alpha/SKILL.md")).unwrap(),
                "v1\n"
            );
            assert_eq!(
                std::fs::read_to_string(dest.join("stale.txt")).unwrap(),
                "stale\n"
            );
            assert!(!dest.join("new.txt").exists());
            assert_eq!(std::fs::read(dest.join(".git/index")).unwrap(), old_index);
            assert_eq!(git_out(&dest, &["rev-parse", "refs/heads/main"]), c1);
            assert_eq!(head_commit(&dest).unwrap(), c1);
        };

        let repo = gix::open(&dest).unwrap();
        assert!(
            checkout_tree_with_failure(
                &repo,
                branch,
                old_oid,
                new_oid,
                FailurePoint::AfterIndexInstall,
            )
            .is_err()
        );
        assert_old_state();

        let repo = gix::open(&dest).unwrap();
        assert!(
            checkout_tree_with_failure(
                &repo,
                branch,
                old_oid,
                new_oid,
                FailurePoint::AfterWorktreeInstall,
            )
            .is_err()
        );
        assert_old_state();

        let repo = gix::open(&dest).unwrap();
        assert!(
            checkout_tree_with_failure(
                &repo,
                branch,
                old_oid,
                new_oid,
                FailurePoint::AfterRefUpdate,
            )
            .is_err()
        );
        assert_old_state();
    }

    #[test]
    fn head_commit_errors_on_missing_repo() {
        let tmp = tempfile::tempdir().unwrap();
        assert!(head_commit(tmp.path()).is_err());
    }
}

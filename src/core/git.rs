use std::num::NonZeroU32;
use std::path::Path;
use std::sync::atomic::AtomicBool;

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
    let before = head_commit(path)?;
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
    if oid.to_string() == before {
        return Ok(None);
    }

    // 先检出成功再移动分支指针，失败时仓库保持原状
    checkout_tree(&repo, oid)?;
    repo.reference(
        branch.as_str(),
        oid,
        gix::refs::transaction::PreviousValue::Any,
        "skills update",
    )
    .map_err(gerr)?;
    Ok(Some(oid.to_string()))
}

/// 返回工作区 HEAD commit 全 hash。
pub fn head_commit(path: &Path) -> Result<String> {
    let repo = gix::open(path).map_err(gerr)?;
    Ok(repo.head_id().map_err(gerr)?.to_string())
}

/// 把 `oid` 的树检出到工作区并重建索引。先在临时目录完成所有
/// 可能失败的 tree/index/checkout 操作，成功后再替换工作区内容。
fn checkout_tree(repo: &gix::Repository, oid: gix::ObjectId) -> Result<()> {
    let workdir = repo
        .work_dir()
        .ok_or_else(|| Error::Git("bare 仓库无工作区".into()))?
        .to_path_buf();
    let staging = workdir
        .parent()
        .unwrap_or(&workdir)
        .join(format!(".skills-checkout-{}", oid));
    std::fs::create_dir(&staging)?;
    let staged_workdir = staging.join("worktree");
    std::fs::create_dir(&staged_workdir)?;

    let result = (|| {
        let tree_id = repo
            .find_commit(oid)
            .map_err(gerr)?
            .tree_id()
            .map_err(gerr)?
            .detach();
        let state =
            gix::index::State::from_tree(&tree_id, repo.objects.clone(), Default::default())
                .map_err(gerr)?;
        let staged_index_path = staging.join("index");
        let mut index = gix::index::File::from_state(state, staged_index_path.clone());
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

        for entry in std::fs::read_dir(&workdir)? {
            let entry = entry?;
            if entry.file_name() == ".git" {
                continue;
            }
            if entry.file_type()?.is_dir() {
                std::fs::remove_dir_all(entry.path())?;
            } else {
                std::fs::remove_file(entry.path())?;
            }
        }
        for entry in std::fs::read_dir(&staged_workdir)? {
            let entry = entry?;
            std::fs::rename(entry.path(), workdir.join(entry.file_name()))?;
        }
        std::fs::rename(staged_index_path, repo.index_path()).map_err(gerr)?;
        Ok(())
    })();

    let cleanup = std::fs::remove_dir_all(&staging);
    match (result, cleanup) {
        (Ok(()), Ok(())) => Ok(()),
        (Err(err), _) => Err(err),
        (Ok(()), Err(err)) => Err(gerr(err)),
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
    fn head_commit_errors_on_missing_repo() {
        let tmp = tempfile::tempdir().unwrap();
        assert!(head_commit(tmp.path()).is_err());
    }
}

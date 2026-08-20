use super::error::{Error, Result};
use super::git;
use super::paths::Layout;
use super::source::SourceSpec;
use std::path::{Path, PathBuf};

pub struct Cached {
    pub path: PathBuf,
    pub commit: String,
    pub fresh: bool,
}

pub struct SkillEntry {
    pub name: String,
    pub description: String,
    pub rel_path: PathBuf,
}

/// 确保 source 已缓存；已存在则复用（不重复下载）。
pub fn ensure_cached(layout: &Layout, spec: &SourceSpec) -> Result<Cached> {
    let dest = layout.cache_dir(&spec.key);
    let reusable = match (&spec.url, &spec.local_path) {
        (Some(_), None) => remote_cache_commit(&dest),
        (None, Some(_)) => local_cache_is_nonempty_dir(&dest).then(|| Ok(String::new())),
        _ => None,
    };
    if let Some(commit) = reusable.transpose()? {
        return Ok(Cached {
            path: dest,
            commit,
            fresh: false,
        });
    }

    remove_cache_path(&dest)?;
    std::fs::create_dir_all(
        dest.parent()
            .ok_or_else(|| Error::Msg(format!("缓存路径没有父目录: {}", dest.display())))?,
    )?;
    match (&spec.url, &spec.local_path) {
        (Some(url), None) => match git::shallow_clone(url, &dest) {
            Ok(commit) => Ok(Cached {
                path: dest,
                commit,
                fresh: true,
            }),
            Err(err) => {
                let _ = remove_cache_path(&dest);
                Err(err)
            }
        },
        (None, Some(src)) => match copy_dir(src, &dest) {
            Ok(()) => Ok(Cached {
                path: dest,
                commit: String::new(),
                fresh: true,
            }),
            Err(err) => {
                let _ = remove_cache_path(&dest);
                Err(err)
            }
        },
        _ => Err(Error::Msg("source 缺少 url 或本地路径".into())),
    }
}

fn remote_cache_commit(dest: &Path) -> Option<Result<String>> {
    if !dest.is_dir() || !dest.join(".git").exists() {
        return None;
    }
    match git::head_commit(dest) {
        Ok(commit) if !commit.trim().is_empty() => Some(Ok(commit)),
        Ok(_) => None,
        Err(_) => None,
    }
}

fn local_cache_is_nonempty_dir(dest: &Path) -> bool {
    dest.is_dir()
        && std::fs::read_dir(dest)
            .map(|mut entries| entries.next().is_some())
            .unwrap_or(false)
}

fn remove_cache_path(path: &Path) -> std::io::Result<()> {
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

/// 扫描根级或最多两层目录内的 SKILL.md。
pub fn scan_skills(root: &Path) -> Result<Vec<SkillEntry>> {
    if root.join("SKILL.md").exists() {
        return Ok(read_entry(root, PathBuf::from("."))?.into_iter().collect());
    }

    let mut level_one = std::fs::read_dir(root)?.collect::<std::io::Result<Vec<_>>>()?;
    level_one.sort_by_key(|entry| entry.file_name());
    let mut out = Vec::new();
    for entry in level_one {
        let dir1 = entry.path();
        if !dir1.is_dir() || dir1.file_name().is_some_and(|name| name == ".git") {
            continue;
        }
        if dir1.join("SKILL.md").exists() {
            if let Some(skill) = read_entry(&dir1, relative_path(root, &dir1))? {
                out.push(skill);
            }
        }

        let mut level_two = std::fs::read_dir(&dir1)?.collect::<std::io::Result<Vec<_>>>()?;
        level_two.sort_by_key(|entry| entry.file_name());
        for entry in level_two {
            let dir2 = entry.path();
            if !dir2.is_dir()
                || dir2.file_name().is_some_and(|name| name == ".git")
                || !dir2.join("SKILL.md").exists()
            {
                continue;
            }
            if let Some(skill) = read_entry(&dir2, relative_path(root, &dir2))? {
                out.push(skill);
            }
        }
    }
    out.sort_by(|a, b| a.name.cmp(&b.name).then(a.rel_path.cmp(&b.rel_path)));
    Ok(out)
}

fn relative_path(root: &Path, path: &Path) -> PathBuf {
    path.strip_prefix(root).unwrap_or(path).to_path_buf()
}

/// 解析 SKILL.md 中最小的 name/description frontmatter。
fn read_entry(dir: &Path, rel_path: PathBuf) -> Result<Option<SkillEntry>> {
    let path = dir.join("SKILL.md");
    let raw = std::fs::read_to_string(&path)?;
    let mut lines = raw.lines();
    let valid_start = lines.next().is_some_and(|line| line.trim() == "---");
    let mut name = None;
    let mut description = None;
    let mut closed = false;

    if valid_start {
        for line in lines {
            if line.trim() == "---" {
                closed = true;
                break;
            }
            if let Some((key, value)) = line.split_once(':') {
                let value = value.trim().trim_matches(['"', '\'']).trim().to_string();
                match key.trim() {
                    "name" if !value.is_empty() => name = Some(value),
                    "description" if !value.is_empty() => description = Some(value),
                    _ => {}
                }
            }
        }
    }

    match (valid_start, closed, name, description) {
        (true, true, Some(name), Some(description)) => Ok(Some(SkillEntry {
            name,
            description,
            rel_path,
        })),
        _ => {
            eprintln!(
                "跳过非法技能文件 {}：需要完整 frontmatter 以及非空 name/description",
                path.display()
            );
            Ok(None)
        }
    }
}

pub fn copy_dir(src: &Path, dst: &Path) -> Result<()> {
    std::fs::create_dir_all(dst)?;
    for entry in std::fs::read_dir(src)? {
        let entry = entry?;
        if entry.file_name() == ".git" {
            continue;
        }
        let target = dst.join(entry.file_name());
        if entry.file_type()?.is_dir() {
            copy_dir(&entry.path(), &target)?;
        } else {
            std::fs::copy(entry.path(), target)?;
        }
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::core::paths::Layout;
    use crate::core::source::{SourceSpec, parse_source};
    use std::process::Command;

    /// 造一个含两个技能的 bare 技能包仓库
    fn skill_repo() -> (tempfile::TempDir, std::path::PathBuf) {
        let tmp = tempfile::tempdir().unwrap();
        let work = tmp.path().join("work");
        let bare = tmp.path().join("bare.git");
        std::fs::create_dir_all(work.join("skills/alpha")).unwrap();
        std::fs::create_dir_all(work.join("skills/beta")).unwrap();
        std::fs::write(
            work.join("skills/alpha/SKILL.md"),
            "---\nname: alpha\ndescription: A 技能\n---\n# Alpha\n",
        )
        .unwrap();
        std::fs::write(
            work.join("skills/beta/SKILL.md"),
            "---\nname: beta\ndescription: B 技能\n---\n# Beta\n",
        )
        .unwrap();
        let run = |args: &[&str]| {
            assert!(
                Command::new("git")
                    .args(args)
                    .current_dir(&work)
                    .status()
                    .unwrap()
                    .success()
            )
        };
        run(&["init", "-b", "main"]);
        run(&["add", "."]);
        run(&[
            "-c",
            "user.email=t@t",
            "-c",
            "user.name=t",
            "commit",
            "-m",
            "c1",
        ]);
        run(&["clone", "--bare", ".", bare.to_str().unwrap()]);
        (tmp, bare)
    }

    #[test]
    fn ensure_cached_clones_once_then_reuses() {
        let (_t, bare) = skill_repo();
        let layout = Layout::at(tempfile::tempdir().unwrap().path().to_path_buf());
        let _ = parse_source(&format!("file://{}/org/pkg", bare.display())).unwrap();
        // file:// URL 的 key 形态走非 github host 分支；直接用内部 key 构造更稳：
        let spec = SourceSpec {
            key: "local-test/org/pkg".into(),
            url: Some(format!("file://{}", bare.display())),
            local_path: None,
        };
        let first = ensure_cached(&layout, &spec).unwrap();
        assert!(first.fresh);
        let second = ensure_cached(&layout, &spec).unwrap();
        assert!(!second.fresh);
        assert_eq!(first.path, second.path);
        assert_eq!(first.commit, second.commit);
    }

    #[test]
    fn scan_finds_multiple_skills_with_frontmatter() {
        let (_t, bare) = skill_repo();
        let layout = Layout::at(tempfile::tempdir().unwrap().path().to_path_buf());
        let spec = SourceSpec {
            key: "local-test/org/pkg".into(),
            url: Some(format!("file://{}", bare.display())),
            local_path: None,
        };
        let cached = ensure_cached(&layout, &spec).unwrap();
        let skills = scan_skills(&cached.path).unwrap();
        assert_eq!(skills.len(), 2);
        assert_eq!(skills[0].name, "alpha");
        assert_eq!(skills[0].rel_path, std::path::PathBuf::from("skills/alpha"));
        assert_eq!(skills[0].description, "A 技能");
    }

    #[test]
    fn scan_supports_root_level_single_skill() {
        let tmp = tempfile::tempdir().unwrap();
        std::fs::write(
            tmp.path().join("SKILL.md"),
            "---\nname: solo\ndescription: 单技能\n---\n",
        )
        .unwrap();
        let skills = scan_skills(tmp.path()).unwrap();
        assert_eq!(skills.len(), 1);
        assert_eq!(skills[0].rel_path, std::path::PathBuf::from("."));
    }

    #[test]
    fn ensure_cached_replaces_invalid_remote_cache() {
        let (_t, bare) = skill_repo();
        let root = tempfile::tempdir().unwrap();
        let layout = Layout::at(root.path().to_path_buf());
        let spec = SourceSpec {
            key: "local-test/invalid-remote".into(),
            url: Some(format!("file://{}", bare.display())),
            local_path: None,
        };
        let dest = layout.cache_dir(&spec.key);
        std::fs::create_dir_all(&dest).unwrap();

        let cached = ensure_cached(&layout, &spec).unwrap();

        assert!(cached.fresh);
        assert!(!cached.commit.is_empty());
        assert!(git::head_commit(&cached.path).is_ok());
    }

    #[test]
    fn ensure_cached_replaces_file_at_remote_cache_path() {
        let (_t, bare) = skill_repo();
        let root = tempfile::tempdir().unwrap();
        let layout = Layout::at(root.path().to_path_buf());
        let spec = SourceSpec {
            key: "local-test/file-remote".into(),
            url: Some(format!("file://{}", bare.display())),
            local_path: None,
        };
        let dest = layout.cache_dir(&spec.key);
        std::fs::create_dir_all(dest.parent().unwrap()).unwrap();
        std::fs::write(&dest, "半成品").unwrap();

        let cached = ensure_cached(&layout, &spec).unwrap();

        assert!(cached.fresh);
        assert!(cached.path.is_dir());
        assert!(!cached.commit.is_empty());
    }

    #[test]
    fn ensure_cached_copies_local_source_once_then_reuses() {
        let source = tempfile::tempdir().unwrap();
        std::fs::write(source.path().join("SKILL.md"), "local skill").unwrap();
        let root = tempfile::tempdir().unwrap();
        let layout = Layout::at(root.path().to_path_buf());
        let spec = SourceSpec {
            key: "local-test/local-source".into(),
            url: None,
            local_path: Some(source.path().to_path_buf()),
        };

        let first = ensure_cached(&layout, &spec).unwrap();
        let second = ensure_cached(&layout, &spec).unwrap();

        assert!(first.fresh);
        assert!(!second.fresh);
        assert_eq!(first.path, second.path);
        assert_eq!(
            std::fs::read_to_string(first.path.join("SKILL.md")).unwrap(),
            "local skill"
        );
    }

    #[test]
    fn scan_skips_invalid_frontmatter_without_directory_name_fallback() {
        let root = tempfile::tempdir().unwrap();
        for name in [
            "no-frontmatter",
            "incomplete",
            "missing-name",
            "missing-description",
        ] {
            std::fs::create_dir_all(root.path().join(name)).unwrap();
        }
        std::fs::write(
            root.path().join("no-frontmatter/SKILL.md"),
            "# no frontmatter\n",
        )
        .unwrap();
        std::fs::write(
            root.path().join("incomplete/SKILL.md"),
            "---\nname: incomplete\ndescription: bad\n",
        )
        .unwrap();
        std::fs::write(
            root.path().join("missing-name/SKILL.md"),
            "---\ndescription: missing name\n---\n",
        )
        .unwrap();
        std::fs::write(
            root.path().join("missing-description/SKILL.md"),
            "---\nname: missing-description\n---\n",
        )
        .unwrap();
        std::fs::create_dir_all(root.path().join("valid")).unwrap();
        std::fs::write(
            root.path().join("valid/SKILL.md"),
            "---\nname: valid\ndescription: valid skill\n---\n",
        )
        .unwrap();

        let skills = scan_skills(root.path()).unwrap();

        assert_eq!(skills.len(), 1);
        assert_eq!(skills[0].name, "valid");
    }

    #[test]
    fn scan_skips_second_level_git_directory() {
        let root = tempfile::tempdir().unwrap();
        std::fs::create_dir_all(root.path().join("group/.git")).unwrap();
        std::fs::create_dir_all(root.path().join("group/real")).unwrap();
        std::fs::write(
            root.path().join("group/.git/SKILL.md"),
            "---\nname: hidden\ndescription: hidden\n---\n",
        )
        .unwrap();
        std::fs::write(
            root.path().join("group/real/SKILL.md"),
            "---\nname: real\ndescription: real\n---\n",
        )
        .unwrap();

        let skills = scan_skills(root.path()).unwrap();

        assert_eq!(skills.len(), 1);
        assert_eq!(skills[0].name, "real");
    }
}

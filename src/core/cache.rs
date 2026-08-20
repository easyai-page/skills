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
    if dest.join(".git").exists() || (spec.local_path.is_some() && dest.exists()) {
        let commit = git::head_commit(&dest).unwrap_or_default();
        return Ok(Cached {
            path: dest,
            commit,
            fresh: false,
        });
    }

    std::fs::create_dir_all(
        dest.parent()
            .ok_or_else(|| Error::Msg(format!("缓存路径没有父目录: {}", dest.display())))?,
    )?;
    match (&spec.url, &spec.local_path) {
        (Some(url), None) => {
            let commit = git::shallow_clone(url, &dest)?;
            Ok(Cached {
                path: dest,
                commit,
                fresh: true,
            })
        }
        (None, Some(src)) => {
            copy_dir(src, &dest)?;
            Ok(Cached {
                path: dest,
                commit: String::new(),
                fresh: true,
            })
        }
        _ => Err(Error::Msg("source 缺少 url 或本地路径".into())),
    }
}

/// 扫描根级或最多两层目录内的 SKILL.md。
pub fn scan_skills(root: &Path) -> Result<Vec<SkillEntry>> {
    if root.join("SKILL.md").exists() {
        return Ok(vec![read_entry(root, PathBuf::from("."))?]);
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
            out.push(read_entry(&dir1, relative_path(root, &dir1))?);
        }

        let mut level_two = std::fs::read_dir(&dir1)?.collect::<std::io::Result<Vec<_>>>()?;
        level_two.sort_by_key(|entry| entry.file_name());
        for entry in level_two {
            let dir2 = entry.path();
            if dir2.is_dir() && dir2.join("SKILL.md").exists() {
                out.push(read_entry(&dir2, relative_path(root, &dir2))?);
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
fn read_entry(dir: &Path, rel_path: PathBuf) -> Result<SkillEntry> {
    let raw = std::fs::read_to_string(dir.join("SKILL.md"))?;
    let mut name = String::new();
    let mut description = String::new();
    let mut lines = raw.lines();
    if lines.next().is_some_and(|line| line.trim() == "---") {
        for line in lines {
            if line.trim() == "---" {
                break;
            }
            if let Some((key, value)) = line.split_once(':') {
                let value = value.trim().trim_matches(['"', '\'']).to_string();
                match key.trim() {
                    "name" => name = value,
                    "description" => description = value,
                    _ => {}
                }
            }
        }
    }
    if name.is_empty() {
        name = dir
            .file_name()
            .map(|name| name.to_string_lossy().into_owned())
            .unwrap_or_else(|| "unnamed".into());
    }
    Ok(SkillEntry {
        name,
        description,
        rel_path,
    })
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
}

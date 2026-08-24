//! `.skills-manifest` 的写入与本地修改检测。
//!
//! copy 副本根目录下的 manifest 有两重职责：
//! - remove 只依赖其**存在性**确认副本归属（见 remove::verify_copy_ownership）；
//! - `files` 字段（相对路径 → sha256）是 update 的校验基线：内容被改、文件缺失、
//!   副本内出现基线外文件，都视为本地修改——因为更新的 remove+recopy 会静默销毁它们。

use std::collections::BTreeMap;
use std::path::{Component, Path};

use sha2::Digest;

use super::error::{Error, Result};

/// copy 副本内的所有权标识文件名。与计划任务 15 的 `.skills-manifest` 约定同名。
pub(crate) const COPY_MANIFEST: &str = ".skills-manifest";

/// 在副本根写入 manifest（staging 完成后、原子提交前调用，install 与 update 共用）。
/// files 记录副本内每个文件的 sha256，键为相对路径（正斜杠，BTreeMap 保证有序）；
/// manifest 自身不入清单。
pub(crate) fn write_copy_manifest(root: &Path) -> Result<()> {
    let mut files = BTreeMap::new();
    hash_tree(root, root, &mut files)?;
    let body = serde_json::json!({ "version": 1, "manager": "skills", "files": files });
    std::fs::write(
        root.join(COPY_MANIFEST),
        serde_json::to_string_pretty(&body)?,
    )?;
    Ok(())
}

/// 对照 manifest 基线检测副本的本地修改。返回空 Vec 表示干净；
/// 否则每行一条人类可读的分歧说明。manifest 缺失/损坏/无 files 字段
/// （任务 15 之前的旧版安装）一律视为无校验基线，按分歧处理。
pub(crate) fn detect_local_modifications(dest: &Path) -> Result<Vec<String>> {
    let text = match std::fs::read_to_string(dest.join(COPY_MANIFEST)) {
        Ok(text) => text,
        Err(err) if err.kind() == std::io::ErrorKind::NotFound => {
            return Ok(vec![format!(
                "缺少 {COPY_MANIFEST}：无校验基线（旧版安装或非本工具副本）"
            )]);
        }
        Err(err) => return Err(Error::Io(err)),
    };
    let baseline = serde_json::from_str::<serde_json::Value>(&text)
        .ok()
        .and_then(|body| body.get("files").cloned())
        .and_then(|v| serde_json::from_value::<BTreeMap<String, String>>(v).ok());
    let Some(baseline) = baseline else {
        return Ok(vec![
            "manifest 无校验基线（旧版安装或文件损坏）；本次确认更新后将写入基线".into(),
        ]);
    };

    let mut lines = Vec::new();
    for (rel, expected) in &baseline {
        // manifest 是本工具写的，但用户可手改：拒绝越界路径组件，防止借校验读副本外的文件
        let Some(rel_path) = checked_relative_path(rel) else {
            lines.push(format!("基线条目含非法路径: {rel}"));
            continue;
        };
        match std::fs::read(dest.join(rel_path)) {
            Ok(bytes) => {
                if format!("{:x}", sha2::Sha256::digest(&bytes)) != *expected {
                    lines.push(format!("内容被修改: {rel}"));
                }
            }
            Err(err) if err.kind() == std::io::ErrorKind::NotFound => {
                lines.push(format!("文件缺失: {rel}"));
            }
            Err(err) => return Err(Error::Io(err)),
        }
    }
    let mut extras = Vec::new();
    collect_extras(dest, dest, &baseline, &mut extras)?;
    extras.sort_unstable();
    for rel in extras {
        lines.push(format!("副本内新增: {rel}（更新后将被删除）"));
    }
    Ok(lines)
}

/// 仅接受纯相对、全 Normal 组件的路径（拒绝绝对路径与 .. 越界）。
fn checked_relative_path(rel: &str) -> Option<&Path> {
    let path = Path::new(rel);
    path.components()
        .all(|c| matches!(c, Component::Normal(_)))
        .then_some(path)
}

/// 递归收集 dir 下每个文件（正斜杠相对路径 → sha256）。与 cache::copy_dir 的产物对齐：
/// 副本里只有目录与常规文件（copy 时符号链接已被解引用），manifest 自身跳过。
fn hash_tree(root: &Path, dir: &Path, out: &mut BTreeMap<String, String>) -> Result<()> {
    for entry in std::fs::read_dir(dir)? {
        let entry = entry?;
        let path = entry.path();
        if entry.file_type()?.is_dir() {
            hash_tree(root, &path, out)?;
            continue;
        }
        if dir == root && entry.file_name() == COPY_MANIFEST {
            continue;
        }
        let rel = forward_slash_relative(root, &path)?;
        out.insert(
            rel,
            format!("{:x}", sha2::Sha256::digest(std::fs::read(&path)?)),
        );
    }
    Ok(())
}

/// 递归收集副本内不在基线中的文件（manifest 自身除外）。
fn collect_extras(
    root: &Path,
    dir: &Path,
    baseline: &BTreeMap<String, String>,
    out: &mut Vec<String>,
) -> Result<()> {
    for entry in std::fs::read_dir(dir)? {
        let entry = entry?;
        let path = entry.path();
        if entry.file_type()?.is_dir() {
            collect_extras(root, &path, baseline, out)?;
            continue;
        }
        if dir == root && entry.file_name() == COPY_MANIFEST {
            continue;
        }
        let rel = forward_slash_relative(root, &path)?;
        if !baseline.contains_key(&rel) {
            out.push(rel);
        }
    }
    Ok(())
}

/// 相对路径转正斜杠字符串，保证跨平台与 manifest 中存储的键一致。
fn forward_slash_relative(root: &Path, path: &Path) -> Result<String> {
    let rel = path
        .strip_prefix(root)
        .map_err(|_| Error::Msg(format!("路径不在副本根内: {}", path.display())))?;
    let mut parts = Vec::new();
    for component in rel.components() {
        match component {
            Component::Normal(s) => parts.push(s.to_string_lossy().into_owned()),
            _ => {
                return Err(Error::Msg(format!(
                    "副本内含非法路径组件: {}",
                    rel.display()
                )));
            }
        }
    }
    Ok(parts.join("/"))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn setup_copy() -> (tempfile::TempDir, std::path::PathBuf) {
        let tmp = tempfile::tempdir().unwrap();
        let dest = tmp.path().join("alpha");
        std::fs::create_dir_all(dest.join("docs")).unwrap();
        std::fs::write(dest.join("SKILL.md"), "v1\n").unwrap();
        std::fs::write(dest.join("docs/deep.md"), "deep\n").unwrap();
        write_copy_manifest(&dest).unwrap();
        (tmp, dest)
    }

    #[test]
    fn written_manifest_detects_clean_then_flags_each_divergence_class() {
        let (_t, dest) = setup_copy();
        // 干净副本：无误报
        assert_eq!(
            detect_local_modifications(&dest).unwrap(),
            Vec::<String>::new()
        );

        // 内容被改
        std::fs::write(dest.join("SKILL.md"), "edited\n").unwrap();
        let lines = detect_local_modifications(&dest).unwrap();
        assert_eq!(lines, vec!["内容被修改: SKILL.md".to_string()]);
        std::fs::write(dest.join("SKILL.md"), "v1\n").unwrap();

        // 文件缺失
        std::fs::remove_file(dest.join("docs/deep.md")).unwrap();
        let lines = detect_local_modifications(&dest).unwrap();
        assert_eq!(lines, vec!["文件缺失: docs/deep.md".to_string()]);
        std::fs::write(dest.join("docs/deep.md"), "deep\n").unwrap();

        // 基线外新增
        std::fs::write(dest.join("notes.txt"), "mine\n").unwrap();
        let lines = detect_local_modifications(&dest).unwrap();
        assert_eq!(
            lines,
            vec!["副本内新增: notes.txt（更新后将被删除）".to_string()]
        );
        std::fs::remove_file(dest.join("notes.txt")).unwrap();
        assert_eq!(
            detect_local_modifications(&dest).unwrap(),
            Vec::<String>::new()
        );
    }

    #[test]
    fn missing_or_legacy_manifest_reports_no_baseline() {
        let (_t, dest) = setup_copy();
        std::fs::remove_file(dest.join(COPY_MANIFEST)).unwrap();
        let lines = detect_local_modifications(&dest).unwrap();
        assert!(lines[0].contains("无校验基线"), "{lines:?}");

        // 任务 15 之前的旧版 manifest：有归属标识但无 files 字段
        std::fs::write(
            dest.join(COPY_MANIFEST),
            "{\n  \"version\": 1,\n  \"manager\": \"skills\"\n}",
        )
        .unwrap();
        let lines = detect_local_modifications(&dest).unwrap();
        assert!(lines[0].contains("无校验基线"), "{lines:?}");

        // manifest 损坏（非法 JSON）同样按无基线处理
        std::fs::write(dest.join(COPY_MANIFEST), "{ 不是合法 json").unwrap();
        let lines = detect_local_modifications(&dest).unwrap();
        assert!(lines[0].contains("无校验基线"), "{lines:?}");
    }
}

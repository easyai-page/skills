# 递归技能扫描 实现计划

> **面向 AI 代理的工作者：** 必需子技能：使用 superpowers:subagent-driven-development（推荐）或 superpowers:executing-plans 逐任务实现此计划。步骤使用复选框（`- [ ]`）语法来跟踪进度。

**目标：** 让 `scan_skills` 支持任意深度的技能目录发现（修复 `mattpocock/skills` 这类三层分类目录结构扫描为空的问题）。

**架构：** 将 `src/core/cache.rs` 中固定两层的 `scan_skills` 重写为「递归遍历 + 目录剪枝」：发现含 `SKILL.md` 的目录即记录并剪枝（不再深入其子目录），跳过 `.` 开头的隐藏目录，用 `DirEntry::file_type()` 判断目录（不跟随符号链接），深度上限 8 层兜底。分组逻辑（`load_plugin_groupings` 及分类推断）、`SkillEntry`、`read_entry`、函数签名均不变，所有调用方零改动。

**技术栈：** Rust（std 库，无新依赖）；测试沿用就地 `#[cfg(test)]` 单测 + `tests/e2e.rs`（assert_cmd 真实二进制 + 本地 bare 仓库，不联网）。

**规格文档：** `docs/superpowers/specs/2026-10-10-recursive-skill-scan-design.md`

---

## 文件结构

- 修改：`src/core/cache.rs` — 重写 `scan_skills`（第 105-189 行区域），新增私有辅助函数 `scan_dir`；文件内 `#[cfg(test)] mod tests` 新增 4 个单测。
- 修改：`tests/e2e.rs` — 新增 1 个三层目录端到端回归测试。

---

### 任务 1：scan_skills 递归重写 + 单元测试

**文件：**
- 修改：`src/core/cache.rs`（`scan_skills` 函数，当前第 105-189 行；测试模块在当前第 371 行起）

- [ ] **步骤 1：编写 4 个失败的单元测试**

在 `src/core/cache.rs` 的 `#[cfg(test)] mod tests` 中（现有 `scan_groups_skills_via_marketplace_manifest` 测试之后）追加：

```rust
    #[test]
    fn scan_finds_skills_at_depth_three() {
        let root = tempfile::tempdir().unwrap();
        for rel in ["skills/engineering/tdd", "skills/productivity/grill-me"] {
            let dir = root.path().join(rel);
            std::fs::create_dir_all(&dir).unwrap();
            let name = dir.file_name().unwrap().to_str().unwrap().to_string();
            std::fs::write(
                dir.join("SKILL.md"),
                format!("---\nname: {name}\ndescription: {name} 技能\n---\n"),
            )
            .unwrap();
        }

        let skills = scan_skills(root.path()).unwrap();

        assert_eq!(skills.len(), 2);
        // 结果按 name 排序：grill-me 在 tdd 前
        assert_eq!(skills[0].name, "grill-me");
        assert_eq!(
            skills[0].rel_path,
            std::path::PathBuf::from("skills/productivity/grill-me")
        );
        assert_eq!(skills[1].name, "tdd");
        assert_eq!(
            skills[1].rel_path,
            std::path::PathBuf::from("skills/engineering/tdd")
        );
        // 三层结构 skills/<分类>/<名> 的分组：取第二级做分类
        assert_eq!(skills[0].group.as_deref(), Some("Productivity"));
        assert_eq!(skills[1].group.as_deref(), Some("Engineering"));
    }

    #[test]
    fn scan_prunes_nested_skill_md_inside_skill_directory() {
        // 技能目录内部的 SKILL.md 是素材/示例，发现技能目录即剪枝，不得重复计数
        let root = tempfile::tempdir().unwrap();
        std::fs::create_dir_all(root.path().join("skills/alpha/examples/inner")).unwrap();
        std::fs::write(
            root.path().join("skills/alpha/SKILL.md"),
            "---\nname: alpha\ndescription: A 技能\n---\n",
        )
        .unwrap();
        std::fs::write(
            root.path().join("skills/alpha/examples/inner/SKILL.md"),
            "---\nname: inner\ndescription: 技能目录内的素材\n---\n",
        )
        .unwrap();

        let skills = scan_skills(root.path()).unwrap();

        assert_eq!(skills.len(), 1);
        assert_eq!(skills[0].name, "alpha");
    }

    #[test]
    fn scan_skips_hidden_directories_at_any_depth() {
        let root = tempfile::tempdir().unwrap();
        std::fs::create_dir_all(root.path().join(".hidden/evil")).unwrap();
        std::fs::create_dir_all(root.path().join("group/.cache/inner")).unwrap();
        std::fs::create_dir_all(root.path().join("visible")).unwrap();
        for (rel, name) in [
            (".hidden/evil", "evil"),
            ("group/.cache/inner", "inner"),
            ("visible", "visible"),
        ] {
            std::fs::write(
                root.path().join(rel).join("SKILL.md"),
                format!("---\nname: {name}\ndescription: {name}\n---\n"),
            )
            .unwrap();
        }

        let skills = scan_skills(root.path()).unwrap();

        assert_eq!(skills.len(), 1);
        assert_eq!(skills[0].name, "visible");
    }

    #[cfg(unix)]
    #[test]
    fn scan_does_not_follow_directory_symlinks() {
        let root = tempfile::tempdir().unwrap();
        let outside = tempfile::tempdir().unwrap();
        std::fs::write(
            outside.path().join("SKILL.md"),
            "---\nname: outside\ndescription: 仓库外技能\n---\n",
        )
        .unwrap();
        // 指向仓库外的符号链接：不跟随则其中技能不会被发现
        std::os::unix::fs::symlink(outside.path(), root.path().join("linked")).unwrap();
        // 自引用链接环：跟随符号链接的实现会无限递归或重复计数
        std::os::unix::fs::symlink(".", root.path().join("loop")).unwrap();
        std::fs::create_dir_all(root.path().join("real")).unwrap();
        std::fs::write(
            root.path().join("real/SKILL.md"),
            "---\nname: real\ndescription: real\n---\n",
        )
        .unwrap();

        let skills = scan_skills(root.path()).unwrap();

        assert_eq!(skills.len(), 1);
        assert_eq!(skills[0].name, "real");
    }
```

- [ ] **步骤 2：运行测试验证失败**

运行：`cargo test --lib scan_ 2>&1 | tail -20`
预期：`scan_finds_skills_at_depth_three` 失败（三层结构扫不到，断言 `skills.len() == 2` 得到 0）；`scan_prunes_nested_skill_md_inside_skill_directory` 在旧实现下也失败（旧实现会把 `skills/alpha/examples/inner` 当成第二层之外的……注意旧实现只扫两层，`examples/inner` 在第三层扫不到但 `skills/alpha` 能扫到，该测试在旧实现下会通过——这是正常的，它锁定的是新实现的剪枝语义）；`scan_skips_hidden_directories_at_any_depth` 在旧实现下失败（旧实现只跳过 `.git`，`.hidden/evil` 会被发现）；`scan_does_not_follow_directory_symlinks` 在旧实现下失败（`dir1.is_dir()` 跟随链接，`linked` 会被发现）。至少有新失败的测试即确认测试有效。

- [ ] **步骤 3：重写 scan_skills 并新增 scan_dir 辅助函数**

将 `src/core/cache.rs` 第 105-189 行的 `scan_skills` 整体替换为：

```rust
/// 递归扫描任意深度目录内的 SKILL.md：发现技能目录即剪枝（不再深入其子目录），
/// 跳过 `.` 开头的隐藏目录，不跟随符号链接。
pub fn scan_skills(root: &Path) -> Result<Vec<SkillEntry>> {
    if root.join("SKILL.md").exists() {
        return Ok(read_entry(root, PathBuf::from("."))?.into_iter().collect());
    }

    /// 深度上限：防病态深层嵌套仓库的兜底；符号链接不跟随，天然免疫链接环。
    /// 相对路径最多 8 个组件（含技能目录自身），正常技能包远低于此。
    const MAX_DEPTH: usize = 8;
    let mut out = Vec::new();
    scan_dir(root, root, 0, MAX_DEPTH, &mut out)?;
    out.sort_by(|a, b| a.name.cmp(&b.name).then(a.rel_path.cmp(&b.rel_path)));

    let (plugin_map, has_manifest) = load_plugin_groupings(root);
    if has_manifest {
        for skill in &mut out {
            if let Some(group) = plugin_map.get(&skill.rel_path) {
                skill.group = Some(group.clone());
            } else {
                skill.group = Some("Other".to_string());
            }
        }
    } else {
        let categories: std::collections::HashSet<_> = out
            .iter()
            .filter_map(|s| {
                let comps: Vec<_> = s
                    .rel_path
                    .iter()
                    .map(|c| c.to_string_lossy().to_string())
                    .collect();
                if comps.len() >= 2 && comps[0] != "skills" {
                    Some(comps[0].clone())
                } else if comps.len() >= 3 && comps[0] == "skills" {
                    Some(comps[1].clone())
                } else {
                    None
                }
            })
            .collect();
        if categories.len() >= 2 {
            for skill in &mut out {
                let comps: Vec<_> = skill
                    .rel_path
                    .iter()
                    .map(|c| c.to_string_lossy().to_string())
                    .collect();
                let cat = if comps.len() >= 2 && comps[0] != "skills" {
                    Some(kebab_to_title(&comps[0]))
                } else if comps.len() >= 3 && comps[0] == "skills" {
                    Some(kebab_to_title(&comps[1]))
                } else {
                    Some("Other".to_string())
                };
                skill.group = cat;
            }
        }
    }

    Ok(out)
}

/// 递归扫描 `dir`（相对 `root` 深度为 `depth`）：含 SKILL.md 的目录记录后剪枝；
/// 跳过隐藏目录；`file_type()` 不跟随符号链接。
fn scan_dir(
    root: &Path,
    dir: &Path,
    depth: usize,
    max_depth: usize,
    out: &mut Vec<SkillEntry>,
) -> Result<()> {
    if depth >= max_depth {
        return Ok(());
    }
    let mut entries = std::fs::read_dir(dir)?.collect::<std::io::Result<Vec<_>>>()?;
    entries.sort_by_key(|entry| entry.file_name());
    for entry in entries {
        if !entry.file_type()?.is_dir() {
            continue;
        }
        // 跳过隐藏目录（.git、.agents、.claude-plugin 等）
        if entry.file_name().to_string_lossy().starts_with('.') {
            continue;
        }
        let path = entry.path();
        if path.join("SKILL.md").exists() {
            // frontmatter 非法时 read_entry 警告跳过，但目录仍按技能目录剪枝
            if let Some(skill) = read_entry(&path, relative_path(root, &path))? {
                out.push(skill);
            }
            continue;
        }
        scan_dir(root, &path, depth + 1, max_depth, out)?;
    }
    Ok(())
}
```

注意：上面 `scan_skills` 中从 `let (plugin_map, has_manifest) = ...` 到 `Ok(out)` 之前的分组代码与现有实现**逐字节相同**——替换时直接从旧函数里平移，不要重写。

- [ ] **步骤 4：运行全部单元测试验证通过**

运行：`cargo test --lib 2>&1 | tail -15`
预期：全部通过（含既有测试 `scan_finds_multiple_skills_with_frontmatter`、`scan_supports_root_level_single_skill`、`scan_skips_invalid_frontmatter_without_directory_name_fallback`、`scan_skips_second_level_git_directory`、`scan_groups_skills_via_marketplace_manifest`——`.git` 属于隐藏目录，既有行为被新规则覆盖，测试保持绿色）。

- [ ] **步骤 5：Commit**

```bash
git add src/core/cache.rs
git commit -m "feat: 技能扫描改为递归遍历+剪枝，支持任意深度目录结构"
```

---

### 任务 2：三层目录端到端回归测试

**文件：**
- 修改：`tests/e2e.rs`（追加到文件末尾）

- [ ] **步骤 1：编写端到端测试**

在 `tests/e2e.rs` 末尾追加（`git`、`skills`、`redirect_agents_target` 辅助函数已存在于该文件，直接复用）：

```rust
#[test]
fn add_discovers_skills_at_depth_three() {
    // 回归：mattpocock/skills 式三层结构 skills/<分类>/<技能>/SKILL.md 必须能被发现
    let tmp = tempfile::tempdir().unwrap();
    let home = tmp.path().join("home");
    let work = tmp.path().join("work");
    let bare = tmp.path().join("bare.git");
    for s in ["skills/engineering/tdd", "skills/productivity/grill-me"] {
        std::fs::create_dir_all(work.join(s)).unwrap();
        let name = s.rsplit('/').next().unwrap();
        std::fs::write(
            work.join(s).join("SKILL.md"),
            format!("---\nname: {name}\ndescription: 技能 {name}\n---\n# {name}\n"),
        )
        .unwrap();
    }
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
    let agents_dir = redirect_agents_target(&home);

    skills(&home)
        .args([
            "add",
            &format!("file://{}", bare.display()),
            "-s",
            "tdd",
            "-t",
            "global:agents",
            "--method",
            "copy",
            "-y",
        ])
        .assert()
        .success();
    assert!(agents_dir.join("tdd/SKILL.md").exists());

    let out = skills(&home).args(["list"]).output().unwrap();
    let stdout = String::from_utf8_lossy(&out.stdout);
    // list 只列已安装技能：装了 tdd，未装的 grill-me 不应出现
    assert!(stdout.contains("tdd"));
    assert!(!stdout.contains("grill-me"));
}
```

- [ ] **步骤 2：运行端到端测试验证通过**

运行：`cargo test --test e2e add_discovers_skills_at_depth_three`
预期：PASS。

- [ ] **步骤 3：Commit**

```bash
git add tests/e2e.rs
git commit -m "test: 新增三层目录结构技能发现的端到端回归测试"
```

---

### 任务 3：全量验证 + 测试机工作流

**文件：** 无新增修改（仅验证）。

- [ ] **步骤 1：格式化与 lint**

```bash
cargo fmt
cargo clippy 2>&1 | tail -10
```
预期：clippy 无警告。若 fmt 产生改动，`git add -A && git commit -m "style: cargo fmt"`。

- [ ] **步骤 2：本地全量测试**

运行：`cargo test 2>&1 | tail -20`
预期：单元 + 集成全部通过。

- [ ] **步骤 3：同步测试机并编译**（项目 AGENTS.md 规定的功能/修复必走流程）

```bash
rsync -a --exclude-from=scripts/rsync-exclude.txt ./ test@192.168.188.210:/mnt/sda3/deploy/skills/tools/
ssh test@192.168.188.210 'export PATH="$HOME/.cargo/bin:$PATH"; cd /mnt/sda3/deploy/skills/tools && cargo build'
```
预期：编译成功。

- [ ] **步骤 4：测试机真实场景验证**

在测试机上用本地目录源构造三层结构 fixture，跑完整链路（不依赖外网）：

```bash
ssh test@192.168.188.210 'set -e
export PATH="$HOME/.cargo/bin:$PATH"
export SKILLS_HOME=$(mktemp -d)
FIX=$(mktemp -d)
mkdir -p "$FIX/skills/engineering/tdd" "$FIX/skills/productivity/grill-me"
printf -- "---\nname: tdd\ndescription: TDD 技能\n---\n" > "$FIX/skills/engineering/tdd/SKILL.md"
printf -- "---\nname: grill-me\ndescription: 提问技能\n---\n" > "$FIX/skills/productivity/grill-me/SKILL.md"
cd /mnt/sda3/deploy/skills/tools
./target/debug/skills add "$FIX" -s tdd -t global:agents --method copy -y
./target/debug/skills list
./target/debug/skills remove tdd -y 2>/dev/null || ./target/debug/skills remove tdd
'
```
预期：`add` 成功（不再报「没有技能」），`list` 输出含 tdd，`remove` 成功。

- [ ] **步骤 5：确认工作区干净**

```bash
git status --short
```
预期：无未提交改动（各任务 commit 已完成；测试机流程通过后的总 commit 已由任务 1/2 完成，本步仅确认）。**禁止 `git push`**。

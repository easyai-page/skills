# 技能收藏（fav）功能实现计划

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** 新增 `skills fav` 收藏功能：只记录技能地址与功能描述快照（不安装），支持两级列表展示、删除、一键转安装，CLI/TUI/Web 三端覆盖。

**Architecture:** `registry.json` 新增 `favorites` 段（`BTreeMap<source_key, Favorite>`，serde default 向后兼容）；新建 `core::favorites` 模块承载 bookmark/unbookmark/fav_install 三个记录层操作；CLI 加 `fav` 子命令，TUI 加第四视图，Web 加 5 个 REST 端点与页面前签。安装复用既有 `install_skill` 管线，冲突确认与 409 重试沿用既有模式。

**Tech Stack:** Rust (edition 2024)、clap 4、serde/serde_json、gix、ratatui/crossterm、axum 0.7、tower（测试）。

**Spec:** `docs/superpowers/specs/2026-08-25-skill-favorites-design.md`

## Global Constraints

- 不新增任何依赖；不改 `Cargo.toml`。
- registry.json 向后兼容：新字段一律 `#[serde(default)]`，旧版文件（无 favorites 段）必须能加载。
- favorites 是纯记录层：不触碰 install/update/remove/git 的原子性、归属核验、本地修改保护语义；`fav rm` 不动缓存、不动 installs。
- 测试全程无网络：git fixture 用本地 bare 仓库 + `file://` URL；集成测试用 `SKILLS_HOME` 环境变量隔离；断言 source key 时从 registry.json 读真实值，不硬编码 file:// 的 key 推导规则（local 源除外，`local/<目录名>` 是确定规则，可硬编码）。
- 用户可见文案（CLI 输出、错误、页面）中文；代码标识符英文；解释「为什么」的注释中文。
- Web 前端遵守 index.html 顶部既有 XSS 约定：一律 textContent/value 注入 + addEventListener 绑定，禁 innerHTML 拼接与内联事件属性。
- 每个任务结束：`cargo test` 全绿 + `cargo fmt`；commit message 中文、conventional 前缀。
- TUI/Web 的既有取舍不改：Web handler 内同步执行 core 操作（与 run_update 一致）；TUI 向导的 suspend/resume 终端模式照搬 install_wizard。

---

### Task 1: registry 收藏数据模型 + NotBookmarked + TargetRec::to_target

**Files:**
- Modify: `src/core/error.rs`（加 NotBookmarked 变体）
- Modify: `src/core/registry.rs`（Favorite/FavSkill 类型、favorites 字段、TargetRec::to_target）
- Modify: `src/core/update.rs`（删除私有 to_target，改用 TargetRec::to_target）
- Modify: `src/core/remove.rs`（手写 match 换成 to_target）

**Interfaces:**
- Consumes: 现有 `Registry`、`TargetRec`、`super::paths::Target`
- Produces（后续任务依赖，签名以此为准）:
  - `registry::FavSkill { name: String, description: String, source_path: PathBuf }`
  - `registry::Favorite { url: Option<String>, local_path: Option<PathBuf>, commit: String, bookmarked_at: String, skills: Vec<FavSkill> }`
  - `Registry.favorites: BTreeMap<String, Favorite>`
  - `Error::NotBookmarked(String)`
  - `TargetRec::to_target(&self) -> crate::core::paths::Target`

- [ ] **Step 1: 写失败测试（registry.rs 的 `#[cfg(test)] mod tests` 内追加）**

```rust
#[test]
fn legacy_registry_without_favorites_loads() {
    // 向后兼容：收藏功能上线前的旧版 registry.json 没有 favorites 段
    let tmp = tempfile::tempdir().unwrap();
    let layout = Layout::at(tmp.path().to_path_buf());
    std::fs::write(
        layout.registry_path(),
        r#"{"version":1,"sources":{},"installs":[]}"#,
    )
    .unwrap();
    let reg = Registry::load(&layout).unwrap();
    assert!(reg.favorites.is_empty());
}

#[test]
fn favorites_roundtrip() {
    let tmp = tempfile::tempdir().unwrap();
    let layout = Layout::at(tmp.path().to_path_buf());
    let mut reg = Registry {
        version: 1,
        ..Default::default()
    };
    reg.favorites.insert(
        "github/o/r".into(),
        Favorite {
            url: Some("https://github.com/o/r".into()),
            local_path: None,
            commit: "deadbeef".into(),
            bookmarked_at: "2026-08-25T10:00:00Z".into(),
            skills: vec![FavSkill {
                name: "a".into(),
                description: "A".into(),
                source_path: "skills/a".into(),
            }],
        },
    );
    reg.save(&layout).unwrap();
    let loaded = Registry::load(&layout).unwrap();
    let fav = &loaded.favorites["github/o/r"];
    assert_eq!(fav.skills[0].name, "a");
    assert_eq!(fav.skills[0].source_path, PathBuf::from("skills/a"));
    let raw = std::fs::read_to_string(layout.registry_path()).unwrap();
    assert!(raw.contains("\"favorites\""));
}

#[test]
fn target_rec_to_target_maps_both_kinds() {
    assert_eq!(
        TargetRec::Global {
            name: "agents".into()
        }
        .to_target(),
        crate::core::paths::Target::Global {
            name: "agents".into()
        }
    );
    assert_eq!(
        TargetRec::Project { root: PathBuf::from("/x") }.to_target(),
        crate::core::paths::Target::Project {
            root: PathBuf::from("/x")
        }
    );
}
```

- [ ] **Step 2: 跑测试确认失败**

Run: `cargo test legacy_registry_without_favorites favorites_roundtrip target_rec_to_target`
Expected: FAIL（编译错误：favorites/Favorite/FavSkill/to_target 不存在）

- [ ] **Step 3: 实现数据模型**

`src/core/error.rs`：在 `NotInstalled` 变体后追加：

```rust
#[error("未收藏: {0}")]
NotBookmarked(String),
```

`src/core/registry.rs`：在 `Install` 结构体之后追加：

```rust
#[derive(Serialize, Deserialize, Clone, Debug)]
pub struct FavSkill {
    pub name: String,
    pub description: String,
    pub source_path: PathBuf, // 相对缓存根；fav install 直接喂给 install_skill
}

#[derive(Serialize, Deserialize, Clone, Debug)]
pub struct Favorite {
    #[serde(default)]
    pub url: Option<String>, // git 源有；本地源为 None
    #[serde(default)]
    pub local_path: Option<PathBuf>, // 本地源有；fav install 据此重建 SourceSpec
    pub commit: String,      // 收藏时缓存 HEAD 快照（本地源为空串）
    pub bookmarked_at: String, // RFC3339
    #[serde(default)]
    pub skills: Vec<FavSkill>,
}

impl TargetRec {
    pub fn to_target(&self) -> crate::core::paths::Target {
        match self {
            TargetRec::Global { name } => crate::core::paths::Target::Global { name: name.clone() },
            TargetRec::Project { root } => crate::core::paths::Target::Project { root: root.clone() },
        }
    }
}
```

`Registry` 结构体加字段（放在 installs 之后）：

```rust
#[serde(default)]
pub favorites: BTreeMap<String, Favorite>,
```

- [ ] **Step 4: 重构 update.rs 与 remove.rs 复用 to_target**

`src/core/update.rs`：删除文件末尾的私有 `fn to_target`，两处调用点改为方法调用：
- `execute_plan` 内 `let target = to_target(&d.target);` → `let target = d.target.to_target();`
- `pre_scan_local_modifications` 内 `let dest = to_target(&d.target).install_dir(cfg)?.join(&rec.skill);` → `let dest = d.target.to_target().install_dir(cfg)?.join(&rec.skill);`
- 清理因此空置的 import（编译器 unused 警告会指出具体哪个）。

`src/core/remove.rs`：`remove_install` 里的手写 match：

```rust
let t = match &rec.target {
    TargetRec::Global { name } => Target::Global { name: name.clone() },
    TargetRec::Project { root } => Target::Project { root: root.clone() },
};
```

替换为：

```rust
let t = rec.target.to_target();
```

- [ ] **Step 5: 跑全部测试确认通过**

Run: `cargo test`
Expected: 全绿（含既有测试——to_target 重构不改变的任何行为）

- [ ] **Step 6: 格式化并提交**

```bash
cargo fmt
git add src/core/error.rs src/core/registry.rs src/core/update.rs src/core/remove.rs
git commit -m "feat: registry 收藏数据模型 + TargetRec::to_target 收敛

Co-Authored-By: Claude Fable 5 <noreply@anthropic.com>"
```

---

### Task 2: core::favorites——bookmark / unbookmark / resolve_key / is_single_skill_repo

**Files:**
- Create: `src/core/favorites.rs`
- Modify: `src/core/mod.rs`（注册模块）
- Modify: `src/core/cache.rs`（SkillEntry.description 的 `#[allow(dead_code)]` 摘除）

**Interfaces:**
- Consumes: Task 1 的 `Favorite`/`FavSkill`/`Error::NotBookmarked`；`cache::ensure_cached`、`cache::scan_skills`、`cache::SkillEntry`；`source::SourceSpec`、`source::parse_source`
- Produces（后续任务依赖，签名以此为准）:
  - `favorites::bookmark(layout: &Layout, reg: &mut Registry, spec: &SourceSpec, skills: &[String]) -> Result<(String, usize)>`——返回 (source_key, 本次收藏技能数)；skills 空=整仓全量覆盖，非空=指定技能 upsert（不动其他）；调用方负责 `reg.save`
  - `favorites::unbookmark(reg: &mut Registry, source_key: &str, skills: &[String]) -> Result<usize>`——返回删除的技能数；skills 空=删整条 source 收藏
  - `favorites::resolve_key(reg: &Registry, input: &str) -> Result<String>`——先精确匹配 favorites key，再走 parse_source 规范化后匹配，都不中返回 `Error::NotBookmarked(原输入)`
  - `favorites::is_single_skill_repo(fav: &Favorite) -> bool`

- [ ] **Step 1: 写失败测试**

创建 `src/core/favorites.rs`，先只写测试模块（实现紧接着补，但先跑出编译失败）：

```rust
//! 技能收藏：只记录地址与功能描述的快照，不安装。收藏与安装是正交的两份记录。

use std::path::{Path, PathBuf};

use super::cache;
use super::error::{Error, Result};
use super::paths::Layout;
use super::registry::{FavSkill, Favorite, Registry, SourceRecord};
use super::source::SourceSpec;

#[cfg(test)]
mod tests {
    use super::*;
    use crate::core::source::parse_source;

    /// 本地双技能源（local 源每次 ensure_cached 都重拷，天然离线）
    fn setup() -> (tempfile::TempDir, Layout, Registry, PathBuf) {
        let tmp = tempfile::tempdir().unwrap();
        let layout = Layout::at(tmp.path().join(".skills"));
        let src = tmp.path().join("src");
        std::fs::create_dir_all(src.join("skills/alpha")).unwrap();
        std::fs::create_dir_all(src.join("skills/beta")).unwrap();
        std::fs::write(
            src.join("skills/alpha/SKILL.md"),
            "---\nname: alpha\ndescription: A 技能\n---\n",
        )
        .unwrap();
        std::fs::write(
            src.join("skills/beta/SKILL.md"),
            "---\nname: beta\ndescription: B 技能\n---\n",
        )
        .unwrap();
        let reg = Registry {
            version: 1,
            ..Default::default()
        };
        (tmp, layout, reg, src)
    }

    fn local_spec(src: &Path) -> SourceSpec {
        parse_source(src.to_str().unwrap()).unwrap()
    }

    #[test]
    fn bookmark_whole_repo_snapshots_all_skills() {
        let (_t, layout, mut reg, src) = setup();
        let (key, n) = bookmark(&layout, &mut reg, &local_spec(&src), &[]).unwrap();
        assert_eq!(key, "local/src");
        assert_eq!(n, 2);
        let fav = &reg.favorites["local/src"];
        assert_eq!(fav.skills.len(), 2);
        assert_eq!(fav.skills[0].name, "alpha");
        assert_eq!(fav.skills[0].description, "A 技能");
        assert_eq!(fav.skills[0].source_path, PathBuf::from("skills/alpha"));
        assert_eq!(fav.local_path.as_deref(), Some(src.as_path()));
        assert!(fav.url.is_none());
        // 收藏同时登记 sources（与 add 相同逻辑），update 体系能看到缓存
        assert!(reg.sources.contains_key("local/src"));
    }

    #[test]
    fn bookmark_single_skill_upserts_without_touching_others() {
        let (_t, layout, mut reg, src) = setup();
        bookmark(&layout, &mut reg, &local_spec(&src), &["alpha".into()]).unwrap();
        assert_eq!(reg.favorites["local/src"].skills.len(), 1);
        // 再收 beta：upsert 进同一条目，不动 alpha
        bookmark(&layout, &mut reg, &local_spec(&src), &["beta".into()]).unwrap();
        let fav = &reg.favorites["local/src"];
        assert_eq!(fav.skills.len(), 2);
        // 改掉 alpha 描述后重收 alpha：只刷新 alpha 条目
        std::fs::write(
            src.join("skills/alpha/SKILL.md"),
            "---\nname: alpha\ndescription: A2 技能\n---\n",
        )
        .unwrap();
        bookmark(&layout, &mut reg, &local_spec(&src), &["alpha".into()]).unwrap();
        let fav = &reg.favorites["local/src"];
        let alpha = fav.skills.iter().find(|s| s.name == "alpha").unwrap();
        assert_eq!(alpha.description, "A2 技能");
        assert_eq!(fav.skills.len(), 2);
    }

    #[test]
    fn rebookmark_whole_repo_overwrites_snapshot() {
        let (_t, layout, mut reg, src) = setup();
        bookmark(&layout, &mut reg, &local_spec(&src), &[]).unwrap();
        // 源里删掉 beta：整仓重收藏 = 覆盖快照（手动刷新手段）
        std::fs::remove_dir_all(src.join("skills/beta")).unwrap();
        let (_key, n) = bookmark(&layout, &mut reg, &local_spec(&src), &[]).unwrap();
        assert_eq!(n, 1);
        let fav = &reg.favorites["local/src"];
        assert_eq!(fav.skills.len(), 1);
        assert_eq!(fav.skills[0].name, "alpha");
    }

    #[test]
    fn bookmark_unknown_skill_errors_and_writes_nothing() {
        let (_t, layout, mut reg, src) = setup();
        let err = bookmark(&layout, &mut reg, &local_spec(&src), &["nope".into()]).unwrap_err();
        assert!(format!("{err}").contains("仓库中无技能 nope"), "{err}");
        assert!(reg.favorites.is_empty());
    }

    #[test]
    fn unbookmark_single_then_cascade_when_empty() {
        let (_t, layout, mut reg, src) = setup();
        bookmark(&layout, &mut reg, &local_spec(&src), &[]).unwrap();
        let n = unbookmark(&mut reg, "local/src", &["alpha".into()]).unwrap();
        assert_eq!(n, 1);
        assert_eq!(reg.favorites["local/src"].skills.len(), 1);
        // 删光技能：级联删 source 条目
        let n = unbookmark(&mut reg, "local/src", &["beta".into()]).unwrap();
        assert_eq!(n, 1);
        assert!(!reg.favorites.contains_key("local/src"));
    }

    #[test]
    fn unbookmark_whole_source_and_missing_errors() {
        let (_t, layout, mut reg, src) = setup();
        bookmark(&layout, &mut reg, &local_spec(&src), &[]).unwrap();
        let n = unbookmark(&mut reg, "local/src", &[]).unwrap();
        assert_eq!(n, 2);
        assert!(reg.favorites.is_empty());
        // 未收藏的 source / 不存在的技能名：NotBookmarked
        assert!(matches!(
            unbookmark(&mut reg, "local/src", &[]),
            Err(Error::NotBookmarked(_))
        ));
        bookmark(&layout, &mut reg, &local_spec(&src), &[]).unwrap();
        assert!(matches!(
            unbookmark(&mut reg, "local/src", &["nope".into()]),
            Err(Error::NotBookmarked(_))
        ));
        // 部分删除遇未知名：已删的不回滚（逐条语义与 remove 一致），但整体报错
        assert!(matches!(
            unbookmark(&mut reg, "local/src", &["alpha".into(), "nope".into()]),
            Err(Error::NotBookmarked(_))
        ));
        assert_eq!(reg.favorites["local/src"].skills.len(), 1);
    }

    #[test]
    fn resolve_key_accepts_exact_key_or_source_expression() {
        let (_t, layout, mut reg, src) = setup();
        bookmark(&layout, &mut reg, &local_spec(&src), &[]).unwrap();
        // 精确 key
        assert_eq!(resolve_key(&reg, "local/src").unwrap(), "local/src");
        // 可解析的 source 表达式（本地绝对路径）
        assert_eq!(
            resolve_key(&reg, src.to_str().unwrap()).unwrap(),
            "local/src"
        );
        // 未收藏
        assert!(matches!(
            resolve_key(&reg, "github/x/y"),
            Err(Error::NotBookmarked(_))
        ));
    }

    #[test]
    fn single_skill_repo_detection() {
        let tmp = tempfile::tempdir().unwrap();
        let layout = Layout::at(tmp.path().join(".skills"));
        let mut reg = Registry {
            version: 1,
            ..Default::default()
        };
        // 根级 SKILL.md 的单技能仓库
        let solo = tmp.path().join("solo");
        std::fs::create_dir_all(&solo).unwrap();
        std::fs::write(
            solo.join("SKILL.md"),
            "---\nname: solo\ndescription: 单技能\n---\n",
        )
        .unwrap();
        bookmark(&layout, &mut reg, &local_spec(&solo), &[]).unwrap();
        assert!(is_single_skill_repo(&reg.favorites["local/solo"]));
        // 多技能仓库不是
        let (_t2, layout2, mut reg2, src2) = setup();
        bookmark(&layout2, &mut reg2, &local_spec(&src2), &[]).unwrap();
        assert!(!is_single_skill_repo(&reg2.favorites["local/src"]));
    }
}
```

- [ ] **Step 2: 跑测试确认失败**

Run: `cargo test favorites`
Expected: FAIL（编译错误：bookmark/unbookmark/resolve_key/is_single_skill_repo 未定义）

- [ ] **Step 3: 实现 favorites.rs 的四个函数**

在 `src/core/favorites.rs` 顶部（测试模块之前）追加实现：

```rust
fn to_fav_skill(e: &cache::SkillEntry) -> FavSkill {
    FavSkill {
        name: e.name.clone(),
        description: e.description.clone(),
        source_path: e.rel_path.clone(),
    }
}

/// 收藏 source：skills 为空 = 整仓全量覆盖快照；非空 = 指定技能 upsert（不动其他）。
/// 同时把 source 登记进 sources 表（or_insert，与 add 相同逻辑），让缓存目录有唯一权威登记处。
/// 失败（clone/扫描/校验）时 favorite 不落盘，由调用方决定何时 save。
pub fn bookmark(
    layout: &Layout,
    reg: &mut Registry,
    spec: &SourceSpec,
    skills: &[String],
) -> Result<(String, usize)> {
    let cached = cache::ensure_cached(layout, spec)?;
    let all = cache::scan_skills(&cached.path)?;
    let picked: Vec<FavSkill> = if skills.is_empty() {
        all.iter().map(to_fav_skill).collect()
    } else {
        let mut out = Vec::new();
        for name in skills {
            let entry = all
                .iter()
                .find(|e| &e.name == name)
                .ok_or_else(|| Error::Msg(format!("仓库中无技能 {name}")))?;
            out.push(to_fav_skill(entry));
        }
        out
    };
    reg.sources
        .entry(spec.key.clone())
        .or_insert(SourceRecord {
            url: spec.url.clone().unwrap_or_default(),
            commit: cached.commit.clone(),
            fetched_at: chrono::Utc::now().to_rfc3339(),
            auto_update: None,
        });
    let count = picked.len();
    let now = chrono::Utc::now().to_rfc3339();
    if reg.favorites.contains_key(&spec.key) && !skills.is_empty() {
        // upsert：只覆盖指定技能，不动同 source 下其他已收藏技能
        let fav = reg.favorites.get_mut(&spec.key).expect("刚检查过存在");
        for s in picked {
            match fav.skills.iter_mut().find(|f| f.name == s.name) {
                Some(slot) => *slot = s,
                None => fav.skills.push(s),
            }
        }
        fav.skills.sort_by(|a, b| a.name.cmp(&b.name));
        fav.commit = cached.commit.clone();
        fav.bookmarked_at = now;
    } else {
        reg.favorites.insert(
            spec.key.clone(),
            Favorite {
                url: spec.url.clone(),
                local_path: spec.local_path.clone(),
                commit: cached.commit.clone(),
                bookmarked_at: now,
                skills: picked,
            },
        );
    }
    Ok((spec.key.clone(), count))
}

/// 删除收藏：skills 为空删整条 source；非空只删列出的（遇未知名报错，已删的不回滚）。
/// 技能删光则级联删 source 条目。不动缓存、不动 installs。
pub fn unbookmark(reg: &mut Registry, source_key: &str, skills: &[String]) -> Result<usize> {
    if !reg.favorites.contains_key(source_key) {
        return Err(Error::NotBookmarked(source_key.into()));
    }
    if skills.is_empty() {
        let n = reg.favorites[source_key].skills.len();
        reg.favorites.remove(source_key);
        return Ok(n);
    }
    let mut removed = 0;
    for name in skills {
        let fav = reg.favorites.get_mut(source_key).expect("刚检查过存在");
        let before = fav.skills.len();
        fav.skills.retain(|s| &s.name != name);
        if fav.skills.len() == before {
            return Err(Error::NotBookmarked(format!(
                "{source_key} 中无技能 {name}"
            )));
        }
        removed += 1;
    }
    if reg.favorites[source_key].skills.is_empty() {
        reg.favorites.remove(source_key);
    }
    Ok(removed)
}

/// rm/install 的 source 入参解析：先精确匹配 favorites key（如 github/o/r），
/// 不中再走 parse_source 规范化（贴 URL / 本地路径也行）。
pub fn resolve_key(reg: &Registry, input: &str) -> Result<String> {
    if reg.favorites.contains_key(input) {
        return Ok(input.into());
    }
    if let Ok(spec) = super::source::parse_source(input)
        && reg.favorites.contains_key(&spec.key)
    {
        return Ok(spec.key);
    }
    Err(Error::NotBookmarked(input.into()))
}

/// 单技能仓库判定：整仓只有一个根级技能（source_path 为 "."）。
/// 列表/视图据此把用途挂到一级行、不展开二级。
pub fn is_single_skill_repo(fav: &Favorite) -> bool {
    fav.skills.len() == 1 && fav.skills[0].source_path == Path::new(".")
}
```

`src/core/mod.rs`：加一行（按字母序放在 error 后）：

```rust
pub mod favorites;
```

`src/core/cache.rs`：`SkillEntry.description` 字段的 `#[allow(dead_code)]` 与相关注释摘除——favorites 开始消费该字段：

```rust
pub struct SkillEntry {
    pub name: String,
    pub description: String, // 解析自 SKILL.md frontmatter；收藏快照与展示层消费
    pub rel_path: PathBuf,
}
```

- [ ] **Step 4: 跑测试确认通过**

Run: `cargo test favorites`
Expected: PASS（8 个新测试 + 既有测试全绿）

- [ ] **Step 5: 格式化并提交**

```bash
cargo fmt
git add src/core/favorites.rs src/core/mod.rs src/core/cache.rs
git commit -m "feat: core::favorites 收藏/删除/键解析/单技能仓库判定

Co-Authored-By: Claude Fable 5 <noreply@anthropic.com>"
```

---

### Task 3: core::favorites——fav_install（含缓存自愈）

**Files:**
- Modify: `src/core/favorites.rs`

**Interfaces:**
- Consumes: Task 1 的 `Favorite`；Task 2 的模块；`install::install_skill`、`cache::ensure_cached`、`registry::SourceRecord`
- Produces（后续任务依赖，签名以此为准）:
  - `favorites::fav_install(layout: &Layout, cfg: &Config, reg: &mut Registry, source_key: &str, skill: &str, target: &Target, method: Method) -> Result<Install>`——从收藏快照取 source_path；缓存缺失时凭快照的 url/local_path 重建 SourceSpec 并 ensure_cached 自愈（git 源以 `.git` 存在为完整标准）；`Error::Conflict` 原样上抛交由前端决策；调用方负责落盘

- [ ] **Step 1: 写失败测试（追加到 favorites.rs 的 tests 模块）**

```rust
    use crate::core::config::Config;
    use crate::core::install::COPY_MANIFEST;
    use crate::core::paths::Target;
    use crate::core::registry::Method;

    /// setup + Config（agents target 指向临时目录），返回可复制安装的环境
    fn setup_with_cfg() -> (tempfile::TempDir, Layout, Config, Registry, PathBuf) {
        let (tmp, layout, reg, src) = setup();
        let mut cfg = Config::default();
        cfg.targets
            .insert("agents".into(), tmp.path().join("global/agents"));
        (tmp, layout, cfg, reg, src)
    }

    #[test]
    fn fav_install_installs_from_snapshot() {
        let (tmp, layout, cfg, mut reg, src) = setup_with_cfg();
        bookmark(&layout, &mut reg, &local_spec(&src), &[]).unwrap();
        let rec = fav_install(
            &layout,
            &cfg,
            &mut reg,
            "local/src",
            "alpha",
            &Target::Global {
                name: "agents".into(),
            },
            Method::Copy,
        )
        .unwrap();
        let dest = tmp.path().join("global/agents/alpha");
        assert!(dest.join("SKILL.md").exists());
        assert!(dest.join(COPY_MANIFEST).exists(), "copy 副本必须带归属标识");
        assert_eq!(rec.source, "local/src");
        assert_eq!(reg.installs.len(), 1);
    }

    #[test]
    fn fav_install_unknown_favorite_or_skill_errors() {
        let (_t, layout, cfg, mut reg, src) = setup_with_cfg();
        bookmark(&layout, &mut reg, &local_spec(&src), &[]).unwrap();
        let t = Target::Global {
            name: "agents".into(),
        };
        assert!(matches!(
            fav_install(&layout, &cfg, &mut reg, "local/nope", "alpha", &t, Method::Copy),
            Err(Error::NotBookmarked(_))
        ));
        assert!(matches!(
            fav_install(&layout, &cfg, &mut reg, "local/src", "nope", &t, Method::Copy),
            Err(Error::NotBookmarked(_))
        ));
        assert!(reg.installs.is_empty());
    }

    #[test]
    fn fav_install_heals_missing_cache() {
        let (tmp, layout, cfg, mut reg, src) = setup_with_cfg();
        bookmark(&layout, &mut reg, &local_spec(&src), &[]).unwrap();
        // 缓存被手动删除：fav install 凭快照的 local_path 重建缓存
        std::fs::remove_dir_all(layout.cache_dir("local/src")).unwrap();
        fav_install(
            &layout,
            &cfg,
            &mut reg,
            "local/src",
            "beta",
            &Target::Global {
                name: "agents".into(),
            },
            Method::Copy,
        )
        .unwrap();
        assert!(tmp.path().join("global/agents/beta/SKILL.md").exists());
    }

    #[test]
    fn fav_install_errors_when_cache_and_local_source_both_gone() {
        let (_t, layout, cfg, mut reg, src) = setup_with_cfg();
        bookmark(&layout, &mut reg, &local_spec(&src), &[]).unwrap();
        std::fs::remove_dir_all(layout.cache_dir("local/src")).unwrap();
        std::fs::remove_dir_all(&src).unwrap(); // 本地源本身也没了，无法自愈
        let err = fav_install(
            &layout,
            &cfg,
            &mut reg,
            "local/src",
            "alpha",
            &Target::Global {
                name: "agents".into(),
            },
            Method::Copy,
        )
        .unwrap_err();
        assert!(format!("{err}").contains("本地源"), "{err}");
        assert!(reg.installs.is_empty());
    }

    #[test]
    fn fav_install_conflict_returns_decision_request() {
        let (_t, layout, cfg, mut reg, src) = setup_with_cfg();
        bookmark(&layout, &mut reg, &local_spec(&src), &[]).unwrap();
        let t = Target::Global {
            name: "agents".into(),
        };
        fav_install(&layout, &cfg, &mut reg, "local/src", "alpha", &t, Method::Copy).unwrap();
        let err = fav_install(&layout, &cfg, &mut reg, "local/src", "alpha", &t, Method::Copy)
            .unwrap_err();
        assert!(matches!(err, Error::Conflict(_)), "{err}");
    }
```

- [ ] **Step 2: 跑测试确认失败**

Run: `cargo test fav_install`
Expected: FAIL（编译错误：fav_install 未定义；注意 Config/Target/Method/COPY_MANIFEST 的 use 也要先缺着报错）

- [ ] **Step 3: 实现 fav_install**

在 `src/core/favorites.rs` 的实现区追加（use 区补 `use super::config::Config;`、`use super::install;`、`use super::paths::Target;`、`use super::registry::{Install, Method};`——合并进已有 use 行）：

```rust
/// 从收藏安装一个技能：source_path 取自收藏快照（不重扫仓库）。
/// 缓存缺失时凭快照的 url/local_path 重建 SourceSpec 并 ensure_cached 自愈（仅缺失时）；
/// 自愈后 HEAD 可能前进，同步刷新 sources 记录与收藏快照的 commit。
/// Error::Conflict 原样上抛交由前端决策；调用方负责落盘 registry。
pub fn fav_install(
    layout: &Layout,
    cfg: &Config,
    reg: &mut Registry,
    source_key: &str,
    skill: &str,
    target: &Target,
    method: Method,
) -> Result<Install> {
    let (source_path, url, local_path) = {
        let fav = reg
            .favorites
            .get(source_key)
            .ok_or_else(|| Error::NotBookmarked(source_key.into()))?;
        let s = fav
            .skills
            .iter()
            .find(|s| s.name == skill)
            .ok_or_else(|| Error::NotBookmarked(format!("{source_key} 中无技能 {skill}")))?;
        (s.source_path.clone(), fav.url.clone(), fav.local_path.clone())
    };
    // 缓存自愈：git 源以 .git 存在为完整标准，本地源以目录存在为准
    let cache = layout.cache_dir(source_key);
    let intact = if url.is_some() {
        cache.join(".git").is_dir()
    } else {
        cache.is_dir()
    };
    if !intact {
        if url.is_none()
            && local_path
                .as_ref()
                .is_some_and(|p| !p.is_dir())
        {
            return Err(Error::Msg(format!(
                "本地源 {} 已不存在，无法重建缓存；请重新 fav 有效路径",
                local_path.unwrap_or_default().display()
            )));
        }
        let spec = SourceSpec {
            key: source_key.into(),
            url,
            local_path,
        };
        let cached = cache::ensure_cached(layout, &spec)?;
        let now = chrono::Utc::now().to_rfc3339();
        let src = reg
            .sources
            .entry(source_key.into())
            .or_insert(SourceRecord {
                url: spec.url.clone().unwrap_or_default(),
                commit: String::new(),
                fetched_at: now.clone(),
                auto_update: None,
            });
        src.commit = cached.commit.clone();
        src.fetched_at = now;
        if let Some(fav) = reg.favorites.get_mut(source_key) {
            fav.commit = cached.commit.clone();
        }
    }
    let commit = reg
        .sources
        .get(source_key)
        .map(|s| s.commit.clone())
        .unwrap_or_default();
    install::install_skill(
        layout,
        cfg,
        reg,
        source_key,
        skill,
        &source_path.to_string_lossy(),
        target,
        method,
        &commit,
    )
}
```

- [ ] **Step 4: 跑测试确认通过**

Run: `cargo test favorites`
Expected: PASS（含 Task 2 的测试全部保持绿）

- [ ] **Step 5: 格式化并提交**

```bash
cargo fmt
git add src/core/favorites.rs
git commit -m "feat: fav_install 从收藏安装（含缓存自愈）

Co-Authored-By: Claude Fable 5 <noreply@anthropic.com>"
```

---

### Task 4: CLI fav 子命令——收藏 / 两级列表 / 删除

**Files:**
- Modify: `src/cli/mod.rs`（Cmd::Fav + FavSub）
- Modify: `src/cli/commands.rs`（Fav 分支 + print_favorites）
- Test: `tests/cli_smoke.rs`（追加解析与本地源收藏测试）

**Interfaces:**
- Consumes: Task 2 的 `bookmark`/`unbookmark`/`resolve_key`/`is_single_skill_repo`；`source::parse_source`；`Registry`
- Produces（后续任务依赖）:
  - CLI 形态 `skills fav [source] [--skill 名...]` / `skills fav rm <source> [--skill 名...]`；Task 5 在此之上加 `fav install`
  - `commands.rs` 内私有 `fn print_favorites(reg: &Registry)`——两级列表输出

- [ ] **Step 1: 写失败测试（追加到 tests/cli_smoke.rs 末尾）**

```rust
#[test]
fn fav_help_and_arg_validation() {
    let tmp = tempfile::tempdir().unwrap();
    let home = tmp.path().join("home");
    // 帮助里能看到 fav
    let out = Command::cargo_bin("skills")
        .unwrap()
        .arg("--help")
        .output()
        .unwrap();
    let s = String::from_utf8(out.stdout).unwrap();
    assert!(s.contains("fav"), "{s}");
    // fav rm / fav install 缺 source：clap 报错非零退出
    fail(&home, &["fav", "rm"]);
    fail(&home, &["fav", "install"]);
    // --skill 必须搭配 source
    let err = fail(&home, &["fav", "--skill", "x"]);
    assert!(err.contains("--skill 需搭配 source"), "{err}");
    // 空收藏列表
    let out = ok(&home, &["fav"]);
    assert!(out.contains("（无收藏）"), "{out}");
}

#[test]
fn fav_bookmark_list_rm_local_source() {
    let tmp = tempfile::tempdir().unwrap();
    let home = tmp.path().join("home");
    let src = tmp.path().join("src");
    std::fs::create_dir_all(src.join("skills/alpha")).unwrap();
    std::fs::write(
        src.join("skills/alpha/SKILL.md"),
        "---\nname: alpha\ndescription: A 技能\n---\n",
    )
    .unwrap();
    // 收藏（本地路径 source；local/<目录名> 是确定规则，key 可硬编码）
    let out = ok(&home, &["fav", src.to_str().unwrap()]);
    assert!(out.contains("已收藏 local/src（1 个技能）"), "{out}");
    // 两级列表：本地源无 commit，一级行显示 (本地源)
    let out = ok(&home, &["fav"]);
    assert!(out.contains("local/src"), "{out}");
    assert!(out.contains("(本地源)"), "{out}");
    assert!(out.contains("└─ alpha — A 技能"), "{out}");
    // 删除整包
    let out = ok(&home, &["fav", "rm", "local/src"]);
    assert!(out.contains("已删除收藏 local/src"), "{out}");
    let out = ok(&home, &["fav"]);
    assert!(out.contains("（无收藏）"), "{out}");
}
```

- [ ] **Step 2: 跑测试确认失败**

Run: `cargo test --test cli_smoke fav`
Expected: FAIL（`fav` 不是已知子命令 / 编译错误）

- [ ] **Step 3: 实现 CLI 解析（src/cli/mod.rs）**

`Cmd` 枚举在 `Ui` 之前插入：

```rust
    /// 收藏技能（只记录地址与功能，不安装）；无参数时列出收藏
    #[command(args_conflicts_with_subcommands = true)]
    Fav {
        source: Option<String>,
        #[arg(short, long)]
        skill: Vec<String>,
        #[command(subcommand)]
        sub: Option<FavSub>,
    },
```

文件末尾追加：

```rust
#[derive(Subcommand)]
pub enum FavSub {
    /// 删除收藏（--skill 删指定技能，否则删整包；不动缓存与已安装副本）
    Rm {
        source: String,
        #[arg(short, long)]
        skill: Vec<String>,
    },
    /// 从收藏安装（Task 5 实现分发，本任务先建结构）
    Install {
        source: String,
        #[arg(short, long)]
        skill: Vec<String>,
        #[arg(short, long)]
        target: Vec<String>,
        #[arg(short = 'g', long)]
        global: bool,
        #[arg(long, value_enum)]
        method: Option<MethodArg>,
        #[arg(short = 'y', long)]
        yes: bool,
    },
}
```

- [ ] **Step 4: 实现分发（src/cli/commands.rs）**

use 区加 `favorites`（并入 `use crate::core::{...}` 列表）。

`run` 的 match 中、`Some(Cmd::Config { .. }) => unreachable!(...)` 之前插入 Fav 分支。本任务只实现列表/收藏/删除；`Install` 暂时返回明示错误，Task 5 替换为真实现：

```rust
        Some(Cmd::Fav { source, skill, sub }) => {
            let mut reg = Registry::load(&layout)?;
            match (sub, source) {
                (Some(FavSub::Rm { source, skill }), _) => {
                    let key = favorites::resolve_key(&reg, &source)?;
                    if skill.is_empty() {
                        let n = favorites::unbookmark(&mut reg, &key, &[])?;
                        reg.save(&layout)?;
                        println!("已删除收藏 {key}（{n} 个技能）");
                    } else {
                        let n = favorites::unbookmark(&mut reg, &key, &skill)?;
                        reg.save(&layout)?;
                        println!("已从 {key} 删除 {n} 个技能收藏");
                    }
                    Ok(())
                }
                (Some(FavSub::Install { .. }), _) => Err(Error::Msg(
                    "fav install 尚未实现（下一任务）".into(),
                )),
                (None, Some(source)) => {
                    let spec = parse_source(&source)?;
                    let (key, n) = favorites::bookmark(&layout, &mut reg, &spec, &skill)?;
                    reg.save(&layout)?;
                    println!("已收藏 {key}（{n} 个技能）");
                    Ok(())
                }
                (None, None) => {
                    if !skill.is_empty() {
                        return Err(Error::Msg(
                            "--skill 需搭配 source：skills fav <仓库> --skill <名>".into(),
                        ));
                    }
                    print_favorites(&reg);
                    Ok(())
                }
            }
        }
```

`use super::{...}` 行加 `FavSub`。

文件底部（`run_config` 之前的自由函数区）加两级列表渲染：

```rust
/// 收藏的两级列表：一级 = 技能包；多技能仓库二级逐行列技能名 + 用途；
/// 单技能仓库（is_single_skill_repo）二级留空，用途直接挂在一级行。
fn print_favorites(reg: &Registry) {
    if reg.favorites.is_empty() {
        println!("（无收藏）");
        return;
    }
    for (key, fav) in &reg.favorites {
        let date: String = fav.bookmarked_at.chars().take(10).collect();
        // chars().take(7)：按字符截断，避免多字节 UTF-8 在字节边界切片 panic
        let commit_short: String = fav.commit.chars().take(7).collect();
        let meta = if fav.url.is_some() {
            format!("({commit_short}, 收藏于 {date})")
        } else {
            "(本地源)".to_string()
        };
        if favorites::is_single_skill_repo(fav) {
            println!("{key} — {}    {meta}", fav.skills[0].description);
            continue;
        }
        println!("{key}    {meta}");
        for (i, s) in fav.skills.iter().enumerate() {
            let branch = if i + 1 == fav.skills.len() {
                "└─"
            } else {
                "├─"
            };
            println!("  {branch} {} — {}", s.name, s.description);
        }
    }
}
```

- [ ] **Step 5: 跑测试确认通过**

Run: `cargo test --test cli_smoke`
Expected: PASS（含 `fav_help_and_arg_validation`、`fav_bookmark_list_rm_local_source`；既有冒烟测试不回归——help 列表断言可能列举子命令名，确认 fav 加入后不破坏既有断言）

- [ ] **Step 6: 格式化并提交**

```bash
cargo fmt
git add src/cli/mod.rs src/cli/commands.rs tests/cli_smoke.rs
git commit -m "feat: CLI fav 子命令（收藏/两级列表/删除）

Co-Authored-By: Claude Fable 5 <noreply@anthropic.com>"
```

---

### Task 5: CLI fav install + add 共用安装循环收敛 + e2e 全链路

**Files:**
- Modify: `src/cli/commands.rs`（install 分支真实现；抽取 resolve_targets/resolve_method/install_loop 三个 add 共用助手）
- Test: `tests/e2e.rs`（追加三个端到端测试）

**Interfaces:**
- Consumes: Task 3 的 `favorites::fav_install`；Task 2 的 `resolve_key`；Task 4 的 `FavSub::Install { source, skill, target, global, method, yes }`
- Produces:
  - `commands.rs` 内私有 `fn resolve_targets(target: &[String], global: bool, cfg: &Config) -> Result<Vec<Target>>`
  - `commands.rs` 内私有 `fn resolve_method(method: Option<MethodArg>, cfg: &Config) -> Method`
  - `commands.rs` 内私有 `fn install_loop(layout, cfg, reg, picked: &[String], targets: &[Target], method: Method, yes: bool, install_fn: &(dyn Fn(&mut Registry, &str, &Target) -> Result<()>)) -> Result<()>`——「逐技能逐目标安装 + Conflict 确认/跳过 + 逐条落盘」循环，add 与 fav install 共用

- [ ] **Step 1: 写失败测试（追加到 tests/e2e.rs 末尾）**

```rust
/// 收藏全链路：整仓收藏 → 两级列表 → 单技能删/补 → 从收藏安装 → 删收藏不影响已安装副本。
#[test]
fn favorites_flow() {
    let tmp = tempfile::tempdir().unwrap();
    let home = tmp.path().join("home");
    let (bare, _work) = fixture_repo(tmp.path());
    let agents_dir = redirect_agents_target(&home);
    let url = format!("file://{}", bare.display());

    // 收藏整仓
    let out = skills(&home).args(["fav", &url]).output().unwrap();
    assert!(
        out.status.success(),
        "{}",
        String::from_utf8_lossy(&out.stderr)
    );
    let stdout = String::from_utf8_lossy(&out.stdout);
    assert!(stdout.contains("已收藏") && stdout.contains("（2 个技能）"), "{stdout}");

    // 两级列表
    let out = skills(&home).args(["fav"]).output().unwrap();
    let stdout = String::from_utf8_lossy(&out.stdout);
    assert!(stdout.contains("├─ alpha — 技能 alpha"), "{stdout}");
    assert!(stdout.contains("└─ beta — 技能 beta"), "{stdout}");

    // 从 registry 读真实 source key（file:// 的 key 推导见 source.rs，不硬编码）
    let key = {
        let reg: serde_json::Value = serde_json::from_str(
            &std::fs::read_to_string(home.join("registry.json")).unwrap(),
        )
        .unwrap();
        reg["favorites"]
            .as_object()
            .unwrap()
            .keys()
            .next()
            .unwrap()
            .clone()
    };

    // 删单个再补回（upsert）
    skills(&home)
        .args(["fav", "rm", &key, "--skill", "alpha"])
        .assert()
        .success();
    let out = skills(&home).args(["fav"]).output().unwrap();
    let stdout = String::from_utf8_lossy(&out.stdout);
    assert!(!stdout.contains("alpha") && stdout.contains("beta"), "{stdout}");
    skills(&home)
        .args(["fav", &url, "--skill", "alpha"])
        .assert()
        .success();

    // 从收藏安装：source 给 URL（验证 resolve_key 的规范化路径）
    skills(&home)
        .args([
            "fav",
            "install",
            &url,
            "--skill",
            "alpha",
            "-t",
            "global:agents",
            "--method",
            "copy",
            "-y",
        ])
        .assert()
        .success();
    assert!(agents_dir.join("alpha/SKILL.md").exists());

    // 收藏与安装是正交记录：删整包收藏，已安装副本原样保留
    skills(&home).args(["fav", "rm", &key]).assert().success();
    let out = skills(&home).args(["fav"]).output().unwrap();
    assert!(String::from_utf8_lossy(&out.stdout).contains("（无收藏）"));
    assert!(
        agents_dir.join("alpha/SKILL.md").exists(),
        "删收藏不得影响已安装副本"
    );
    let out = skills(&home).args(["list"]).output().unwrap();
    assert!(String::from_utf8_lossy(&out.stdout).contains("alpha"));
}

/// 单技能仓库：二级留空，用途挂在一级行。
#[test]
fn favorites_single_skill_repo_display() {
    let tmp = tempfile::tempdir().unwrap();
    let home = tmp.path().join("home");
    let work = tmp.path().join("solo-work");
    let bare = tmp.path().join("solo.git");
    std::fs::create_dir_all(&work).unwrap();
    std::fs::write(
        work.join("SKILL.md"),
        "---\nname: solo\ndescription: 单技能用途\n---\n",
    )
    .unwrap();
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

    skills(&home)
        .args(["fav", &format!("file://{}", bare.display())])
        .assert()
        .success();
    let out = skills(&home).args(["fav"]).output().unwrap();
    let stdout = String::from_utf8_lossy(&out.stdout);
    assert!(stdout.contains("— 单技能用途"), "{stdout}");
    assert!(
        !stdout.contains("├─") && !stdout.contains("└─"),
        "单技能仓库不得有二级行: {stdout}"
    );
}

/// 缓存被手动删除后，fav install 自愈重克隆。
#[test]
fn fav_install_heals_missing_cache() {
    let tmp = tempfile::tempdir().unwrap();
    let home = tmp.path().join("home");
    let (bare, _work) = fixture_repo(tmp.path());
    let agents_dir = redirect_agents_target(&home);
    let url = format!("file://{}", bare.display());
    skills(&home).args(["fav", &url]).assert().success();
    let key = {
        let reg: serde_json::Value = serde_json::from_str(
            &std::fs::read_to_string(home.join("registry.json")).unwrap(),
        )
        .unwrap();
        reg["favorites"]
            .as_object()
            .unwrap()
            .keys()
            .next()
            .unwrap()
            .clone()
    };
    std::fs::remove_dir_all(home.join(&key)).unwrap();
    skills(&home)
        .args([
            "fav",
            "install",
            &key,
            "--skill",
            "beta",
            "-t",
            "global:agents",
            "--method",
            "copy",
            "-y",
        ])
        .assert()
        .success();
    assert!(agents_dir.join("beta/SKILL.md").exists());
}
```

- [ ] **Step 2: 跑测试确认失败**

Run: `cargo test --test e2e favorites`
Expected: FAIL（`fav install` 分支还是"尚未实现"错误 / 编译错误）

- [ ] **Step 3: 抽取 add 共用助手（不改变 add 行为的纯重构，先抽后接）**

`src/cli/commands.rs` 文件底部加三个函数：

```rust
/// add 与 fav install 共用的目标解析：-t 列表 / -g（配置里第一个 global target）/ 裸默认（当前项目）。
fn resolve_targets(target: &[String], global: bool, cfg: &Config) -> Result<Vec<Target>> {
    if !target.is_empty() {
        return target.iter().map(|s| Target::parse(s)).collect();
    }
    if global {
        return Ok(vec![default_global_target(cfg)?]);
    }
    Ok(vec![Target::Project {
        root: std::env::current_dir()?,
    }])
}

fn resolve_method(method: Option<MethodArg>, cfg: &Config) -> Method {
    match method {
        Some(MethodArg::Copy) => Method::Copy,
        Some(MethodArg::Symlink) => Method::Symlink,
        None => cfg.default_method,
    }
}

/// 「逐技能逐目标安装 + Conflict 确认/跳过 + 逐条落盘」循环。
/// 逐条落盘的原因同 add 原注释：中途失败时 registry 与磁盘已写入的副本保持一致。
/// install_fn 返回 Ok=装成，Err(Conflict)=走确认，其余 Err 直接中断。
// 参数即一次安装批次的全部上下文（布局/配置/记录/技能集/目标集/方式/确认/动作），
// 打包成结构体只是挪位置，与 install_skill 同款保留平铺签名。
#[allow(clippy::too_many_arguments)]
fn install_loop(
    layout: &Layout,
    cfg: &Config,
    reg: &mut Registry,
    picked: &[String],
    targets: &[Target],
    method: Method,
    yes: bool,
    install_fn: &(dyn Fn(&mut Registry, &str, &Target) -> Result<()>),
) -> Result<()> {
    for s in picked {
        for t in targets {
            let installed = match install_fn(reg, s, t) {
                Ok(()) => {
                    println!("已安装 {s} → {t:?} ({method:?})");
                    true
                }
                Err(Error::Conflict(p)) => {
                    if yes {
                        println!("跳过已存在: {p:?}");
                        false
                    } else {
                        let overwrite = dialoguer::Confirm::new()
                            .with_prompt(format!("{p:?} 已存在，覆盖？"))
                            .interact()
                            .map_err(|e| Error::Msg(e.to_string()))?;
                        if overwrite {
                            let rec = install::to_rec(t);
                            let _ = remove::remove_install(layout, cfg, reg, s, &rec);
                            install_fn(reg, s, t)?;
                            true
                        } else {
                            false
                        }
                    }
                }
                Err(e) => return Err(e),
            };
            if installed {
                reg.save(layout)?;
            }
        }
    }
    Ok(())
}
```

然后把 `Cmd::Add` 分支的目标/method 解析与安装循环替换为助手调用。**有意的行为改进**：技能存在性校验从「逐技能安装到一半才发现」提前为全部校验后再开工（原实现装完前几个技能才在第 N 个上报"仓库中无技能"）：

```rust
            let targets = resolve_targets(&target, global, &cfg)?;
            let method = resolve_method(method, &cfg);
            // 全部技能名先校验再开工，避免装到一半才报"仓库中无技能"
            for s in &picked {
                if !all.iter().any(|e| &e.name == s) {
                    return Err(Error::Msg(format!("仓库中无技能 {s}")));
                }
            }
            install_loop(&layout, &cfg, &mut reg, &picked, &targets, method, yes, &|reg: &mut Registry, s: &str, t: &Target| {
                let entry = all.iter().find(|e| &e.name == s).expect("刚校验过存在");
                install::install_skill(
                    &layout,
                    &cfg,
                    reg,
                    &spec.key,
                    &entry.name,
                    &entry.rel_path.to_string_lossy(),
                    t,
                    method,
                    &cached.commit,
                )?;
                Ok(())
            })?;
            reg.save(&layout)
```

闭包参数标注了显式类型——`&dyn Fn` 强制转换下不带标注的闭包参数类型推断不可靠，标注是硬性要求（fav install 的闭包同理）。

（Add 分支原有的 `let targets: Vec<Target> = ...` 三段式与 `let method = match ...`、以及 `for s in &picked { ... for t in &targets { match install_skill ... } }` 循环整体删除，被上面代码取代。）

- [ ] **Step 4: 先验证重构零回归，再接 fav install**

Run: `cargo test`
Expected: 全绿（add 行为不变，由既有 e2e/单测锁定）

把 Task 4 的占位分支替换为真实现：

```rust
                (Some(FavSub::Install { source, skill, target, global, method, yes }), _) => {
                    let key = favorites::resolve_key(&reg, &source)?;
                    let picked: Vec<String> = if skill.is_empty() {
                        let fav = &reg.favorites[&key];
                        if fav.skills.len() == 1 {
                            vec![fav.skills[0].name.clone()]
                        } else {
                            // 从收藏的技能集里选（不重扫全仓）
                            let names: Vec<String> =
                                fav.skills.iter().map(|s| s.name.clone()).collect();
                            let idx = dialoguer::MultiSelect::new()
                                .with_prompt("选择要安装的技能")
                                .items(&names)
                                .interact()
                                .map_err(|e| Error::Msg(e.to_string()))?;
                            idx.into_iter().map(|i| names[i].clone()).collect()
                        }
                    } else {
                        skill
                    };
                    if picked.is_empty() {
                        println!("未选择技能，取消安装");
                        return Ok(());
                    }
                    let targets = resolve_targets(&target, global, &cfg)?;
                    let method = resolve_method(method, &cfg);
                    install_loop(&layout, &cfg, &mut reg, &picked, &targets, method, yes, &|reg: &mut Registry, s: &str, t: &Target| {
                        favorites::fav_install(&layout, &cfg, reg, &key, s, t, method)?;
                        Ok(())
                    })?;
                    reg.save(&layout)
                }
```

- [ ] **Step 5: 跑全部测试确认通过**

Run: `cargo test`
Expected: 全绿（含三个新 e2e）

- [ ] **Step 6: 格式化并提交**

```bash
cargo fmt
git add src/cli/commands.rs tests/e2e.rs
git commit -m "feat: CLI fav install + add/fav 共用安装循环收敛

Co-Authored-By: Claude Fable 5 <noreply@anthropic.com>"
```

---

### Task 6: TUI 收藏视图——reducer + 渲染

**Files:**
- Modify: `src/tui/app.rs`（View::Favorites、FavRow、fav_rows、Action::DeleteFav、按视图计数导航）
- Modify: `src/tui/ui.rs`（第四页签 + 收藏表格渲染 + 冒烟测试更新）

**Interfaces:**
- Consumes: `favorites::is_single_skill_repo`、`favorites::unbookmark`、`registry::{Favorite, FavSkill}`
- Produces（Task 7 依赖）:
  - `app::View::Favorites`、`app::FavRow::{Source(String), Skill(String, usize)}`
  - `AppState::fav_rows(&self) -> Vec<FavRow>`——收藏展平成行：多技能仓库 = 标题行 + 每技能一行；单技能仓库 = 仅标题行
  - `Action::DeleteFav`——reducer 内直接删收藏（无需交互）

- [ ] **Step 1: 写失败测试（替换/追加 src/tui/app.rs 的 tests 模块）**

```rust
    use crate::core::registry::{FavSkill, Favorite};

    fn app_with_favorites() -> AppState {
        let mut reg = Registry {
            version: 1,
            ..Default::default()
        };
        reg.favorites.insert(
            "github/o/r".into(),
            Favorite {
                url: Some("https://github.com/o/r".into()),
                local_path: None,
                commit: "deadbeef".into(),
                bookmarked_at: "2026-08-25T10:00:00Z".into(),
                skills: vec![
                    FavSkill {
                        name: "a".into(),
                        description: "A".into(),
                        source_path: "skills/a".into(),
                    },
                    FavSkill {
                        name: "b".into(),
                        description: "B".into(),
                        source_path: "skills/b".into(),
                    },
                ],
            },
        );
        reg.favorites.insert(
            "local/solo".into(),
            Favorite {
                url: None,
                local_path: Some("/x/solo".into()),
                commit: String::new(),
                bookmarked_at: "2026-08-25T10:00:00Z".into(),
                skills: vec![FavSkill {
                    name: "solo".into(),
                    description: "单技能".into(),
                    source_path: ".".into(),
                }],
            },
        );
        AppState::new(reg)
    }

    #[test]
    fn tab_cycles_four_views() {
        let mut app = app_with(1);
        assert_eq!(app.view, View::Installed);
        app.reduce(Action::NextView); // Install
        app.reduce(Action::NextView); // Sources
        app.reduce(Action::NextView); // Favorites
        assert_eq!(app.view, View::Favorites);
        app.reduce(Action::NextView);
        assert_eq!(app.view, View::Installed);
        app.reduce(Action::PrevView); // 反向回 Favorites
        assert_eq!(app.view, View::Favorites);
    }

    #[test]
    fn favorites_rows_flatten_two_levels() {
        let app = app_with_favorites();
        let rows = app.fav_rows();
        // BTreeMap 排序：github/o/r 在前（1 标题 + 2 技能），local/solo 单技能仓库只 1 行
        assert_eq!(
            rows,
            vec![
                FavRow::Source("github/o/r".into()),
                FavRow::Skill("github/o/r".into(), 0),
                FavRow::Skill("github/o/r".into(), 1),
                FavRow::Source("local/solo".into()),
            ]
        );
    }

    #[test]
    fn favorites_navigation_clamps_per_view() {
        let mut app = app_with_favorites();
        app.view = View::Favorites;
        for _ in 0..10 {
            app.reduce(Action::Down);
        }
        assert_eq!(app.selected, 3);
        app.reduce(Action::Up);
        assert_eq!(app.selected, 2);
        // 切视图清零选中
        app.reduce(Action::NextView);
        assert_eq!(app.selected, 0);
    }

    #[test]
    fn delete_fav_skill_row_then_source_row() {
        let mut app = app_with_favorites();
        app.view = View::Favorites;
        app.selected = 1; // Skill(github/o/r, 0)
        app.reduce(Action::DeleteFav);
        assert_eq!(app.registry.favorites["github/o/r"].skills.len(), 1);
        assert_eq!(app.registry.favorites["github/o/r"].skills[0].name, "b");
        // 删除后行数收缩，selected 被 clamp 到合法范围
        assert!(app.selected < app.fav_rows().len());
        // 标题行：删整包
        app.selected = 0;
        app.reduce(Action::DeleteFav);
        assert!(!app.registry.favorites.contains_key("github/o/r"));
        assert!(app.registry.favorites.contains_key("local/solo"));
        // 非收藏视图不误伤
        app.view = View::Installed;
        app.reduce(Action::DeleteFav);
        assert!(app.registry.favorites.contains_key("local/solo"));
    }
```

注意：既有 `tab_switches_view` 测试断言三环切换，会被四环取代——删除该旧测试（被 `tab_cycles_four_views` 覆盖）。

- [ ] **Step 2: 跑测试确认失败**

Run: `cargo test tui::app`
Expected: FAIL（编译错误：FavRow/fav_rows/DeleteFav/Favorites 不存在）

- [ ] **Step 3: 实现 app.rs**

顶部 use 区补 `use crate::core::favorites;`。

```rust
#[derive(PartialEq, Debug, Clone, Copy)]
pub enum View {
    Installed,
    Install,
    Sources,
    Favorites,
}

#[derive(PartialEq, Debug)]
pub enum Action {
    Up,
    Down,
    NextView,
    PrevView,
    ToggleAutoUpdate,
    DeleteFav,
    Quit,
}

/// 收藏视图的扁平行：source 标题行或其中的技能行。
#[derive(PartialEq, Debug)]
pub enum FavRow {
    Source(String),       // source key
    Skill(String, usize), // source key + skills 下标
}
```

`AppState` 加方法：

```rust
    /// 收藏视图展平行：多技能仓库 = 标题行 + 每技能一行；
    /// 单技能仓库只出标题行（二级留空，用途挂在一级）。
    pub fn fav_rows(&self) -> Vec<FavRow> {
        let mut rows = Vec::new();
        for (key, fav) in &self.registry.favorites {
            rows.push(FavRow::Source(key.clone()));
            if !favorites::is_single_skill_repo(fav) {
                for i in 0..fav.skills.len() {
                    rows.push(FavRow::Skill(key.clone(), i));
                }
            }
        }
        rows
    }
```

`reduce` 的 Up/Down 改为按视图计数（此前任何视图都用 installs 行数，是有意修正的边界 quirk；Sources/Install 视图无可选行）：

```rust
            Action::Up => self.selected = self.selected.saturating_sub(1),
            Action::Down => {
                let rows = match self.view {
                    View::Installed => self.visible_rows().len(),
                    View::Favorites => self.fav_rows().len(),
                    View::Install | View::Sources => 0,
                };
                let max = rows.saturating_sub(1);
                self.selected = (self.selected + 1).min(max);
            }
```

NextView/PrevView 四环：

```rust
            Action::NextView => {
                self.view = match self.view {
                    View::Installed => View::Install,
                    View::Install => View::Sources,
                    View::Sources => View::Favorites,
                    View::Favorites => View::Installed,
                };
                self.selected = 0;
            }
            Action::PrevView => {
                self.view = match self.view {
                    View::Installed => View::Favorites,
                    View::Favorites => View::Sources,
                    View::Sources => View::Install,
                    View::Install => View::Installed,
                };
                self.selected = 0;
            }
```

DeleteFav 分支（加在 ToggleAutoUpdate 之后）：

```rust
            Action::DeleteFav => {
                if self.view != View::Favorites {
                    return;
                }
                let rows = self.fav_rows();
                if let Some(row) = rows.get(self.selected) {
                    match row {
                        FavRow::Source(key) => {
                            let _ = favorites::unbookmark(&mut self.registry, key, &[]);
                        }
                        FavRow::Skill(key, i) => {
                            let name =
                                self.registry.favorites[key].skills[*i].name.clone();
                            let _ = favorites::unbookmark(&mut self.registry, key, &[name]);
                        }
                    }
                    // 行数已收缩：selected clamp 到合法范围
                    let max = self.fav_rows().len().saturating_sub(1);
                    self.selected = self.selected.min(max);
                }
            }
```

- [ ] **Step 4: 实现 ui.rs 渲染**

页签行改四环：

```rust
    let titles = ["已安装", "安装向导", "仓库缓存", "收藏"];
    let idx = match app.view {
        View::Installed => 0,
        View::Install => 1,
        View::Sources => 2,
        View::Favorites => 3,
    };
```

match 加收藏视图分支（放 View::Sources 之后）：

```rust
        View::Favorites => {
            let rows = app.fav_rows();
            let table_rows = rows.iter().enumerate().map(|(i, row)| {
                let style = if i == app.selected {
                    Style::default().fg(Color::Yellow)
                } else {
                    Style::default()
                };
                match row {
                    FavRow::Source(key) => {
                        let fav = &app.registry.favorites[key];
                        let date: String = fav.bookmarked_at.chars().take(10).collect();
                        if crate::core::favorites::is_single_skill_repo(fav) {
                            // 单技能仓库：二级留空，用途挂一级行
                            Row::new(vec![
                                Cell::from(key.clone()),
                                Cell::from(fav.skills[0].description.clone()),
                                Cell::from(date),
                            ])
                        } else {
                            Row::new(vec![
                                Cell::from(key.clone()),
                                Cell::from(format!("{} 个技能", fav.skills.len())),
                                Cell::from(date),
                            ])
                        }
                    }
                    FavRow::Skill(key, idx) => {
                        let s = &app.registry.favorites[key].skills[*idx];
                        Row::new(vec![
                            Cell::from(format!("  {}", s.name)),
                            Cell::from(s.description.clone()),
                            Cell::from(String::new()),
                        ])
                    }
                }
                .style(style)
            });
            f.render_widget(
                Table::new(
                    table_rows,
                    [
                        Constraint::Percentage(35),
                        Constraint::Percentage(50),
                        Constraint::Percentage(15),
                    ],
                )
                .header(
                    Row::new(vec!["收藏", "用途", "收藏时间"])
                        .style(Style::default().fg(Color::Cyan)),
                )
                .block(
                    Block::default()
                        .borders(Borders::ALL)
                        .title("f=收藏 d=删除 i=安装"),
                ),
                chunks[1],
            );
        }
```

ui.rs 的 use 区补 `use super::app::FavRow;`。冒烟测试 `draw_all_views_smoke` 的视图数组改为 `[View::Installed, View::Install, View::Sources, View::Favorites]`；fixture 的 reg 可加一条 favorite（也可不加——空收藏渲染同样要不 panic，建议两种都不断言、只渲染）。

- [ ] **Step 5: 跑测试确认通过**

Run: `cargo test tui`
Expected: PASS（含四环切换、扁平行、clamp、删除分发、四视图渲染冒烟）

- [ ] **Step 6: 格式化并提交**

```bash
cargo fmt
git add src/tui/app.rs src/tui/ui.rs
git commit -m "feat: TUI 收藏视图（四环页签 + 两级列表 + 删除）

Co-Authored-By: Claude Fable 5 <noreply@anthropic.com>"
```

---

### Task 7: TUI 收藏/安装向导（mod.rs 交互接线）

**Files:**
- Modify: `src/tui/mod.rs`（按键分发、fav_wizard、fav_install_wizard；从 install_wizard 抽取 pick_target）

**Interfaces:**
- Consumes: Task 6 的 `View::Favorites`/`FavRow`/`fav_rows`；Task 2/3 的 `bookmark`/`fav_install`；`cache::{ensure_cached, scan_skills}`、`source::parse_source`
- Produces:
  - `tui/mod.rs` 内私有 `fn pick_target(cfg: &Config) -> Result<Target>`——install_wizard 与 fav_install_wizard 共用的目标选择（global targets + 当前项目）
  - `fn fav_wizard(layout: &Layout, app: &mut AppState) -> Result<()>`
  - `fn fav_install_wizard(layout: &Layout, cfg: &Config, app: &mut AppState) -> Result<()>`

- [ ] **Step 1: 写按键分发的回归测试（追加到 src/tui/mod.rs 的 tests 模块）**

交互向导本身无法单测（与 install_wizard 一致，走手动验证），但 DeleteFav 的按键分发语义已在 Task 6 由 reducer 测试锁定。本任务只加一个守护测试：install_wizard 抽出 pick_target 后行为不变（编译期 + 既有测试保证），以及收藏向导落盘路径不丢内存 auto_update 切换——既有 `wizard_save_path_preserves_in_memory_auto_update_toggle` 已锁定该语义，fav_wizard 沿用同一模式（直接操作 app.registry 并 save），无需新测试。

- [ ] **Step 2: 抽取 pick_target 并改造 install_wizard**

install_wizard 中这段目标选择代码：

```rust
    // 目标候选：配置的 global targets + 当前项目
    let mut targets: Vec<(String, Target)> = cfg
        .targets
        .keys()
        .map(|n| (format!("global:{n}"), Target::Global { name: n.clone() }))
        .collect();
    targets.push((
        format!("project:{}", std::env::current_dir()?.display()),
        Target::Project {
            root: std::env::current_dir()?,
        },
    ));
    let labels: Vec<&str> = targets.iter().map(|(l, _)| l.as_str()).collect();
    let ti = dialoguer::Select::new()
        .with_prompt("安装到目标")
        .items(&labels)
        .default(0)
        .interact()
        .map_err(|e| Error::Msg(e.to_string()))?;
    let target = targets[ti].1.clone();
```

整体替换为 `let target = pick_target(cfg)?;`，并在文件中新增自由函数：

```rust
/// 安装向导共用的目标选择：配置的 global targets + 当前项目（project:<cwd>）。
fn pick_target(cfg: &Config) -> Result<Target> {
    let mut targets: Vec<(String, Target)> = cfg
        .targets
        .keys()
        .map(|n| (format!("global:{n}"), Target::Global { name: n.clone() }))
        .collect();
    targets.push((
        format!("project:{}", std::env::current_dir()?.display()),
        Target::Project {
            root: std::env::current_dir()?,
        },
    ));
    let labels: Vec<&str> = targets.iter().map(|(l, _)| l.as_str()).collect();
    let ti = dialoguer::Select::new()
        .with_prompt("安装到目标")
        .items(&labels)
        .default(0)
        .interact()
        .map_err(|e| Error::Msg(e.to_string()))?;
    Ok(targets[ti].1.clone())
}
```

- [ ] **Step 3: 实现两个收藏向导**

`src/tui/mod.rs` 的 use 区补 `favorites`。install_wizard 之后追加：

```rust
/// 收藏向导（已在正常终端模式下运行）：输 source → 扫描 → 多选（默认全选）→ 走 core 收藏。
/// 直接操作并落盘内存 registry（与 install_wizard 同因：TUI 内未落盘的 auto_update 切换不得丢失）。
fn fav_wizard(layout: &Layout, app: &mut AppState) -> Result<()> {
    let source: String = dialoguer::Input::new()
        .with_prompt("source（github:owner/repo、URL 或本地路径）")
        .interact_text()
        .map_err(|e| Error::Msg(e.to_string()))?;
    let spec = parse_source(&source)?;
    let cached = cache::ensure_cached(layout, &spec)?;
    if !cached.fresh {
        println!("已缓存 {}，复用", spec.key);
    }
    let all = cache::scan_skills(&cached.path)?;
    let names: Vec<String> = all.iter().map(|s| s.name.clone()).collect();
    let defaults = vec![true; names.len()];
    let idx = dialoguer::MultiSelect::new()
        .with_prompt("选择要收藏的技能")
        .items(&names)
        .defaults(&defaults)
        .interact()
        .map_err(|e| Error::Msg(e.to_string()))?;
    if idx.is_empty() {
        println!("未选择技能，取消收藏");
        return Ok(());
    }
    // 全选 = 整仓收藏（传空切片走全量覆盖语义，快照随仓库收缩也能刷新干净）；
    // 部分选 = 只收藏勾选项（upsert）
    let picked: Vec<String> = if idx.len() == names.len() {
        Vec::new()
    } else {
        idx.into_iter().map(|i| names[i].clone()).collect()
    };
    let (key, n) = favorites::bookmark(layout, &mut app.registry, &spec, &picked)?;
    let n = if picked.is_empty() { all.len() } else { n };
    app.registry.save(layout)?;
    println!("已收藏 {key}（{n} 个技能）");
    Ok(())
}

/// 从收藏安装（已在正常终端模式下运行）：当前行定技能集 → 选目标 → 走 core 安装。
fn fav_install_wizard(layout: &Layout, cfg: &Config, app: &mut AppState) -> Result<()> {
    let rows = app.fav_rows();
    let Some(row) = rows.get(app.selected) else {
        println!("（无收藏）");
        return Ok(());
    };
    let (key, picked) = match row {
        FavRow::Skill(key, i) => (
            key.clone(),
            vec![app.registry.favorites[key].skills[*i].name.clone()],
        ),
        FavRow::Source(key) => {
            let fav = &app.registry.favorites[key];
            if favorites::is_single_skill_repo(fav) {
                (key.clone(), vec![fav.skills[0].name.clone()])
            } else {
                let names: Vec<String> = fav.skills.iter().map(|s| s.name.clone()).collect();
                let idx = dialoguer::MultiSelect::new()
                    .with_prompt("选择要安装的技能")
                    .items(&names)
                    .interact()
                    .map_err(|e| Error::Msg(e.to_string()))?;
                if idx.is_empty() {
                    println!("未选择技能，取消安装");
                    return Ok(());
                }
                (key.clone(), idx.into_iter().map(|i| names[i].clone()).collect())
            }
        }
    };
    let target = pick_target(cfg)?;
    let method = cfg.default_method;
    for s in &picked {
        match favorites::fav_install(layout, cfg, &mut app.registry, &key, s, &target, method) {
            Ok(_) => println!("已安装 {s} → {target:?} ({method:?})"),
            Err(Error::Conflict(p)) => {
                let overwrite = dialoguer::Confirm::new()
                    .with_prompt(format!("{p:?} 已存在，覆盖？"))
                    .interact()
                    .map_err(|e| Error::Msg(e.to_string()))?;
                if overwrite {
                    let rec = install::to_rec(&target);
                    let _ = remove::remove_install(layout, cfg, &mut app.registry, s, &rec);
                    favorites::fav_install(
                        layout,
                        cfg,
                        &mut app.registry,
                        &key,
                        s,
                        &target,
                        method,
                    )?;
                    println!("已覆盖安装 {s} → {target:?} ({method:?})");
                } else {
                    println!("跳过 {s}");
                }
            }
            Err(e) => return Err(e),
        }
    }
    app.registry.save(layout)?;
    Ok(())
}
```

- [ ] **Step 4: 接按键分发（event_loop）**

现有 `if k.code == KeyCode::Char('i') { ... install_wizard ... }` 整段替换为按视图分发，并新增 `f` 键：

```rust
        if let Event::Key(k) = event::read()? {
            if k.code == KeyCode::Char('i') {
                // i：Installed 等视图=从仓库装；Favorites=从收藏装。向导需正常终端（dialoguer）。
                guard.suspend();
                let r = if app.view == View::Favorites {
                    fav_install_wizard(layout, cfg, app)
                } else {
                    install_wizard(layout, cfg, app)
                };
                if let Err(e) = r {
                    eprintln!("安装失败: {e}");
                    let _ = dialoguer::Input::<String>::new()
                        .with_prompt("按回车返回 TUI")
                        .allow_empty(true)
                        .interact_text();
                }
                guard.resume()?;
                term.clear()?;
                continue;
            }
            if k.code == KeyCode::Char('f') && app.view == View::Favorites {
                guard.suspend();
                let r = fav_wizard(layout, app);
                if let Err(e) = r {
                    eprintln!("收藏失败: {e}");
                    let _ = dialoguer::Input::<String>::new()
                        .with_prompt("按回车返回 TUI")
                        .allow_empty(true)
                        .interact_text();
                }
                guard.resume()?;
                term.clear()?;
                continue;
            }
            let action = match k.code {
                KeyCode::Char('q') | KeyCode::Esc => Action::Quit,
                KeyCode::Up | KeyCode::Char('k') => Action::Up,
                KeyCode::Down | KeyCode::Char('j') => Action::Down,
                KeyCode::Tab => Action::NextView,
                KeyCode::BackTab => Action::PrevView,
                KeyCode::Char('a') => Action::ToggleAutoUpdate,
                KeyCode::Char('d') => Action::DeleteFav,
                _ => continue,
            };
```

（其余不变。）

- [ ] **Step 5: 跑测试 + 手动验证**

Run: `cargo test`
Expected: 全绿（重构不改行为；向导交互无单测，与 install_wizard 同标准）

手动验证（实现者本地跑一遍并核对）：

```bash
cargo run -- tui
# 1. Tab 切到「收藏」页签
# 2. 按 f，输入一个本地技能目录路径（如含 skills/alpha/SKILL.md 的目录），全选确认 → 列表出现两级行
# 3. 选中技能行按 i，选目标 → 安装成功提示
# 4. 选中标题行按 d → 整包收藏消失；q 退出后重进，收藏状态与退出前一致
```

- [ ] **Step 6: 格式化并提交**

```bash
cargo fmt
git add src/tui/mod.rs
git commit -m "feat: TUI 收藏/安装向导（f/i/d 按键接线）

Co-Authored-By: Claude Fable 5 <noreply@anthropic.com>"
```

---

### Task 8: Web API——收藏 4 端点 + targets 1 端点

**Files:**
- Modify: `src/web/api.rs`（路由 + 5 个 handler + tower 测试）

**Interfaces:**
- Consumes: Task 2/3 的 `bookmark`/`unbookmark`/`resolve_key`/`fav_install`；Task 1 的 `TargetRec::to_target`；`config::Config`、`remove::remove_install`
- Produces（Task 9 前端依赖的契约）:
  - `GET /api/favorites` → `200 {source_key: Favorite}`（favorites map 原样序列化）
  - `POST /api/favorites`，body `{source: String, skill: [String]}`（空数组=整仓）→ `200 {key, skills: 数量}`；source 无法解析或技能不存在 → `400`；clone/IO 失败 → `500`
  - `POST /api/favorites/remove`，body `{source: String, skill: [String]}` → `200`；未收藏 → `404`
  - `POST /api/favorites/install`，body `{source, skill: String, target: TargetRec, method?: "symlink"|"copy", overwrite?: bool}` → `200 {installed}`；冲突 → `409` + 文本明细（前端 confirm 后带 `overwrite:true` 重试）；未收藏 → `404`；其余 → `500`
  - `GET /api/targets` → `200 [{name, path}]`（config 的 global targets）；config 损坏 → `500`（与 run_update 相同的显式报错纪律，不回退默认配置）

- [ ] **Step 1: 写失败测试（追加到 src/web/api.rs 的 tests 模块）**

```rust
    /// 在 state.tmp 里造一个本地双技能源，返回其绝对路径。
    fn make_local_source(tmp: &tempfile::TempDir) -> String {
        let src = tmp.path().join("mysrc");
        std::fs::create_dir_all(src.join("skills/alpha")).unwrap();
        std::fs::create_dir_all(src.join("skills/beta")).unwrap();
        std::fs::write(
            src.join("skills/alpha/SKILL.md"),
            "---\nname: alpha\ndescription: A 技能\n---\n",
        )
        .unwrap();
        std::fs::write(
            src.join("skills/beta/SKILL.md"),
            "---\nname: beta\ndescription: B 技能\n---\n",
        )
        .unwrap();
        src.to_string_lossy().into_owned()
    }

    fn post_json(uri: &str, body: serde_json::Value) -> axum::http::Request<axum::body::Body> {
        axum::http::Request::builder()
            .method("POST")
            .uri(uri)
            .header("content-type", "application/json")
            .body(axum::body::Body::from(serde_json::to_string(&body).unwrap()))
            .unwrap()
    }

    #[tokio::test]
    async fn favorites_api_lifecycle() {
        let state = test_state();
        let src = make_local_source(&state.tmp);
        let keep = state.tmp.clone();
        let app = router(state);
        // 收藏整仓
        let resp = app
            .clone()
            .oneshot(post_json("/api/favorites", serde_json::json!({"source": src, "skill": []})))
            .await
            .unwrap();
        assert_eq!(resp.status(), 200);
        let body = axum::body::to_bytes(resp.into_body(), usize::MAX)
            .await
            .unwrap();
        let v: serde_json::Value = serde_json::from_slice(&body).unwrap();
        assert_eq!(v["key"], "local/mysrc");
        assert_eq!(v["skills"], 2);
        // 列表
        let resp = app
            .clone()
            .oneshot(
                axum::http::Request::builder()
                    .uri("/api/favorites")
                    .body(axum::body::Body::empty())
                    .unwrap(),
            )
            .await
            .unwrap();
        let body = axum::body::to_bytes(resp.into_body(), usize::MAX)
            .await
            .unwrap();
        let v: serde_json::Value = serde_json::from_slice(&body).unwrap();
        assert_eq!(v["local/mysrc"]["skills"][0]["description"], "A 技能");
        // 删单个再删整包
        let resp = app
            .clone()
            .oneshot(post_json(
                "/api/favorites/remove",
                serde_json::json!({"source": "local/mysrc", "skill": ["alpha"]}),
            ))
            .await
            .unwrap();
        assert_eq!(resp.status(), 200);
        let resp = app
            .clone()
            .oneshot(post_json(
                "/api/favorites/remove",
                serde_json::json!({"source": "local/mysrc", "skill": []}),
            ))
            .await
            .unwrap();
        assert_eq!(resp.status(), 200);
        // 再删：404
        let resp = app
            .oneshot(post_json(
                "/api/favorites/remove",
                serde_json::json!({"source": "local/mysrc", "skill": []}),
            ))
            .await
            .unwrap();
        assert_eq!(resp.status(), 404);
        drop(keep);
    }

    #[tokio::test]
    async fn add_favorite_rejects_bad_source_and_unknown_skill() {
        let state = test_state();
        let src = make_local_source(&state.tmp);
        let keep = state.tmp.clone();
        let app = router(state);
        // 无法解析的 source：400
        let resp = app
            .clone()
            .oneshot(post_json("/api/favorites", serde_json::json!({"source": "noslash", "skill": []})))
            .await
            .unwrap();
        assert_eq!(resp.status(), 400);
        // 仓内不存在的技能：400
        let resp = app
            .oneshot(post_json(
                "/api/favorites",
                serde_json::json!({"source": src, "skill": ["nope"]}),
            ))
            .await
            .unwrap();
        assert_eq!(resp.status(), 400);
        drop(keep);
    }

    #[tokio::test]
    async fn install_favorite_conflict_then_overwrite() {
        let state = test_state();
        // agents target 指到临时目录：绝不能落盘到真实 home
        std::fs::create_dir_all(&state.layout.root).unwrap();
        std::fs::write(
            state.layout.config_path(),
            format!("[targets]\nagents = {:?}\n", state.tmp.path().join("agents")),
        )
        .unwrap();
        let src = make_local_source(&state.tmp);
        let agents = state.tmp.path().join("agents");
        let keep = state.tmp.clone();
        let app = router(state);
        // 先收藏
        let resp = app
            .clone()
            .oneshot(post_json("/api/favorites", serde_json::json!({"source": src, "skill": []})))
            .await
            .unwrap();
        assert_eq!(resp.status(), 200);
        let install = |overwrite: bool| {
            post_json(
                "/api/favorites/install",
                serde_json::json!({
                    "source": "local/mysrc",
                    "skill": "alpha",
                    "target": {"kind": "global", "name": "agents"},
                    "method": "copy",
                    "overwrite": overwrite
                }),
            )
        };
        // 首次安装 200
        let resp = app.clone().oneshot(install(false)).await.unwrap();
        assert_eq!(resp.status(), 200);
        assert!(agents.join("alpha/SKILL.md").exists());
        // 冲突 409
        let resp = app.clone().oneshot(install(false)).await.unwrap();
        assert_eq!(resp.status(), 409);
        // overwrite 重试 200
        let resp = app.clone().oneshot(install(true)).await.unwrap();
        assert_eq!(resp.status(), 200);
        // 未收藏的技能 404
        let resp = app
            .oneshot(post_json(
                "/api/favorites/install",
                serde_json::json!({
                    "source": "local/mysrc",
                    "skill": "nope",
                    "target": {"kind": "global", "name": "agents"}
                }),
            ))
            .await
            .unwrap();
        assert_eq!(resp.status(), 404);
        drop(keep);
    }

    #[tokio::test]
    async fn targets_endpoint_lists_configured() {
        let app = router(test_state());
        let resp = app
            .oneshot(
                axum::http::Request::builder()
                    .uri("/api/targets")
                    .body(axum::body::Body::empty())
                    .unwrap(),
            )
            .await
            .unwrap();
        assert_eq!(resp.status(), 200);
        let body = axum::body::to_bytes(resp.into_body(), usize::MAX)
            .await
            .unwrap();
        let v: serde_json::Value = serde_json::from_slice(&body).unwrap();
        let names: Vec<&str> = v.as_array().unwrap().iter().filter_map(|t| t["name"].as_str()).collect();
        assert!(names.contains(&"agents"), "{names:?}");
    }
```

- [ ] **Step 2: 跑测试确认失败**

Run: `cargo test web::api`
Expected: FAIL（路由不存在 → 405/404 而非预期状态码）

- [ ] **Step 3: 实现 5 个 handler**

`src/web/api.rs`：use 区补 `use crate::core::registry::Method;`。router() 加路由：

```rust
        .route("/api/favorites", get(list_favorites).post(add_favorite))
        .route("/api/favorites/remove", post(remove_favorite))
        .route("/api/favorites/install", post(install_favorite))
        .route("/api/targets", get(list_targets))
```

handler 实现（追加在 run_update 之后）：

```rust
async fn list_favorites(State(s): S) -> Result<Json<serde_json::Value>, (StatusCode, String)> {
    let s = s.lock().unwrap();
    let reg = Registry::load(&s.layout)
        .map_err(|e| (StatusCode::INTERNAL_SERVER_ERROR, format!("加载 registry 失败: {e}")))?;
    Ok(Json(serde_json::json!(reg.favorites)))
}

#[derive(serde::Deserialize)]
struct FavAddReq {
    source: String,
    #[serde(default)]
    skill: Vec<String>,
}

async fn add_favorite(
    State(s): S,
    Json(req): Json<FavAddReq>,
) -> Result<Json<serde_json::Value>, (StatusCode, String)> {
    let s = s.lock().unwrap();
    let spec = crate::core::source::parse_source(&req.source)
        .map_err(|e| (StatusCode::BAD_REQUEST, format!("{e}")))?;
    let mut reg = Registry::load(&s.layout)
        .map_err(|e| (StatusCode::INTERNAL_SERVER_ERROR, format!("加载 registry 失败: {e}")))?;
    // 用户输入类错误（无法解析/仓内无此技能）→ 400；clone/IO 类 → 500
    let (key, n) = crate::core::favorites::bookmark(&s.layout, &mut reg, &spec, &req.skill)
        .map_err(|e| {
            let bad_input = matches!(
                e,
                crate::core::error::Error::Msg(_) | crate::core::error::Error::BadTarget(_)
            );
            let code = if bad_input {
                StatusCode::BAD_REQUEST
            } else {
                StatusCode::INTERNAL_SERVER_ERROR
            };
            (code, format!("收藏失败: {e}"))
        })?;
    reg.save(&s.layout)
        .map_err(|e| (StatusCode::INTERNAL_SERVER_ERROR, format!("保存 registry 失败: {e}")))?;
    Ok(Json(serde_json::json!({ "key": key, "skills": n })))
}

#[derive(serde::Deserialize)]
struct FavRemoveReq {
    source: String,
    #[serde(default)]
    skill: Vec<String>,
}

async fn remove_favorite(
    State(s): S,
    Json(req): Json<FavRemoveReq>,
) -> Result<StatusCode, (StatusCode, String)> {
    let s = s.lock().unwrap();
    let mut reg = Registry::load(&s.layout)
        .map_err(|e| (StatusCode::INTERNAL_SERVER_ERROR, format!("加载 registry 失败: {e}")))?;
    let key = crate::core::favorites::resolve_key(&reg, &req.source)
        .map_err(|e| (StatusCode::NOT_FOUND, format!("{e}")))?;
    crate::core::favorites::unbookmark(&mut reg, &key, &req.skill)
        .map_err(|e| (StatusCode::NOT_FOUND, format!("{e}")))?;
    reg.save(&s.layout)
        .map_err(|e| (StatusCode::INTERNAL_SERVER_ERROR, format!("保存 registry 失败: {e}")))?;
    Ok(StatusCode::OK)
}

#[derive(serde::Deserialize)]
struct FavInstallReq {
    source: String,
    skill: String,
    target: TargetRec,
    method: Option<Method>,
    overwrite: Option<bool>,
}

/// 从收藏安装。冲突 → 409 + 明细，前端 confirm 后带 overwrite=true 重试（同 run_update 的确认链）。
async fn install_favorite(
    State(s): S,
    Json(req): Json<FavInstallReq>,
) -> Result<Json<serde_json::Value>, (StatusCode, String)> {
    let s = s.lock().unwrap();
    // config 损坏必须显式 500（同 run_update：静默回退默认会把内置 target 解析到真实 home 并落盘）
    let cfg = crate::core::config::Config::load(&s.layout)
        .map_err(|e| (StatusCode::INTERNAL_SERVER_ERROR, format!("加载 config 失败: {e}")))?;
    let mut reg = Registry::load(&s.layout)
        .map_err(|e| (StatusCode::INTERNAL_SERVER_ERROR, format!("加载 registry 失败: {e}")))?;
    let key = crate::core::favorites::resolve_key(&reg, &req.source)
        .map_err(|e| (StatusCode::NOT_FOUND, format!("{e}")))?;
    let target = req.target.to_target();
    let method = req.method.unwrap_or(cfg.default_method);
    let do_install = |reg: &mut Registry| {
        crate::core::favorites::fav_install(
            &s.layout, &cfg, reg, &key, &req.skill, &target, method,
        )
    };
    match do_install(&mut reg) {
        Ok(_) => {
            reg.save(&s.layout)
                .map_err(|e| (StatusCode::INTERNAL_SERVER_ERROR, format!("保存 registry 失败: {e}")))?;
            Ok(Json(serde_json::json!({ "installed": req.skill })))
        }
        Err(crate::core::error::Error::Conflict(p)) => {
            if !req.overwrite.unwrap_or(false) {
                return Err((StatusCode::CONFLICT, format!("{p:?} 已存在")));
            }
            // 与 CLI 的覆盖路径一致：先按记录删（无记录则忽略），再重装
            let _ = crate::core::remove::remove_install(&s.layout, &cfg, &mut reg, &req.skill, &req.target);
            do_install(&mut reg).map_err(|e| (StatusCode::INTERNAL_SERVER_ERROR, format!("覆盖安装失败: {e}")))?;
            reg.save(&s.layout)
                .map_err(|e| (StatusCode::INTERNAL_SERVER_ERROR, format!("保存 registry 失败: {e}")))?;
            Ok(Json(serde_json::json!({ "installed": req.skill })))
        }
        Err(crate::core::error::Error::NotBookmarked(e)) => {
            Err((StatusCode::NOT_FOUND, e))
        }
        Err(e) => Err((StatusCode::INTERNAL_SERVER_ERROR, format!("安装失败: {e}"))),
    }
}

async fn list_targets(State(s): S) -> Result<Json<serde_json::Value>, (StatusCode, String)> {
    let s = s.lock().unwrap();
    let cfg = crate::core::config::Config::load(&s.layout)
        .map_err(|e| (StatusCode::INTERNAL_SERVER_ERROR, format!("加载 config 失败: {e}")))?;
    let v: Vec<serde_json::Value> = cfg
        .targets
        .iter()
        .map(|(n, p)| serde_json::json!({ "name": n, "path": p }))
        .collect();
    Ok(Json(serde_json::json!(v)))
}
```

- [ ] **Step 4: 跑测试确认通过**

Run: `cargo test web::api`
Expected: PASS（4 个新测试 + 既有 6 个全绿）

- [ ] **Step 5: 格式化并提交**

```bash
cargo fmt
git add src/web/api.rs
git commit -m "feat: Web 收藏 API（favorites 增删列装 + targets）

Co-Authored-By: Claude Fable 5 <noreply@anthropic.com>"
```

---

### Task 9: Web 前端收藏页签 + CLAUDE.md 更新 + 全量验证

**Files:**
- Modify: `src/web/static/index.html`（nav 按钮、收藏区块、JS 渲染与交互）
- Modify: `CLAUDE.md`（常用命令一节的子命令列表加 fav）

**Interfaces:**
- Consumes: Task 8 的 5 个端点契约（body/状态码/字段名逐项对齐）
- Produces: 无新接口（纯前端 + 文档收尾）

- [ ] **Step 1: 改 index.html 的结构部分**

nav 区加按钮（放在「仓库缓存」之后）：

```html
  <button id="nav-favorites">收藏</button>
```

`<div id="sources" ...>` 之后加收藏区块：

```html
<div id="favorites" style="display:none">
  <p>
    <input id="fav-source" placeholder="仓库地址（owner/repo、URL 或本地路径）" size="42">
    <input id="fav-skill" placeholder="技能名（可空 = 整仓）" size="18">
    <button id="fav-add">收藏</button>
  </p>
  <div id="fav-list"></div>
</div>
```

- [ ] **Step 2: 改 JS——show/refresh/nav 接线**

`show()` 的 div 列表加 favorites：

```js
function show(id) {
  for (const d of ['installs','sources','favorites']) document.getElementById(d).style.display = d===id?'':'none';
  refresh();
}
```

`refresh()` 加收藏拉取：

```js
async function refresh() {
  const installs = await api('installs');
  document.getElementById('installs').replaceChildren(renderInstalls(installs));
  const sources = await api('sources');
  document.getElementById('sources').replaceChildren(renderSources(sources));
  const favs = await api('favorites');
  document.getElementById('fav-list').replaceChildren(renderFavorites(favs));
}
```

文件底部事件绑定加两行：

```js
document.getElementById('nav-favorites').addEventListener('click', () => show('favorites'));
document.getElementById('fav-add').addEventListener('click', addFav);
```

- [ ] **Step 3: 加收藏渲染与交互函数**

追加到 script 内（renderSources 之后）。全部经 textContent/value/addEventListener 注入，遵守文件顶部 XSS 约定：

```js
function renderFavorites(favs) {
  const wrap = el('div');
  for (const [key, f] of Object.entries(favs)) {
    const single = f.skills.length === 1 && f.skills[0].source_path === '.';
    const head = el('p');
    head.appendChild(el('strong', key));
    const meta = f.url ? `  （${(f.commit||'').slice(0,7)}，收藏于 ${(f.bookmarked_at||'').slice(0,10)}）` : '  （本地源）';
    head.appendChild(document.createTextNode(meta));
    if (single) {
      // 单技能仓库：二级留空，用途与操作挂一级行
      head.appendChild(document.createTextNode(' — ' + f.skills[0].description));
      const ins = el('button', '安装');
      ins.addEventListener('click', () => openInstall(key, f.skills[0].name));
      head.appendChild(ins);
      const del = el('button', '删除');
      del.addEventListener('click', () => rmFav(key, []));
      head.appendChild(del);
      wrap.appendChild(head);
      continue;
    }
    const delAll = el('button', '删除整包');
    delAll.addEventListener('click', () => rmFav(key, []));
    head.appendChild(delAll);
    wrap.appendChild(head);
    const table = el('table');
    for (const s of f.skills) {
      const tr = el('tr');
      tr.appendChild(el('td', s.name));
      tr.appendChild(el('td', s.description));
      const ins = el('button', '安装');
      ins.addEventListener('click', () => openInstall(key, s.name));
      cell(tr, ins);
      const del = el('button', '删除');
      del.addEventListener('click', () => rmFav(key, [s.name]));
      cell(tr, del);
      table.appendChild(tr);
    }
    wrap.appendChild(table);
  }
  return wrap;
}
async function addFav() {
  const source = document.getElementById('fav-source').value.trim();
  const skill = document.getElementById('fav-skill').value.trim();
  if (!source) return;
  try {
    await api('favorites', {method:'POST', headers:{'content-type':'application/json'},
      body: JSON.stringify({source, skill: skill ? [skill] : []})});
    refresh();
  } catch (e) {
    alert('收藏失败: ' + e.message);
  }
}
async function rmFav(source, skills) {
  if (!confirm(`删除收藏 ${source}${skills.length ? ' 的 ' + skills.join(',') : ''}？`)) return;
  await api('favorites/remove', {method:'POST', headers:{'content-type':'application/json'},
    body: JSON.stringify({source, skill: skills})});
  refresh();
}
// 安装面板：global target 下拉（/api/targets）+ project 绝对路径输入 + method 选择
let installPanel = null;
async function openInstall(source, skill) {
  if (installPanel) installPanel.remove();
  let targets;
  try {
    targets = await api('targets');
  } catch (e) {
    alert('加载 targets 失败: ' + e.message);
    return;
  }
  const panel = el('p');
  panel.appendChild(el('span', `安装 ${skill} → `));
  const sel = document.createElement('select');
  for (const t of targets) {
    const opt = el('option', `global:${t.name}`);
    opt.value = `global:${t.name}`;
    sel.appendChild(opt);
  }
  const optP = el('option', 'project（绝对路径）');
  optP.value = 'project';
  sel.appendChild(optP);
  panel.appendChild(sel);
  const pathInput = document.createElement('input');
  pathInput.placeholder = '/abs/project/path';
  pathInput.style.display = 'none';
  sel.addEventListener('change', () => {
    pathInput.style.display = sel.value === 'project' ? '' : 'none';
  });
  panel.appendChild(pathInput);
  const msel = document.createElement('select');
  for (const [v, label] of [['', '默认'], ['symlink', 'symlink'], ['copy', 'copy']]) {
    const o = el('option', label);
    o.value = v;
    msel.appendChild(o);
  }
  panel.appendChild(msel);
  const ok = el('button', '确认安装');
  ok.addEventListener('click', () => doInstall(source, skill, sel.value, pathInput.value.trim(), msel.value));
  panel.appendChild(ok);
  const cancel = el('button', '取消');
  cancel.addEventListener('click', () => { panel.remove(); installPanel = null; });
  panel.appendChild(cancel);
  document.getElementById('favorites').appendChild(panel);
  installPanel = panel;
}
async function doInstall(source, skill, targetSel, projectPath, method) {
  const target = targetSel === 'project'
    ? {kind:'project', root: projectPath}
    : {kind:'global', name: targetSel.slice('global:'.length)};
  const body = {source, skill, target, overwrite: false};
  if (method) body.method = method;
  try {
    await api('favorites/install', {method:'POST', headers:{'content-type':'application/json'},
      body: JSON.stringify(body)});
    alert('已安装 ' + skill);
  } catch (e) {
    // 409 = 目标已存在：确认后以 overwrite 重试一次（同 runUpdate 的确认链）
    if (e.status === 409) {
      if (!confirm(e.message + '\n\n覆盖现有目录？')) return;
      body.overwrite = true;
      try {
        await api('favorites/install', {method:'POST', headers:{'content-type':'application/json'},
          body: JSON.stringify(body)});
        alert('已覆盖安装 ' + skill);
      } catch (e2) {
        alert('安装失败: ' + e2.message);
      }
      return;
    }
    alert('安装失败: ' + e.message);
  }
}
```

- [ ] **Step 4: 更新 CLAUDE.md 的子命令列表**

`CLAUDE.md` 常用命令一节：

```bash
cargo run -- list                  # 本地运行 CLI（子命令：add/list/remove/update/tag/auto-update/config/tui/ui）
```

改为：

```bash
cargo run -- list                  # 本地运行 CLI（子命令：add/list/remove/update/tag/auto-update/config/fav/tui/ui）
```

- [ ] **Step 5: 全量验证**

```bash
cargo fmt
cargo clippy --all-targets
cargo test
```

Expected: fmt 无改动、clippy 无警告、全部测试绿。

Web 端手动冒烟（前端无自动化测试，与项目现状一致）：

```bash
cargo run -- ui --no-open --port 17823 &
# 另一个终端：
# 1. curl -s localhost:17823/ | grep 收藏                    → 页签在
# 2. curl -s -X POST localhost:17823/api/favorites -H 'content-type: application/json' \
#      -d '{"source":"<本地技能目录绝对路径>","skill":[]}'    → {"key":"local/...","skills":N}
# 3. curl -s localhost:17823/api/favorites                  → 两级 JSON
# 4. curl -s localhost:17823/api/targets                    → [{name,path}...]
# 5. 浏览器打开页面点一遍：收藏 → 安装（确认链）→ 删除
```

- [ ] **Step 6: 提交**

```bash
git add src/web/static/index.html CLAUDE.md
git commit -m "feat: Web 收藏页面前端 + CLAUDE.md 子命令列表更新

Co-Authored-By: Claude Fable 5 <noreply@anthropic.com>"
```

---

## 收尾检查单（全部任务完成后）

- [ ] `cargo test` 全绿（单元 + cli_smoke + e2e）
- [ ] `cargo fmt --check` 无输出；`cargo clippy --all-targets` 无警告
- [ ] spec 对照：`docs/superpowers/specs/2026-08-25-skill-favorites-design.md` 每节都有落地（范围外条目未偷跑实现）
- [ ] 既有不变量未被触碰：install/update/remove 原子性、归属核验、config 自愈分发、路径安全（由既有测试套件锁定）

# 技能收藏（fav）功能设计

日期：2026-08-25
状态：已批准
关联：`2026-08-20-skills-manager-design.md`（产品本体）

## 概述

新增「技能收藏」：只记录技能地址与功能描述，**不安装**。收藏整个技能包仓库时，列出仓内每个技能的功能；收藏支持一键转安装。CLI / TUI / Web 三端全覆盖。

收藏与安装是正交的两份记录：收藏不写 installs、不碰任何目标目录；安装过的技能可以留在收藏里，收藏的技能也可以不装。

## 核心语义

- **收藏对象**：技能包（source）。`fav <source>` 收藏整仓（扫描到的全部技能）；`fav <source> --skill <名>` 只收藏指定技能（`--skill` 可重复，与 add 的 `-s` 对齐；给出的每个技能名都必须在仓内存在，否则报 `Error::Msg("仓库中无技能 x")`）。source 表达式复用 `parse_source`（GitHub 简写 / https / ssh / file:// / 本地绝对路径），不做 GitHub `/tree/` 子目录链接解析。
- **两级结构**：一级 = 技能包（source key），二级 = 技能名 + 用途。单技能仓库（根级 SKILL.md，source_path 为 `.`）二级留空，用途直接挂在一级行。
- **快照**：收藏时把扫描到的技能名/用途/source_path 连同缓存 HEAD commit 写入 registry；`fav` 列表离线可看，不做实时扫描。重复 `fav <repo>` = 复用缓存重扫并覆盖该 source 的收藏快照，即手动刷新手段（ensure_cached 对 git 源本来就只复用不 pull，与 add 一致；要拉新内容走既有 `skills update` + `auto-update --source` 体系）。
- **单技能 upsert**：`--skill` 收藏时，source 条目不存在则新建（只含指定技能），已存在则只覆盖指定技能条目，不动同 source 下其他已收藏技能。
- **来源登记**：fav 时把 source `or_insert` 进 `registry.sources`（与 add 相同逻辑，已存在则不覆盖），让缓存目录有唯一权威登记处，TUI/Web 的「仓库缓存」视图自然可见；auto_update 默认 None，不参与 update 拉取。

## 数据模型（registry.json）

`Registry` 新增第三段，`#[serde(default)]` 保证旧版 registry.json 无此段也能加载：

```rust
pub struct Favorite {
    pub url: Option<String>,          // git 源有；本地源为 None
    pub local_path: Option<PathBuf>,  // 本地源有；fav install 据此重建 SourceSpec
    pub commit: String,               // 收藏时缓存 HEAD 快照（本地源为空串）
    pub bookmarked_at: String,        // RFC3339
    pub skills: Vec<FavSkill>,
}

pub struct FavSkill {
    pub name: String,
    pub description: String,
    pub source_path: PathBuf,         // 相对缓存根，fav install 直接喂给 install_skill
}

// Registry 内：
#[serde(default)]
pub favorites: BTreeMap<String, Favorite>,   // key = source key（如 github/o/r）
```

落盘沿用 `Registry::save` 的 tmp+rename 原子写。

## core::favorites 模块

纯记录操作 + 安装转接，不引入第二套磁盘逻辑：

- `bookmark(layout, spec, skills: &[String]) -> Result<Favorite>`
  ensure_cached（复用缓存）→ scan_skills → skills 为空则全量、非空则只收藏列出的（逐个校验存在性）→ 组装/更新 Favorite。clone/扫描失败时 ensure_cached 已自清半成品，favorite 不落盘。
- `unbookmark(reg, source_key, skills: &[String]) -> Result<()>`
  skills 为空删整条 source 收藏；非空只删列出的技能（技能删光则级联删 source 条目）。**不动缓存、不动 installs**——与 remove 不清缓存的既有行为一致。
- `fav_install(layout, cfg, reg, source_key, skill, target, method) -> Result<Install>`
  从收藏快照取 source_path（不重扫）；缓存目录缺失时凭 Favorite 里的 url/local_path 重建 SourceSpec 并 ensure_cached 自愈（仅缺失时，本地源也不例外）。之后与 add 完全同路：`install_skill` + `Error::Conflict` 交由前端决策 + 调用方逐条落盘。

错误处理：新增 `Error::NotBookmarked(String)` 变体（fav install/rm 未命中收藏时返回）。

## CLI：`fav` 子命令

```bash
skills fav <source> [--skill <名>]                          # 收藏
skills fav                                                  # 无参：两级列表
skills fav rm <source> [--skill <名>]                       # 删收藏
skills fav install <source> [--skill <名>] [-t 目标]... [-g] [--method X] [-y]
```

- 解析形态：`Fav { source: Option<String>, skill: Vec<String>, yes, sub: Option<FavSub> }`，`FavSub = Rm { source, skill: Vec<String> } | Install { source, skill: Vec<String>, target, global, method, yes }`，用 `args_conflicts_with_subcommands` 让 `rm`/`install` 优先按子命令解析；解析行为由 CLI 测试锁定。
- rm/install 的 `<source>` 先精确匹配 favorites 的 key（如 `github/o/r`），不匹配再走 `parse_source` 规范化后匹配——贴 key 或贴 URL 都行。
- `fav install` 未给 `--skill` 且该收藏含多个技能时，dialoguer MultiSelect 从**收藏的技能集**里选（不重扫全仓）；target/method/Conflict 确认/-y 跳过逻辑与 add 逐字一致。
- 列表输出（无收藏时打印 `（无收藏）`，与 list 的 `（无已安装技能）` 对齐）：

```
github/mattpocock/skills    (a1b2c3d, 收藏于 2026-08-25)
  ├─ web-design — 前端设计技能
  └─ tdd — 测试驱动开发
local/my-skill — 单技能的用途描述    (本地源)
```

单技能仓库的判定：`skills.len() == 1 && skills[0].source_path == "."`，此时一级行直接带用途，无二级行。

## TUI：第四视图「收藏」

- `View::Favorites` 加入 Tab 循环：已安装 / 安装向导 / 仓库缓存 / **收藏**。`AppState` 新增收藏视图的扁平行展开（`FavRow::Source(source_key)` 标题行 + `FavRow::Skill(source_key, idx)` 技能行），`selected` 索引扁平行。
- 按键（仅收藏视图生效）：
  - `f`：收藏向导。suspend 终端 → dialoguer 输入 source → 扫描后 MultiSelect（默认全选）→ `core::favorites::bookmark`。全程复用 install_wizard 的 suspend/resume 模式。
  - `d`：删除选中收藏。标题行 = 删整包，技能行 = 删单个。
  - `i`：从收藏安装。技能行直接带出 source_path；目标选择复用 install_wizard 的 target Select 段（global targets + 当前项目）。标题行按 `i` 时若多技能则先 MultiSelect 选技能。
- 与既有惯例一致：只操作内存 `app.registry`，正常退出时统一落盘；错误路径不落盘。

## Web：「收藏」页签 + 5 个端点

- `GET /api/favorites` → 两级 JSON（favorites map 原样）。
- `POST /api/favorites`：body `{source, skill?}`。同步 clone+扫描（与 run_update 同为 handler 内阻塞执行，本期不改这个既有取舍）。
- `POST /api/favorites/remove`：body `{source, skill?}`。
- `POST /api/favorites/install`：body `{source, skill, target: TargetRec, method?, overwrite?}`。冲突返回 409，前端 confirm 后带 `overwrite=true` 重试——复用 run_update 已验证的 409 确认链模式。
- `GET /api/targets`：返回 config 的 global targets（`[{name, path}]`）。Web 端此前没有任何安装入口，fav install 是首个，安装对话框 = global 下拉（数据来自本端点）+ project 绝对路径输入 + method 选择。

index.html 加「收藏」nav 按钮、收藏输入框（source + 可选 skill 名）与两级列表（source 标题行含收藏时间/删除整包按钮；技能行含用途、安装、删除按钮）。渲染严格遵循文件顶部既有 XSS 安全约定：一律 textContent/value 注入、addEventListener 绑定，禁 innerHTML 拼接。

## 安全与既有不变量

favorites 是纯记录层：不触碰目标目录，不改变 install/update/remove/git 的任何原子性与归属核验语义。fav install 走 install_skill 原路径，路径校验（技能名单一组件、source_path 不出缓存根）原样生效。Web 新端点与既有端点同构（Mutex + 显式 500/404/409），错误不静默。

## 测试计划

- **单元（core::favorites）**：serde roundtrip；旧版 registry（无 favorites 段）加载兼容；bookmark 整仓/单技能/upsert/重复收藏覆盖语义；unbookmark 删单个/删空级联/未命中报错；单技能仓库归并一级的判定函数。
- **集成（tests/e2e.rs 增补）**：本地 bare 仓库 `fav` → 列表断言两级输出 → `fav <repo> --skill` → `fav rm --skill` → `fav install`（list 出现副本、registry installs 有记录、收藏仍在）→ 删缓存后 `fav install` 自愈重克隆。全程 file:// 无网络、SKILLS_HOME 隔离。
- **CLI 冒烟（tests/cli_smoke.rs 增补）**：`fav --help`、`fav rm`/`fav install` 子命令解析、rm/install 的 source 既能给 key 也能给 URL。
- **Web**：tower oneshot 测 5 个新端点（含 install 的 409 冲突链与 overwrite 重试），沿用 api.rs 现有测试模式。
- **TUI**：reducer 单测（Tab 四环切换、收藏视图导航 clamp、删除动作按行类型分发）+ 四视图渲染冒烟。

## 范围外（YAGNI）

- 收藏的自动刷新（重复 fav 即手动刷新；仓库内容更新走既有 update 体系）。
- 收藏条目的 tag 分类。
- GitHub `/tree/` 子目录链接解析（`--skill` 已覆盖单技能指定）。
- Web 端 `add` 命令等价物（fav install 是唯一安装入口）。

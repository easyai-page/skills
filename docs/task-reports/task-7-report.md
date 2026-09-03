# Task 7 报告：TUI 收藏/安装向导（f/i/d 按键接线）

分支：feat/skill-favorites　提交：6aad56f　日期：2026-08-27

## 实现内容

按 brief 逐字落地，改动两个文件：

### src/tui/mod.rs（+154/-25）

1. **抽取 `pick_target(cfg) -> Result<Target>`**：install_wizard 内联的「配置的 global targets + 当前项目」Select 段整体抽为自由函数，install_wizard 原位置替换为 `let target = pick_target(cfg)?;`，行为不变。
2. **`fav_wizard(layout, app)`**：输 source → parse_source → ensure_cached（复用提示）→ scan_skills → MultiSelect 默认全选 → `favorites::bookmark`；全选时传 `&[]` 走整仓覆盖语义（快照随仓库收缩可刷新干净），部分选走 upsert；直接操作 `app.registry` 并 `save`（与 install_wizard 同因：TUI 内未落盘的 auto_update 切换不得被磁盘副本覆盖）。
3. **`fav_install_wizard(layout, cfg, app)`**：当前行定技能集——技能行直取该行技能；标题行单技能仓库直取唯一技能，多技能仓库先 MultiSelect；然后 pick_target → 逐技能 `favorites::fav_install`；`Error::Conflict` 弹 Confirm，确认则 remove 后重装，否则跳过；结束 `save`。
4. **按键接线（event_loop）**：
   - `i` 按视图分发：Favorites → fav_install_wizard，其他 → install_wizard（suspend/resume + 失败时「按回车返回 TUI」的既有模式不变）。
   - 新增 `f`（仅 Favorites 视图）→ fav_wizard，同样的 suspend/resume 模式。
   - match 新增 `KeyCode::Char('d') => Action::DeleteFav`（reducer 内部已限定仅 Favorites 视图生效，Task 6 测试锁定）。
5. import 增补：`favorites`（core）、`FavRow`/`View`（app）。

### src/tui/app.rs（-3）

移除 `Action::DeleteFav` 上的 `#[allow(dead_code)]` 及过渡注释——'d' 键接线后变体已由生产代码构造。

## 测试结果（RED/GREEN 证据）

- **基线（改动前）**：`cargo test` 143 全绿（123 单元 + 13 cli_smoke + 7 e2e）。
- **Step 1 偏差说明**：brief 复选框标题写「写按键分发的回归测试」，但正文明确「无需新测试」——DeleteFav 分发语义由 Task 6 reducer 测试（`delete_fav_skill_row_then_source_row` 等）锁定，落盘不丢内存切换由既有 `wizard_save_path_preserves_in_memory_auto_update_toggle` 锁定，向导交互与 install_wizard 同标准（无单测、手动验证）。以正文为准，未新增测试。
- **RED（编译期证据）**：先在 app.rs 移除 `#[allow(dead_code)]` 而未接线 'd' 键 → `cargo clippy --all-targets` 报 `warning: variant DeleteFav is never constructed`（项目零警告标准下即失败态）。
- **GREEN**：完成 mod.rs 全部改动后——`cargo test` 143 全绿（与基线数量一致，纯重构 + 新代码路径）；`cargo clippy --all-targets` 零警告（dead_code 警告随接线消除）；`cargo fmt` 已跑。
- **手动验证（brief Step 5 的脚本化等价）**：pty 驱动真实 `target/debug/skills tui`（HOME/SKILLS_HOME/cwd 全部隔离到临时目录，fixture 为双技能本地源），完整走完 brief 流程并逐项核对：
  1. Tab×3 切到收藏页，底栏渲染 `f=收藏 d=删除 i=安装` ✓
  2. `f` → 输入本地路径 → MultiSelect 两项均预选 `[x]` → Enter → 输出「已收藏 local/src（2 个技能）」✓（全选 = 整仓语义）
  3. 列表出现两级行：标题行 `local/src  2 个技能  2026-08-27` + 技能行 alpha/beta ✓
  4. 选中技能行按 `i` → 跳过技能多选（技能行语义正确）→ pick_target 列出 global:agents/claude/codex/project:<cwd> → Enter →「已安装 alpha → Global { name: "agents" } (Symlink)」✓；磁盘产物为 symlink → 缓存目录 ✓
  5. 标题行按 `d` → 整包收藏消失 ✓；`q` 退出落盘后 registry.json：favorites={}、installs=[alpha]、sources=[local/src] ✓
  6. 重进 TUI 状态一致、正常退出（rc=0）✓

## 与 brief 的偏差

1. **未新增测试**（上详）——brief 复选框标题与正文矛盾，以正文「无需新测试」为准。
2. **import 增补 `View`**：brief 未提，但 Step 4 的接线代码引用 `View::Favorites`，编译必需；同理 `FavRow` 加入 `use app::{...}`。
3. **多改了 src/tui/app.rs**：brief Files 段只列 mod.rs，但上游任务要求移除 DeleteFav 的 `#[allow(dead_code)]`，必须动 app.rs；commit 相应包含两个文件（brief Step 6 只写 `git add src/tui/mod.rs`）。

## 留给审查者的疑虑

1. **`fav_wizard` 里 `n` 的显示口径**：`bookmark` 返回的 count 在整仓语义下等于扫描数，brief 代码又做一次 `if picked.is_empty() { all.len() } else { n }`，属于防御性冗余（两值恒等），逐字保留未动。
2. **`fav_install_wizard` 的覆盖路径**：`remove_install` 的返回值被 `let _ =` 吞掉（与 install_wizard 既有写法一致）——若 remove 因 Mismatch 失败而重装成功，registry 可能出现重复记录风险；此为 install_wizard 既有语义的延续，非本任务引入，未擅自改动。
3. **'d' 键在非收藏视图落入 `_ => continue` 之前会被 match 捕获**构造 DeleteFav，靠 reducer 内部视图判断 no-op；语义已被 Task 6 测试锁定，但与 'f' 的「事件循环层就按视图过滤」风格不完全对称，系按 brief 逐字实现。
4. 工作区存在与本任务无关的既有变动（task-8..15 报告的工作树删除、task-3..6 报告未跟踪），提交时已用显式 pathspec 隔离，未裹挟。

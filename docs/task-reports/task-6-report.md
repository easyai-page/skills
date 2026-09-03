# Task 6 报告：TUI 收藏视图——reducer + 渲染

日期：2026-08-27
分支：feat/skill-favorites
提交：0a12d2e `feat: TUI 收藏视图（四环页签 + 两级列表 + 删除）`

## 实现内容

严格按 `.superpowers/sdd/2026-08-25-skill-favorites/task-6-brief.md` 的代码逐字落地，改动两个文件：

### src/tui/app.rs
- `View` 增加 `Favorites`，Tab 切换从三环改四环（NextView: Installed→Install→Sources→Favorites→Installed；PrevView 反向）。
- `Action` 增加 `DeleteFav`。
- 新增 `FavRow::{Source(String), Skill(String, usize)}` 扁平行枚举。
- 新增 `AppState::fav_rows()`：多技能仓库 = 标题行 + 每技能一行；单技能仓库（`favorites::is_single_skill_repo` 判定）只出标题行。
- `Action::Down` 改为按视图计数：Installed 用 `visible_rows()`、Favorites 用 `fav_rows()`、Install/Sources 返回 0 行（brief 明示的刻意 quirk 修复——此前任何视图都用 installs 行数）。
- `Action::DeleteFav` 分支（加在 ToggleAutoUpdate 之后）：非收藏视图直接 return；标题行调 `unbookmark(key, &[])` 删整包，技能行按当前下标取名字后调 `unbookmark(key, &[name])` 删单个；删除后 selected clamp 到收缩后的合法范围。
- 按 brief 删除旧 `tab_switches_view` 三环测试（被 `tab_cycles_four_views` 覆盖）。

### src/tui/ui.rs
- 页签行改四环：`["已安装", "安装向导", "仓库缓存", "收藏"]`。
- `View::Favorites` 渲染分支（放 Sources 之后）：表格列 收藏(35%)/用途(50%)/收藏时间(15%)；标题行单技能仓库用途挂一级、多技能仓库显示「N 个技能」；技能行缩进两空格、时间为空；`bookmarked_at` 取前 10 字符为日期；块标题「f=收藏 d=删除 i=安装」。
- 冒烟测试 `draw_all_views_smoke` 视图数组扩为四视图；fixture 增加两条收藏（多技能 github/o/r + 单技能 local/solo），两种渲染分支都走到，只渲染不断言。

## 测试结果

### RED（Step 2 证据）
写测试后 `cargo test tui::app` 编译失败，与 brief 预期一致：

```
error[E0599]: no method named `fav_rows` found for struct `app::AppState`
error[E0599]: no variant or associated item named `DeleteFav` found for enum `app::Action`
error: could not compile `skills` (bin "skills" test) due to 13 previous errors
```

（另含 View::Favorites / FavRow 未定义等 E0433/E0599，共 13 个编译错误。）

### GREEN（Step 5 证据）
实现后 `cargo test tui`：8 个测试全过——

```
test tui::app::tests::favorites_rows_flatten_two_levels ... ok
test tui::app::tests::navigation_wraps_and_clamps ... ok
test tui::app::tests::delete_fav_skill_row_then_source_row ... ok
test tui::app::tests::tab_cycles_four_views ... ok
test tui::app::tests::favorites_navigation_clamps_per_view ... ok
test tui::app::tests::toggle_auto_update_flips_selected_copy_install ... ok
test tui::ui::tests::draw_all_views_smoke ... ok
test tui::tests::wizard_save_path_preserves_in_memory_auto_update_toggle ... ok
```

### 全量验证
- `cargo test`：143 全绿（123 单元 + 13 cli_smoke + 7 e2e），fmt 后复跑 tui 仍 8/8 过。
- `cargo clippy --all-targets`：零警告。
- `cargo fmt`：已跑，仅影响本次改动文件的排版。

## 与 brief 的偏差

1. **`DeleteFav` 变体加 `#[allow(dead_code)]`**（唯一语义外偏差）。事件循环的 `KeyCode::Char('d')` 接线属于 Task 7（计划文档 1954 行确认），本任务落地后非 test 构建中该变体无构造点，`cargo clippy --all-targets` 会报 dead_code 警告，与「零警告」验收冲突。处理：在变体上加 `#[allow(dead_code)]` 并附中文注释说明过渡原因，沿用 `src/core/git.rs` `FailurePoint` 的既有先例（同样仅测试构造）。Task 7 接线后应移除该 allow。
2. **ui.rs 冒烟测试 fixture 加了收藏条目**。brief 原文允许「可加一条 favorite（也可不加）」，实际加了两条（多技能 + 单技能各一），目的是让收藏视图的两种渲染分支都在冒烟中走到；仍只渲染不断言，符合 brief 的建议。
3. 测试 import 未按 brief 片段单独开一行 `use crate::core::registry::{FavSkill, Favorite};`，而是合并进既有 use 行（rustfmt 惯例，无语义差别）。

## 留给审查者的疑虑

1. **Task 7 完成后需移除 `#[allow(dead_code)]`**：'d' 键接线后该属性即成多余，若忘记移除会掩盖未来真正的死代码。
2. **删除无确认、错误被吞**：`DeleteFav` reducer 内 `let _ = unbookmark(...)` 忽略错误且无二次确认（brief 明示「无需交互」）。reducer 层错误只剩「行与 registry 不同步」一种理论可能（fav_rows 刚从同一 registry 生成，实际不会触发），可接受；但 TUI 惯例是退出时才统一落盘，用户误删后只要不退出、强杀进程即可不持久化——这是既有 TUI 事务模型的自然行为，审查者可确认这是否符合预期。
3. **Install/Sources 视图 Down 键 clamp 到 0**：brief 明示这是「有意修正的边界 quirk」。行为后果：在这两个视图按 j/Down 会把 selected 归 0（此前会按 installs 行数累计）。两视图本无可选行，影响仅为切回 Installed 时选中行可能变化（切视图本就 selected=0，实际无感），记录在案供确认。
4. **收藏时间截断取前 10 字符**：`bookmarked_at` 为 RFC3339，前 10 字符恰为 `YYYY-MM-DD`；若未来格式变动（如缺省写入非标准串）会显示截断垃圾。当前写入路径（chrono `to_rfc3339()`）恒定，风险低。

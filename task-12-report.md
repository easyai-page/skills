# 任务 12 报告：TUI（ratatui + crossterm，三视图）

## 范围

按 `.superpowers/sdd/2026-08-20-skills-manager/task-12-brief.md` 实现三视图 TUI。TUI 为薄前端：状态只有 `AppState`（registry 副本 + view + selected + tag_filter），所有持久化与安装业务全部走 `core`。

## 文件

- `src/tui/app.rs`（新建）—— `View` / `Action` / `AppState` + 纯函数 `reduce` reducer；内联 4 个 reducer 测试（简报原文）。
- `src/tui/ui.rs`（新建）—— 无状态 ratatui 渲染：顶部 Tabs（已安装/安装向导/仓库缓存），已安装视图五列表格（技能/方式/目标/分类/自动更新，选中行黄色高亮），仓库缓存视图列出 source commit 前 7 位与 auto_update。附 1 个 `TestBackend` 三视图渲染冒烟测试（不做像素断言）。
- `src/tui/mod.rs`（重写）—— 事件循环与入口 `run`。

## 与简报的一致/偏差

一致：
- reducer 逻辑、三态 auto_update 循环（None→true→false→None，Symlink 跳过）、视图切换顺序、tag 过滤、`q/Esc`、`j/k/↑/↓`、`Tab/BackTab`、`a` 键位全部按简报。
- 退出时 `registry.save(layout)` 落盘。

偏差（均为任务约束要求，不改变简报行为）：
1. **RAII 终端守卫**：新增 `TermGuard`（`enter/suspend/resume` + `Drop`），run 的正常、错误、panic unwind 路径都会恢复 raw mode 与 Alternate Screen；事件循环返回后先显式 `suspend` 再传播 `Result`。
2. **安装向导 `i` 键**：按简报末尾的范围控制说明实现——`suspend` 切出 raw mode 后用 dialoguer 走 `Input(source) → MultiSelect(技能) → Select(目标：配置的 global targets + 当前 project)`，安装/冲突覆盖/登记全部复用 `core::{cache, install, remove}`，完成后重载 registry 刷新表格、`resume` 恢复 TUI。向导失败时在切出状态打印错误并等待回车，回到 TUI 不丢现场。
3. **`Enter → Action::Select`**：消除 `Select` 变体 dead_code 警告，为后续详情视图预留入口（reducer 中仍是 no-op）。
4. `ui.rs` 中 `visible_rows()` 的临时 Vec 先绑定再迭代（`E0716` 编译修正）。

## 测试

- TDD：先写 4 个 reducer 测试，`cargo test tui` 编译失败（red，16 个 resolve 错误）→ 实现后通过（green）。
- `cargo test tui`：5 passed（4 reducer + 1 ui smoke）。
- `cargo test`（全量）：87 unit + 10 cli_smoke = 97 passed, 0 failed。
- `cargo fmt` 已跑；`cargo build` 无 tui 相关警告（其余警告为任务 1-11 遗留）。

## 提交

- `feat(tui): 三视图 TUI（已安装/安装向导/仓库缓存）`

## 第 1 轮修复（审查后跟进）

- **重要（必修）**：`install_wizard` 不再从磁盘 `Registry::load` 全新副本，改为直接操作 `app.registry`（`&mut app.registry`），末尾 `reg.save(layout)` 落盘的即是内存状态；用户在 TUI 内按 `a` 的 auto_update 切换不再被静默覆盖。删除原 `app.registry = Registry::load(layout)?` 刷新行（同一对象，无需刷新）。
  - 测试：`tui::tests::wizard_save_path_preserves_in_memory_auto_update_toggle`——reducer 切换（仅内存）→ 走向导同款 save 路径 → 重载断言切换仍在。
- **次要**：`run()` 错误路径跳过退出落盘，加注释说明语义（异常时内存状态可信度低，保留磁盘一致快照）。
- **次要**：`ui.rs` Sources 视图 commit 截断改 `chars().take(7)`，防多字节 UTF-8 字节切片 panic。
- **次要**：`TermGuard::enter` 半路失败恢复——`EnterAlternateScreen` 失败时 `disable_raw_mode` 后再返回错误。

验证：`cargo fmt`；`cargo test tui` 6/6 通过；`cargo test` 98/98 通过。

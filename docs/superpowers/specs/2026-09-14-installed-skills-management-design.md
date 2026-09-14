# 已安装技能管理增强设计：删除、单项手动更新、更新模式与安装方式配置

日期：2026-09-14  
状态：已确认（用户已批准方案 2：TUI 动作管理菜单 + Web 综合弹窗与行内操作）

## 背景与目标

当前 `skills` 的 TUI 和 Web 界面在「已安装技能」的管理能力上存在明显缺口：
1. **TUI 已安装列表**：
   - 缺少删除操作（`d` 仅在收藏视图生效，已安装视图无响应）；
   - 缺少手动更新操作（无法单独更新某个选中的技能副本）；
   - 缺少配置技能更新模式与安装方式的操作（`a` 键仅对 copy 副本在内存中做开/关/跟随的静默三态循环，对 symlink 无效，且未即时落盘，无显式菜单）。
2. **Web 界面**：
   - 缺少单项手动更新操作（顶部仅有全量更新，已安装列表每行无法单独触发该技能的更新）；
   - 缺少修改技能更新模式与安装方式操作（symlink 副本仅显示静态文本「跟随包级」，无法配置或联动修改所属包策略，更无法在 symlink 与 copy 之间转换）。

本项目通过在 Core 层沉淀单项手动更新与安装方式原子互转能力，并在 TUI（采用集中操作菜单）与 Web（采用集中管理弹窗 + 行内便捷操作）端全面补齐：
- **删除操作 (Remove)**：带安全确认的物理与记录删除。
- **单项手动更新 (Manual Update)**：普通更新与强制覆盖更新（检测到本地修改时提示并支持确认强制覆盖）。
- **自动更新策略配置 (Auto-Update Policy)**：开 / 关 / 跟随包级，symlink 项支持联动查看与配置所属仓库策略。
- **安装方式原子互转 (Switch Method)**：symlink ↔ copy 互转，支持归属校验与本地修改保护。

---

## 架构与安全不变量

所有新增与变更必须严格遵守项目的六大安全不变量：
1. **原子性**：方式转换与更新均走 staging 临时目录 + rename 提交或事务安全操作；任一步骤失败安全回滚。
2. **归属核验**：删除与方式转换时，核验 copy 副本上的 `.skills-manifest` 标识；实况与记录不符时拒绝操作并返回 `Mismatch`，绝不误删用户数据。
3. **本地修改保护**：从 copy 转换为 symlink 或执行非 force 更新时，先核验 manifest 文件内容与哈希。若副本偏离基线且未显式指定 `force`，返回 `Mismatch` 并中断，保护用户未提交的修改。
4. **磁盘与 Registry 一致性**：每项操作成功后即时原子落盘 `reg.save(layout)`，杜绝因进程意外退出产生未保存状态。
5. **路径安全**：严格校验相对与绝对路径组件，防止 symlink 逃逸。

---

## Core 层设计 (`src/core/`)

### 1. 单项手动更新：`src/core/update.rs`

新增通用单项更新函数：
```rust
pub fn update_single(
    layout: &Layout,
    cfg: &Config,
    reg: &mut Registry,
    skill: &str,
    target: &TargetRec,
    force: bool,
) -> Result<Vec<String>>
```
- **流程**：
  1. 构造 `Selection { skill: skill.to_string(), target: target.clone() }`。
  2. 调用已有 `build_plan(reg, Some(&sel))`。若记录不存在（`plan.missing.is_some()`），返回 `Error::NotInstalled`。
  3. 调用 `execute_plan(layout, cfg, reg, &plan, force)`。
  4. 若副本为 copy 且本地被改动（文件哈希不符/文件增删），在 `!force` 时抛出 `Error::Mismatch`，带冲突明细。
  5. 成功后原子落盘 `reg.save(layout)` 并返回已更新的明细列表（若已是最新，返回空列表）。

### 2. 安装方式原子互转：`src/core/install.rs`

新增安装方式切换函数：
```rust
pub fn switch_install_method(
    layout: &Layout,
    cfg: &Config,
    reg: &mut Registry,
    skill: &str,
    target: &TargetRec,
    new_method: Method,
    force: bool,
) -> Result<()>
```
- **流程**：
  - 查找记录 `reg.find(skill, target)`，不存在则返回 `Error::NotInstalled`。
  - 若 `inst.method == new_method`，直接 `Ok(())`（幂等）。
  - **Copy → Symlink**：
    1. 本地修改检测：核验目标目录的 manifest 基线。若存在本地改动且 `!force`，返回 `Error::Mismatch`，拒绝覆盖。
    2. 安全移除原 copy：调用 `remove_recorded_copy`。
    3. 重新建立 symlink：校验缓存源路径后调用 `symlink_dir`。
    4. 更新内存记录：`inst.method = Method::Symlink`，`inst.auto_update = None`（软链接始终跟随包级）。
    5. 原子落盘 `reg.save(layout)`。
  - **Symlink → Copy**：
    1. 安全移除原 symlink：调用 `remove_recorded_link`。
    2. Staging + 原子 Rename：以当前缓存 commit 为基线，拷贝文件至临时 staging 目录，生成 `.skills-manifest`，重命名至目标路径。
    3. 更新内存记录：`inst.method = Method::Copy`。
    4. 原子落盘 `reg.save(layout)`。

---

## TUI 交互设计 (`src/tui/`)

### 1. 按键映射与操作入口 (`src/tui/mod.rs` & `src/tui/app.rs`)
- 在「已安装」视图 (`View::Installed`) 中：
  - `Enter`（或 `Space`）：唤起当前选定安装项的**管理菜单** (`installed_action_menu`)。
  - `d`：快捷键，直接进入删除二次确认。
- 标题栏更新 (`src/tui/ui.rs`)：
  - 展示为：`已安装 [本项目+全局] (Enter 管理操作，d 删除，p 切换过滤)`。

### 2. 管理菜单交互流程 (`installed_action_menu`)
按下 `Enter` 后调用 `guard.suspend()` 暂时切出 raw mode，使用 `inquire` 进行对话式交互：
1. **顶部信息展示**：
   ```text
   ▶ 管理技能: <skill> @ <target>
     当前方式: <copy|symlink> | 自动更新: <开|关|跟随包级> | 分类: <tags>
   ```
2. **菜单选项**：
   - `1. 手动更新 (普通更新)`：调用 `update_single(..., force: false)`。
     - 若遭遇 `Mismatch`，打印修改明细并弹出 `inquire::Confirm` 询问是否放弃修改强制覆盖；确认后以 `force: true` 重试。
     - 成功打印更新结果，等待用户回车返回。
   - `2. 强制覆盖更新`：二次确认后直接以 `force: true` 调用 `update_single`。
   - `3. 配置自动更新策略`：
     - 若为 Copy 副本：`inquire::Select` 选择 `跟随包级` / `开启` / `关闭`，即时落盘 `app.registry.save(layout)`。
     - 若为 Symlink 副本：提示软链接跟随包级，提供选项：`切换所属仓库自动更新策略 (当前: 开/关)` 或 `返回`。
   - `4. 转换安装方式 (symlink ⇄ copy)`：
     - 从 Symlink 转 Copy：确认后执行。
     - 从 Copy 转 Symlink：提示将丢弃独立副本。若有本地修改且未 force，捕获 Mismatch 提示后询问是否确认丢弃修改强制转换。
   - `5. 删除该技能`：弹出 `inquire::Confirm` 确认，执行 `remove::remove_install` 并落盘，重新校准选中索引。
   - `0. 返回`：取消并返回 TUI。
3. 操作完毕后调用 `guard.resume()`，`term.clear()` 重新绘制 TUI 界面。

---

## Web API 与前端设计 (`src/web/`)

### 1. API 接口扩展 (`src/web/api.rs`)

#### a. 扩展 `POST /api/update`
- 接受可选 JSON 请求体：
  ```json
  {
    "skill": "skill-name",
    "target": { "scope": "project", "agent": "agents", "root": "/path" },
    "force": false
  }
  ```
- 若带 `skill` 与 `target`，走 `update_single`；若不带，保持全量更新。
- 遇本地改动冲突返回 HTTP 409。

#### b. 新增 `POST /api/switch-method`
- 接受 JSON 请求体：
  ```json
  {
    "skill": "skill-name",
    "target": { "scope": "project", "agent": "agents", "root": "/path" },
    "method": "copy", // 或 "symlink"
    "force": false
  }
  ```
- 调用 `install::switch_install_method`。
- Copy 转 Symlink 遇本地修改返回 HTTP 409，前端弹出二次确认后以 `force: true` 重试。

### 2. 前端界面与弹窗设计 (`src/web/static/index.html`)

#### a. 已安装列表表格（行内优化）
- 表格列保持：`技能 | 方式 | 目标 | 分类 | 自动更新 | 操作`
- **「自动更新」列**：
  - Copy 项：下拉框（跟随包级 / 开 / 关）。
  - Symlink 项：展示为 `跟随包级 (包:开)` 或 `跟随包级 (包:关)`，文字可点击直接切换所属包级更新策略。
- **「操作」列**：
  - 提供三个按钮：`[更新]`、`[管理]`、`[删除]`。

#### b. 集中管理弹窗 (Modal)
点击 `[管理]` 按钮弹出对话框，集中配置：
1. **技能与目标信息**：显示技能名称、所属仓库及目标路径。
2. **安装方式切换**：单选 `symlink` / `copy`。切换触发 `/api/switch-method`（409 时提示覆盖确认）。
3. **自动更新策略**：
   - Copy 副本：下拉选择 开 / 关 / 跟随包级。
   - Symlink 副本：显示所属仓库策略开关，支持直接勾选切换。
4. **更新操作**：提供「检查更新」与「强制更新」按钮。
5. **删除操作**：红色「删除此技能」按钮，弹出确认后删除并刷新。

---

## 测试策略

1. **单元测试 (Unit Tests)**：
   - `core::update`：单项更新测试、无更新跳过测试、单项本地修改 Mismatch 拦截与 force 覆盖测试。
   - `core::install`：symlink 转 copy 测试、copy 转 symlink 测试、copy 本地修改拦截与 force 覆盖测试、幂等切换测试。
2. **API 测试 (`tests/api_test.rs` / `src/web/api.rs tests`)**：
   - `test_update_single_success`
   - `test_update_single_mismatch_returns_409`
   - `test_switch_method_symlink_to_copy`
   - `test_switch_method_copy_to_symlink_mismatch_409`
3. **冒烟与端到端测试**：
   - TUI 渲染与事件分发测试。
   - 测试机真实命令行与交互场景验证。

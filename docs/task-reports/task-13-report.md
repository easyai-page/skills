# 任务 13 报告：Web 管理页（REST API + 内嵌前端）

- 提交：`e83ab4c feat(web): REST API + 内嵌单页管理界面`（基于 7eefbc2）
- 分支：feat/skills-manager
- 日期：2026-08-20

## 交付内容

### `src/web/mod.rs`（修改）
- `pub mod api;` + `run(layout, port, no_open)`：新建 tokio Runtime，block_on 内绑定 `127.0.0.1:{port}`，`open::that` 打开浏览器（`no_open` 跳过），`axum::serve` 起服务。io 错误经 `Error::Io`（`#[from]`）传播。
- 端口来源：CLI 层 `port.unwrap_or(cfg.web_port)`（`src/cli/commands.rs:26`），符合"端口来自 Config.web.port"。

### `src/web/api.rs`（新建，含内联测试）
- `AppState { layout, #[cfg(test)] tmp }`，状态经 `Arc<Mutex<AppState>>` 共享；所有 handler 先 `lock()` 再 load→改→save，registry/config 文件写天然串行化（简报指定方案）。
- 路由：
  - `GET /` → `include_str!("static/index.html")`
  - `GET /api/installs` / `GET /api/sources` → registry JSON
  - `POST /api/auto-update` → 包级（source 键）或副本级（skill+target）三态设置，404/400/500 区分
  - `POST /api/tags` → `core::tags::set_tags` + save
  - `POST /api/remove` → `core::remove::remove_install`（走 core 校验）+ save
  - `POST /api/update` → `core::update::build_plan(None)` + `execute_plan`，返回 `{done: [...]}`
- 业务全部走 core，web 仅做 HTTP 薄壳。

### `src/web/static/index.html`（新建）
- 无框架单页，与简报逐字一致：已安装/仓库缓存双视图、分类编辑、自动更新三态（copy）/包级勾选、删除确认、执行更新。

### `src/core/paths.rs`（1 行）
- `Layout` 补 `#[derive(Clone)]`（简报注明"任务 1 中补上"，实际缺失，此处补上）。

## 与简报的两处偏差（均为修复简报内在矛盾，语义不变）

1. **`AppState` 去掉 `#[derive(Clone)]`**：`tmp: tempfile::TempDir` 不可 Clone；且 axum state 实为 `Arc<Mutex<AppState>>`，外层 Arc 已满足 Clone，无需 AppState 自身 Clone。
2. **`tmp` 字段最终采用简报的 `Arc<tempfile::TempDir>`**，并在 `set_auto_update_writes_registry` 测试中加 `let keep = state.tmp.clone();`。原因（调试图证）：tower `oneshot` 消费并 drop router → 连带 drop AppState → TempDir 清理整个临时目录 → oneshot 之后从磁盘 reload 必得空 registry（实测 `STATUS=200` 但文件 NotFound）。测试必须自持一份 tempdir 句柄跨 oneshot 存活，否则简报测试无法通过。

## TDD 过程

1. 先写 3 个失败测试 → `cargo test web` 编译失败（E0425/E0433，符合预期）。
2. 实现 api.rs/mod.rs/index.html + Layout Clone。
3. `cargo test web`：3 passed。
4. 调试发现上述 oneshot/TempDir 问题并修复。
5. `cargo fmt` 后 `cargo test`：**91 unit + 10 integration，全部通过，0 失败**。

## 验证命令与结果

- `cargo test web` → 3 passed（list_installs_returns_json / set_auto_update_writes_registry / index_html_served_at_root）
- `cargo test` → 91 passed + 10 passed（cli_smoke），0 failed
- `cargo fmt` 已执行；`cargo build` 仅余 core 既有 warning（tags.rs unused import、cache.rs dead_code 等，非本任务引入，未动）

## 未触碰

- `docs/`、`.superpowers/` 未动；core 仅补 1 行 derive。

## 修复轮 1（审查 findings）

### Finding 1：index.html 未转义模板拼接 XSS/属性注入（Important）
- **修法**：放弃「escape 函数 + 拼字符串」路线，整体改为 DOM 构建——从根上消除注入面，无需枚举转义点。
  - 新增 `el(tag, text)`（textContent 注入）与 `cell(tr, child)` 助手；`renderInstalls`/`renderSources` 用 `document.createElement` 构建表格，容器用 `replaceChildren` 替换（原 `innerHTML` 赋值点 2 处全部移除）。
  - 全部内联事件属性（5 处数据相关 + 3 处 nav 静态按钮）改为 `addEventListener`，数据经闭包传递：tagInput change→`setTags`、select change→`setAU`、删除按钮 click→`rm`、checkbox change→`setSourceAU`、nav 三按钮改 id + 绑定。
  - 数据注入点逐一核对（自审清单）：
    - `i.skill` → textContent（行 57）；闭包传参（63/74/81）
    - `i.method` → textContent（58）
    - `i.target.kind/name/root` → 字符串拼接进 textContent（59，非 HTML 解析）；闭包→`targetBody`→`JSON.stringify` 请求体
    - `i.tags.join(',')` → `input.value` 属性赋值（62，无 HTML 解析）
    - `i.auto_update` → 仅布尔比较（71）
    - 源键 `k` → textContent（94）；闭包（99）
    - `s.commit` → textContent（95，加 `|| ''` 防空）；`s.auto_update` → `checked` 布尔（98）
  - 机械证明：`grep innerHTML|onclick|onchange|onerror|oninput` 仅命中注释行 1 处，无任何内联 handler 或 innerHTML 残留。
- 功能保持：视图/控件/成功路径行为与原版一致（三态 select 选中逻辑 `v === '' ? null : v === 'true'` 与原三元逐值等价）。

### Finding 2：run_update 吞错报“无更新”（Important）
- **api.rs**：签名改 `Result<Json<Value>, (StatusCode, String)>`。`Registry::load` 失败 → 500 `加载 registry 失败: {e}`；`execute_plan` 失败（git/网络/落盘/归属校验）→ 500 `执行更新失败: {e}`。成功路径不变，仍返回 `{done: [...]}`。
  - 保留的 `unwrap_or_default`：`Config::load`（本 finding 未涉及；且缺失配置文件时 `Config::load` 本就返回 `Ok(default)`）；`list_installs`/`list_sources` 两处（deferred minor，未动）。
- **index.html**：`api()` 助手非 2xx 时读取响应文本并 `throw new Error(body || 'HTTP '+status)`；`runUpdate()` 加 try/catch——成功保持 `alert(r.done...) + refresh()`，失败 `alert('更新失败: ' + e.message)`。其余调用点的 catch 缺失属 deferred minor，未动。

### 覆盖测试（api.rs 内联，tower oneshot）
- 新增 `run_update_returns_500_on_corrupted_registry`：人为写坏 registry.json，断言 500 且 body 含「加载 registry 失败」（证明不再 200 + 空 done）。
- 新增 `run_update_ok_with_empty_plan`：空计划成功路径断言 200 + `{done: []}`（钉住成功行为不变）。
- 既有 3 测试未改，全部通过。

### 验证命令与结果
- `cargo test web` → **5 passed; 0 failed**（含 2 个新测试）
- `cargo test` → **93 passed + 10 passed（cli_smoke），0 failed**（较修复前 91+10 净增 2）
- `cargo fmt` 已执行；警告核查：web 代码 0 警告，仅余 core 既有 5 条（tags.rs/cache.rs/error.rs/git.rs/paths.rs，非本轮引入）
- 前端无测试框架，转义正确性经逐注入点人工核对 + grep 机械证明（见上）

# Task 9 报告：Web 前端收藏页签 + 全仓收尾验证

分支：feat/skill-favorites　提交：a7238f3「feat: Web 收藏页面前端 + CLAUDE.md 子命令列表更新」

## 实现内容

### 1. src/web/static/index.html（+146 行）

brief 代码逐字落地，未自行改写：

- **结构**：nav 在「仓库缓存」后加 `<button id="nav-favorites">收藏</button>`；`<div id="sources">` 后加 `<div id="favorites">` 区块（source 输入 + 可选 skill 名输入 + 收藏按钮 + `fav-list` 容器）。
- **接线**：`show()` 的 div 列表加 `favorites`；`refresh()` 追加 `api('favorites')` 拉取并渲染；文件底部绑定 `nav-favorites` / `fav-add` 两个事件。
- **渲染与交互**（renderSources 之后追加）：
  - `renderFavorites(favs)`：两级渲染。source 标题行 = 粗体 key + meta（git 源显示 commit 前 7 位 + 收藏日期前 10 位；本地源显示「本地源」）。单技能仓库（`skills.length===1 && source_path==='.'`）二级留空，用途/安装/删除挂一级行；多技能仓库渲染技能表格（名称、用途、安装、删除）+ 标题行「删除整包」。
  - `addFav()`：POST /api/favorites，skill 非空时包成单元素数组，空 = 整仓；失败 alert 服务端错误文本。
  - `rmFav(source, skills)`：confirm 后 POST /api/favorites/remove。
  - `openInstall(source, skill)`：安装面板——global target 下拉（GET /api/targets 数据）+ 「project（绝对路径）」选项（选中时展开路径输入框）+ method 选择（默认/symlink/copy）+ 确认/取消。同时只允许一个面板实例。
  - `doInstall(...)`：POST /api/favorites/install，body `{source, skill, target:{kind,...}, overwrite:false}`，method 非「默认」才带上。409 走 runUpdate 同款确认链：confirm 后 `overwrite=true` 重试一次。

### 2. CLAUDE.md

常用命令一节的子命令列表补 `fav`：`add/list/remove/update/tag/auto-update/config/fav/tui/ui`。

## 契约核对（实现前逐项验证，全部一致）

- `TargetRec`  serde 形态 `{"kind":"global","name":...}` / `{"kind":"project","root":...}`（registry.rs:8-13，tag="kind" + lowercase）——与前端 doInstall 构造的 body 一致。
- `Method` lowercase 序列化（"symlink"/"copy"）——与 msel 的 value 一致。
- `Favorite` 字段：`url: Option<String>`（本地源为 null，前端 `f.url ?` 分支正确）、`commit`、`bookmarked_at`、`skills[].{name,description,source_path}`——与 renderFavorites 引用一致。
- 单技能仓库判定：core 侧 `source_path == Path::new(".")`（favorites.rs:123-126），前端 `=== '.'` 同语义。
- Task 8 五个端点签名（FavAddReq/FavRemoveReq/FavInstallReq/list_targets）与前端请求体逐项对齐。

## 验证结果

- `cargo fmt --check`：无输出（fmt 未改动任何文件）。
- `cargo clippy --all-targets`：零警告。
- `cargo test`：全绿——127 单元 + 13 cli_smoke + 7 e2e = 147 passed / 0 failed。
- Web curl 冒烟（`SKILLS_HOME` 指向临时目录隔离，未触碰真实 `~/.skills`）：
  1. `GET /` 含「收藏」（5 处匹配：nav 按钮、输入框 placeholder、确认文案等）✓
  2. `POST /api/favorites`（本地双技能目录整仓）→ `{"key":"local/mysrc","skills":2}` ✓
  3. `GET /api/favorites` → 两级 JSON，字段名与前端引用一致 ✓
  4. `GET /api/targets` → `[{name,path}]` 三项内置 target ✓
  5. install 首次 200（文件落盘 `<root>/.agents/skills/alpha/SKILL.md`）→ 重复 install 409 + 可读消息 → `overwrite=true` 重试 200 ✓（409 确认链全通）
  6. `POST /api/favorites/remove` 删整包 200，favorites 清空 ✓
  冒烟临时目录已删除，服务器已停止。
- spec 对照（2026-08-25-skill-favorites-design.md「Web」节 92-100 行）：nav 按钮、收藏输入框、两级列表（标题行含收藏时间/删除整包；技能行含用途/安装/删除）、安装对话框构成、409 确认链、XSS 约定——全部落地；范围外条目（收藏 tag、自动刷新、Web 端 add 等价物）未偷跑。
- XSS 约定自查：全部用户/registry 数据经 `el()`（textContent）或 `createTextNode` 注入，事件全部 addEventListener + 闭包传值，无 innerHTML 拼接、无内联事件属性。

## 与 brief 的偏差

无设计语义偏差，仅执行方式一处：brief Step 5 的手动冒烟命令直接用默认 SKILLS_HOME，会污染真实 `~/.skills` 数据；实际执行时改用 `SKILLS_HOME=<临时目录>` 隔离（与项目集成测试的隔离约定一致），验证项与预期结果不变。

## 留给审查者的疑虑

1. **单技能判定的跨层字符串约定**：前端硬编码 `source_path === '.'`，依赖 serde 把 `PathBuf(".")` 序列化为 `"."`。若 core 侧未来改变表示（如 `"./"`），前端会静默退化为两级渲染。core 有 `is_single_skill_repo` 同语义函数与 e2e（favorites_single_skill_repo_display）锁定，但跨层约定本身无编译期保障。
2. **project 路径无前端本地校验**：安装面板选 project 但路径留空时仍会发请求（`root:""`），由后端报错弹出。与 brief 逐字一致，未自行加校验；体验上可接受（错误消息可见）。
3. **refresh() 串行拉三端点**：每次切页签都全量拉 installs/sources/favorites，任一端点失败会导致排在其后的视图不刷新。这是既有模式（本次只是顺延加第三个），规模下无实际问题，未改动。
4. **安装成功后面板不自动关闭**：仅 alert 提示，面板保留供连续安装，用户点「取消」关闭。与 brief 一致。
5. **index.html 无自动化测试**：遵循项目现状（前端无测试框架），靠 api.rs 的 tower oneshot 测试（含 409 链）+ curl 冒烟 + 人工审查覆盖。

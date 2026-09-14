# 已安装技能管理增强实施计划：删除、单项手动更新、更新模式与方式转换

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** 为 skills 管理器的 TUI 与 Web 前端补齐已安装技能的删除、单项手动与强制更新、自动更新策略配置、以及 symlink 与 copy 安装方式原子互转功能。

**Architecture:** 在 Core 层扩展单项更新 `update::update_single` 和安装方式切换 `install::switch_install_method`，严格遵循 6 项安全不变量；Web 端扩展 API 端点并在前端增加行内快捷操作与集中管理 Modal；TUI 端通过 `Enter` 动作管理菜单（Action Menu）与 `d` 删除快捷键提供交互。

**Tech Stack:** Rust (edition 2024), ratatui, crossterm, inquire, axum, vanilla JS/HTML, assert_cmd/tempfile.

**Spec:** `docs/superpowers/specs/2026-09-14-installed-skills-management-design.md`

## Global Constraints

- 代码注释、commit message、文档一律中文；代码与命名用英文。
- 修改 core/install/update/remove 时不得破坏原子性、归属核验（`.skills-manifest`）、本地修改保护（`Mismatch` 拦截）、磁盘与 registry 一致性。
- Web 前端必须保持 XSS 防御：用户与外部数据严禁 `innerHTML` 拼接，一律经 `textContent`/`value` 赋值与 `addEventListener` 绑定。
- commit message 采用中文，简洁描述变更意图；严禁自动 `git push`。

---

### Task 1: Core 单项手动更新 (`core::update::update_single`)

**Files:**
- Modify: `src/core/update.rs`
- Test: `src/core/update.rs` (内嵌单元测试)

**Interfaces:**
- Produces:
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

- [ ] **Step 1: Write the failing test**

在 `src/core/update.rs` 的 `mod tests` 底部添加单项更新的单元测试：
```rust
#[test]
fn update_single_updates_specified_copy_and_saves() {
    let (_t, layout, cfg, mut reg) = setup_installed(Method::Copy);
    let dest = cfg.targets["agents"].join("alpha");
    // 演进缓存与源
    std::fs::write(
        layout.cache_dir("github/o/r").join("skills/alpha/SKILL.md"),
        "---\nname: alpha\ndescription: A\n---\nv2\n",
    )
    .unwrap();
    reg.sources.get_mut("github/o/r").unwrap().commit = "c2".into();

    let target = TargetRec::Global {
        agent: "agents".into(),
    };
    let done = update_single(&layout, &cfg, &mut reg, "alpha", &target, false).unwrap();
    assert!(done.iter().any(|l| l.contains("副本 alpha")));
    assert!(std::fs::read_to_string(dest.join("SKILL.md")).unwrap().contains("v2"));
    assert_eq!(reg.installs[0].commit, "c2");
}

#[test]
fn update_single_fails_on_local_modification_without_force() {
    let (_t, layout, cfg, mut reg) = setup_installed(Method::Copy);
    let dest = cfg.targets["agents"].join("alpha");
    // 本地修改文件
    std::fs::write(dest.join("SKILL.md"), "tampered").unwrap();
    reg.sources.get_mut("github/o/r").unwrap().commit = "c2".into();

    let target = TargetRec::Global {
        agent: "agents".into(),
    };
    let err = update_single(&layout, &cfg, &mut reg, "alpha", &target, false).unwrap_err();
    match err {
        Error::Mismatch(msg) => assert!(msg.contains("本地修改")),
        other => panic!("expected Mismatch, got {other:?}"),
    }

    // force=true 时覆盖成功
    let done = update_single(&layout, &cfg, &mut reg, "alpha", &target, true).unwrap();
    assert!(done.iter().any(|l| l.contains("副本 alpha")));
}
```

- [ ] **Step 2: Run test to verify it fails**

Run: `cargo test core::update::tests::update_single -q`
Expected: FAIL (update_single not found)

- [ ] **Step 3: Implement `update_single` in `src/core/update.rs`**

在 `src/core/update.rs` 中实现：
```rust
/// 对指定已安装项执行单项手动更新（无视自动更新开关强制更新）。
/// force=false 时若本地副本有修改返回 Error::Mismatch；force=true 则覆盖更新。
/// 成功后原子落盘保存 registry，返回更新摘要行。
pub fn update_single(
    layout: &Layout,
    cfg: &crate::core::config::Config,
    reg: &mut Registry,
    skill: &str,
    target: &TargetRec,
    force: bool,
) -> Result<Vec<String>> {
    let sel = Selection {
        skill: skill.to_string(),
        target: target.clone(),
    };
    let plan = build_plan(reg, Some(&sel));
    if let Some(missing) = plan.missing {
        return Err(Error::NotInstalled(missing));
    }
    let done = execute_plan(layout, cfg, reg, &plan, force)?;
    reg.save(layout)?;
    Ok(done)
}
```

- [ ] **Step 4: Run test to verify it passes**

Run: `cargo test core::update::tests::update_single -q`
Expected: PASS (2 tests pass)

- [ ] **Step 5: Commit**

```bash
git add src/core/update.rs
git commit -m "feat(core): 新增 update_single 单项手动更新函数与本地修改拦截测试"
```

---

### Task 2: Core 安装方式原子互转 (`core::install::switch_install_method`)

**Files:**
- Modify: `src/core/install.rs`
- Test: `src/core/install.rs` (内嵌单元测试)

**Interfaces:**
- Produces:
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

- [ ] **Step 1: Write the failing tests**

在 `src/core/install.rs` 的 `mod tests` 底部添加安装方式切换测试：
```rust
#[test]
fn switch_install_method_symlink_to_copy() {
    let (tmp, layout, cfg, mut reg) = setup();
    let project_root = tmp.path().join("project");
    let target = Target {
        agent: "agents".into(),
        scope: Scope::Project(project_root.clone()),
    };
    // 先装 symlink
    install_skill(
        &layout, &cfg, &mut reg, "github/o/r", "alpha", "skills/alpha", &target, Method::Symlink, "c1",
    ).unwrap();
    let trec = to_rec(&target);
    assert_eq!(reg.find("alpha", &trec).unwrap().method, Method::Symlink);

    // 切换到 Copy
    switch_install_method(&layout, &cfg, &mut reg, "alpha", &trec, Method::Copy, false).unwrap();
    let inst = reg.find("alpha", &trec).unwrap();
    assert_eq!(inst.method, Method::Copy);
    let dest = project_root.join(".agents/skills/alpha");
    assert!(dest.join(COPY_MANIFEST).is_file());
    assert!(!dest.symlink_metadata().unwrap().file_type().is_symlink());
}

#[test]
fn switch_install_method_copy_to_symlink_checks_mismatch() {
    let (tmp, layout, cfg, mut reg) = setup();
    let project_root = tmp.path().join("project");
    let target = Target {
        agent: "agents".into(),
        scope: Scope::Project(project_root.clone()),
    };
    // 先装 Copy
    install_skill(
        &layout, &cfg, &mut reg, "github/o/r", "alpha", "skills/alpha", &target, Method::Copy, "c1",
    ).unwrap();
    let trec = to_rec(&target);
    let dest = project_root.join(".agents/skills/alpha");
    // 改动文件
    std::fs::write(dest.join("SKILL.md"), "modified").unwrap();

    // 未 force 切换拒绝
    let err = switch_install_method(&layout, &cfg, &mut reg, "alpha", &trec, Method::Symlink, false).unwrap_err();
    assert!(matches!(err, Error::Mismatch(_)));

    // force 切换成功
    switch_install_method(&layout, &cfg, &mut reg, "alpha", &trec, Method::Symlink, true).unwrap();
    let inst = reg.find("alpha", &trec).unwrap();
    assert_eq!(inst.method, Method::Symlink);
    assert!(dest.symlink_metadata().unwrap().file_type().is_symlink());
}
```

- [ ] **Step 2: Run test to verify it fails**

Run: `cargo test core::install::tests::switch_install_method -q`
Expected: FAIL (switch_install_method not found)

- [ ] **Step 3: Implement `switch_install_method` in `src/core/install.rs`**

在 `src/core/install.rs` 中实现：
```rust
/// 将已安装技能在 Symlink 和 Copy 之间原子切换。
/// Copy -> Symlink 时：若存在本地修改且 !force 则返回 Error::Mismatch 拒绝转换。
/// 转换成功后更新 registry 记录并原子落盘。
pub fn switch_install_method(
    layout: &Layout,
    cfg: &crate::core::config::Config,
    reg: &mut Registry,
    skill: &str,
    target: &TargetRec,
    new_method: Method,
    force: bool,
) -> Result<()> {
    let (inst_source, inst_source_path, inst_commit, cur_method) = {
        let inst = reg.find(skill, target).ok_or_else(|| Error::NotInstalled(skill.to_string()))?;
        if inst.method == new_method {
            return Ok(());
        }
        (inst.source.clone(), inst.source_path.clone(), inst.commit.clone(), inst.method)
    };

    let t = target.to_target();
    let dest_dir = paths::install_dir(layout, cfg, &t)?;
    let dest = dest_dir.join(skill);

    match (cur_method, new_method) {
        (Method::Copy, Method::Symlink) => {
            // 归属与修改保护
            if dest.exists() {
                if !force {
                    check_copy_unmodified(&dest)?;
                }
                remove_install_path(&dest)?;
            }
            // 建立 symlink
            let src_dir = safe_source_path(layout, &inst_source, &inst_source_path.to_string_lossy())?;
            symlink_dir(&src_dir, &dest)?;
        }
        (Method::Symlink, Method::Copy) => {
            // 移除旧 symlink
            if dest.exists() || dest.symlink_metadata().is_ok() {
                remove_install_path(&dest)?;
            }
            // 走 staging + rename 建立带 manifest 的 copy 副本
            let src_dir = safe_source_path(layout, &inst_source, &inst_source_path.to_string_lossy())?;
            copy_install(&src_dir, &dest_dir, &dest)?;
        }
        _ => unreachable!(),
    }

    // 更新 registry
    if let Some(inst) = reg.find_mut(skill, target) {
        inst.method = new_method;
        if new_method == Method::Symlink {
            inst.auto_update = None; // 软连接始终跟随包级
        }
    }
    reg.save(layout)?;
    Ok(())
}
```

- [ ] **Step 4: Run test to verify it passes**

Run: `cargo test core::install::tests::switch_install_method -q`
Expected: PASS (2 tests pass)

- [ ] **Step 5: Commit**

```bash
git add src/core/install.rs
git commit -m "feat(core): 实现 switch_install_method 安装方式原子互转与修改保护"
```

---

### Task 3: Web API 扩展：单项更新与方式转换端点

**Files:**
- Modify: `src/web/api.rs`
- Test: `src/web/api.rs` (内嵌测试)

**Interfaces:**
- Consumes:
  - `core::update::update_single`
  - `core::install::switch_install_method`
- Produces:
  - `POST /api/update`: 接受可选 JSON `{ skill, target, force }`
  - `POST /api/switch-method`: 接受 JSON `{ skill, target, method, force }`

- [ ] **Step 1: Write the failing tests**

在 `src/web/api.rs` 的测试模块中增加针对单项更新与方式转换的 HTTP API 测试：
```rust
#[tokio::test]
async fn update_single_via_api() {
    let tmp = tempfile::tempdir().unwrap();
    let layout = Layout::new(tmp.path().join("skills"));
    let mut reg = Registry::load(&layout).unwrap_or_default();
    // 写入一个假 copy 安装与源
    let s_dir = layout.cache_dir("github/o/r").join("skills/s1");
    std::fs::create_dir_all(&s_dir).unwrap();
    std::fs::write(s_dir.join("SKILL.md"), "---\nname: s1\ndescription: D\n---\n").unwrap();
    reg.sources.insert("github/o/r".into(), crate::core::registry::SourceRecord {
        url: "url".into(), commit: "c1".into(), fetched_at: "t".into(), auto_update: None,
    });
    let target = TargetRec::Global { agent: "agents".into() };
    reg.installs.push(crate::core::registry::Install {
        skill: "s1".into(), source: "github/o/r".into(), source_path: "skills/s1".into(),
        target: target.clone(), method: Method::Copy, commit: "c1".into(),
        auto_update: None, installed_at: "t".into(),
    });
    reg.save(&layout).unwrap();

    let state = AppState { layout, tmp: std::sync::Arc::new(tmp) };
    let app = router(state);

    let req = axum::http::Request::builder()
        .method("POST")
        .uri("/api/update")
        .header("content-type", "application/json")
        .body(axum::body::Body::from(serde_json::to_string(&serde_json::json!({
            "skill": "s1",
            "target": target,
            "force": false
        })).unwrap()))
        .unwrap();

    let res = tower::ServiceExt::oneshot(app, req).await.unwrap();
    assert_eq!(res.status(), StatusCode::OK);
}

#[tokio::test]
async fn switch_method_via_api() {
    let tmp = tempfile::tempdir().unwrap();
    let layout = Layout::new(tmp.path().join("skills"));
    let mut reg = Registry::load(&layout).unwrap_or_default();
    let s_dir = layout.cache_dir("github/o/r").join("skills/s1");
    std::fs::create_dir_all(&s_dir).unwrap();
    std::fs::write(s_dir.join("SKILL.md"), "---\nname: s1\ndescription: D\n---\n").unwrap();
    reg.sources.insert("github/o/r".into(), crate::core::registry::SourceRecord {
        url: "url".into(), commit: "c1".into(), fetched_at: "t".into(), auto_update: None,
    });
    let cfg = crate::core::config::Config::default();
    let target = crate::core::paths::Target { agent: "agents".into(), scope: crate::core::paths::Scope::Global };
    let rec = crate::core::install::install_skill(
        &layout, &cfg, &mut reg, "github/o/r", "s1", "skills/s1", &target, Method::Symlink, "c1",
    ).unwrap();
    reg.save(&layout).unwrap();

    let state = AppState { layout: layout.clone(), tmp: std::sync::Arc::new(tmp) };
    let app = router(state);

    let req = axum::http::Request::builder()
        .method("POST")
        .uri("/api/switch-method")
        .header("content-type", "application/json")
        .body(axum::body::Body::from(serde_json::to_string(&serde_json::json!({
            "skill": "s1",
            "target": rec.target,
            "method": "copy",
            "force": false
        })).unwrap()))
        .unwrap();

    let res = tower::ServiceExt::oneshot(app, req).await.unwrap();
    assert_eq!(res.status(), StatusCode::OK);

    let reg = Registry::load(&layout).unwrap();
    assert_eq!(reg.installs[0].method, Method::Copy);
}
```

- [ ] **Step 2: Run test to verify it fails**

Run: `cargo test web::api::tests::switch_method_via_api -q`
Expected: FAIL (/api/switch-method not found or 404)

- [ ] **Step 3: Implement handler and router updates in `src/web/api.rs`**

1. 修改路由注册：
```rust
pub fn router(state: AppState) -> Router {
    Router::new()
        .route("/", get(index))
        .route("/api/installs", get(list_installs))
        .route("/api/scan", post(scan_source))
        .route("/api/install", post(install_from_source))
        .route("/api/sources", get(list_sources))
        .route("/api/auto-update", post(set_auto_update))
        .route("/api/tags", post(set_tags))
        .route("/api/remove", post(remove_install))
        .route("/api/update", post(run_update))
        .route("/api/switch-method", post(switch_method))
        .route("/api/favorites", get(list_favorites).post(add_favorite))
        .route("/api/favorites/remove", post(remove_favorite))
        .route("/api/favorites/install", post(install_favorite))
        .route("/api/agents", get(list_agents))
        .with_state(Arc::new(Mutex::new(state)))
}
```

2. 升级 `UpdateQuery` 和 `run_update`，支持可选 JSON body 或 query：
```rust
#[derive(serde::Deserialize, Default)]
struct UpdateReq {
    skill: Option<String>,
    target: Option<TargetRec>,
    force: Option<bool>,
}

async fn run_update(
    State(s): S,
    axum::extract::Query(query): axum::extract::Query<UpdateQuery>,
    body: Option<Json<UpdateReq>>,
) -> Result<Json<serde_json::Value>, (StatusCode, String)> {
    let s = s.lock().unwrap();
    let cfg = crate::core::config::Config::load(&s.layout).map_err(|e| {
        (StatusCode::INTERNAL_SERVER_ERROR, format!("加载 config 失败: {e}"))
    })?;
    let mut reg = Registry::load(&s.layout).map_err(|e| {
        (StatusCode::INTERNAL_SERVER_ERROR, format!("加载 registry 失败: {e}"))
    })?;

    let (skill, target, force) = if let Some(Json(b)) = body {
        (b.skill, b.target, b.force.unwrap_or_else(|| query.force.unwrap_or(false)))
    } else {
        (None, None, query.force.unwrap_or(false))
    };

    if let (Some(skill), Some(target)) = (skill, target) {
        let done = crate::core::update::update_single(&s.layout, &cfg, &mut reg, &skill, &target, force)
            .map_err(|e| match e {
                crate::core::error::Error::Mismatch(msg) => (StatusCode::CONFLICT, msg),
                e => (StatusCode::INTERNAL_SERVER_ERROR, format!("执行单项更新失败: {e}")),
            })?;
        return Ok(Json(serde_json::json!({ "done": done })));
    }

    let plan = crate::core::update::build_plan(&reg, None);
    let done = crate::core::update::execute_plan(&s.layout, &cfg, &mut reg, &plan, force).map_err(
        |e| match e {
            crate::core::error::Error::Mismatch(msg) => (StatusCode::CONFLICT, msg),
            e => (StatusCode::INTERNAL_SERVER_ERROR, format!("执行更新失败: {e}")),
        },
    )?;
    Ok(Json(serde_json::json!({ "done": done })))
}
```

3. 实现 `switch_method` 处理函数：
```rust
#[derive(serde::Deserialize)]
struct SwitchMethodReq {
    skill: String,
    target: TargetRec,
    method: Method,
    force: Option<bool>,
}

async fn switch_method(
    State(s): S,
    Json(req): Json<SwitchMethodReq>,
) -> Result<StatusCode, (StatusCode, String)> {
    let s = s.lock().unwrap();
    let cfg = crate::core::config::Config::load(&s.layout).map_err(|e| {
        (StatusCode::INTERNAL_SERVER_ERROR, format!("加载 config 失败: {e}"))
    })?;
    let mut reg = Registry::load(&s.layout).map_err(|e| {
        (StatusCode::INTERNAL_SERVER_ERROR, format!("加载 registry 失败: {e}"))
    })?;
    let force = req.force.unwrap_or(false);

    crate::core::install::switch_install_method(
        &s.layout, &cfg, &mut reg, &req.skill, &req.target, req.method, force,
    )
    .map_err(|e| match e {
        crate::core::error::Error::Mismatch(msg) => (StatusCode::CONFLICT, msg),
        crate::core::error::Error::NotInstalled(msg) => (StatusCode::NOT_FOUND, msg),
        e => (StatusCode::INTERNAL_SERVER_ERROR, format!("切换安装方式失败: {e}")),
    })?;

    Ok(StatusCode::OK)
}
```

- [ ] **Step 4: Run test to verify it passes**

Run: `cargo test web::api::tests -q`
Expected: PASS (all web tests pass)

- [ ] **Step 5: Commit**

```bash
git add src/web/api.rs
git commit -m "feat(web): /api/update 支持单项更新，新增 /api/switch-method 端点"
```

---

### Task 4: Web 前端 UI：行内操作扩展与集中管理弹窗

**Files:**
- Modify: `src/web/static/index.html`

**Interfaces:**
- Consumes:
  - `POST /api/update` (带 JSON body)
  - `POST /api/switch-method`
  - `POST /api/auto-update`
  - `POST /api/remove`

- [ ] **Step 1: Inspect index.html structure and add modal styles**

在 `src/web/static/index.html` 的 `<style>` 中补充轻量居中弹窗样式：
```css
.modal-mask { position: fixed; inset: 0; background: rgba(0,0,0,0.5); display: flex; align-items: center; justify-content: center; z-index: 100; }
.modal-box { background: white; padding: 20px; border-radius: 8px; min-width: 360px; max-width: 500px; box-shadow: 0 4px 12px rgba(0,0,0,0.15); }
.modal-box h3 { margin-top: 0; }
.modal-actions { margin-top: 16px; text-align: right; }
.modal-actions button { margin-left: 8px; }
.btn-danger { color: #d32f2f; }
.clickable-badge { cursor: pointer; text-decoration: underline; }
```

- [ ] **Step 2: Update `renderInstalls` and add single update & manage button**

修改 `renderInstalls`:
1. **自动更新列**：
   - `copy` 副本保持下拉框；
   - `symlink` 副本显示 `跟随包级 (包: ${srcAU})`，点击文本即可弹出是否切换该仓库包级 auto_update 的提示并调用 `setSourceAU`。
2. **操作列**：
   - 增加 `[更新]` 按钮：调用 `updateSingle(i)`。
   - 增加 `[管理]` 按钮：调用 `openManageModal(i, sources)`。
   - `[删除]` 按钮保持。

- [ ] **Step 3: Implement `updateSingle`, `switchMethod`, and `openManageModal`**

在 `<script>` 中添加：
```javascript
async function updateSingle(i, force = false) {
  const body = { skill: i.skill, target: i.target, force };
  try {
    const r = await api('update', {
      method: 'POST',
      headers: { 'content-type': 'application/json' },
      body: JSON.stringify(body)
    });
    alert(r.done.join('\n') || `${i.skill} 已是最新`);
    refresh();
  } catch (e) {
    if (e.status === 409) {
      if (confirm(e.message + '\n\n本地修改将被覆盖，是否强制更新？')) {
        return updateSingle(i, true);
      }
      return;
    }
    alert('更新失败: ' + e.message);
  }
}

async function doSwitchMethod(skill, target, method, force = false) {
  try {
    await api('switch-method', {
      method: 'POST',
      headers: { 'content-type': 'application/json' },
      body: JSON.stringify({ skill, target, method, force })
    });
    alert(`已转换为 ${method}`);
    refresh();
  } catch (e) {
    if (e.status === 409) {
      if (confirm(e.message + '\n\n本地修改将被丢弃，是否强制转换为软链接？')) {
        return doSwitchMethod(skill, target, method, true);
      }
      return;
    }
    alert('转换安装方式失败: ' + e.message);
  }
}

function openManageModal(i, sources) {
  const existing = document.getElementById('manage-modal');
  if (existing) existing.remove();

  const mask = el('div');
  mask.id = 'manage-modal';
  mask.className = 'modal-mask';

  const box = el('div');
  box.className = 'modal-box';
  box.appendChild(el('h3', `管理技能: ${i.skill}`));

  const targetStr = i.target.scope === 'global' ? `global:${i.target.agent}` : `project:${i.target.root}`;
  box.appendChild(el('p', `目标: ${targetStr}`));

  // 1. 安装方式
  const methodP = el('p');
  methodP.appendChild(el('strong', '安装方式: '));
  for (const m of ['symlink', 'copy']) {
    const label = el('label');
    label.style.marginRight = '12px';
    const radio = document.createElement('input');
    radio.type = 'radio';
    radio.name = 'manage-method';
    radio.value = m;
    radio.checked = i.method === m;
    radio.addEventListener('change', () => {
      if (confirm(`确认将 ${i.skill} 转换为 ${m}？`)) {
        mask.remove();
        doSwitchMethod(i.skill, i.target, m, false);
      } else {
        radio.checked = i.method === m;
      }
    });
    label.appendChild(radio);
    label.appendChild(document.createTextNode(' ' + m));
    methodP.appendChild(label);
  }
  box.appendChild(methodP);

  // 2. 自动更新策略
  const auP = el('p');
  auP.appendChild(el('strong', '自动更新策略: '));
  if (i.method === 'copy') {
    const sel = document.createElement('select');
    for (const [v, label] of [['', '跟随包级'], ['true', '开'], ['false', '关']]) {
      const opt = el('option', label);
      opt.value = v;
      opt.selected = v === '' ? i.auto_update === null : i.auto_update === (v === 'true');
      sel.appendChild(opt);
    }
    sel.addEventListener('change', async () => {
      await setAU(i.skill, i.target.scope, targetRef(i), i.target.agent, sel.value);
      alert('已更新策略');
      mask.remove();
      refresh();
    });
    auP.appendChild(sel);
  } else {
    const srcRecord = sources[i.source] || {};
    const srcState = srcRecord.auto_update ? '开' : '关';
    auP.appendChild(document.createTextNode(`软连接跟随所属仓库 [${i.source}] (当前: ${srcState}) `));
    const toggleBtn = el('button', `切换仓库策略为 ${srcRecord.auto_update ? '关' : '开'}`);
    toggleBtn.addEventListener('click', async () => {
      await setSourceAU(i.source, !srcRecord.auto_update);
      alert('已修改所属仓库更新策略');
      mask.remove();
      refresh();
    });
    auP.appendChild(toggleBtn);
  }
  box.appendChild(auP);

  // 3. 动作按钮
  const btnRow = el('div');
  btnRow.style.marginTop = '16px';
  const updBtn = el('button', '检查更新');
  updBtn.addEventListener('click', () => { mask.remove(); updateSingle(i, false); });
  btnRow.appendChild(updBtn);

  const forceUpdBtn = el('button', '强制覆盖更新');
  forceUpdBtn.style.marginLeft = '8px';
  forceUpdBtn.addEventListener('click', () => {
    if (confirm('确认强制覆盖更新（可能覆盖本地修改）？')) {
      mask.remove();
      updateSingle(i, true);
    }
  });
  btnRow.appendChild(forceUpdBtn);

  const delBtn = el('button', '删除此技能');
  delBtn.className = 'btn-danger';
  delBtn.style.marginLeft = '8px';
  delBtn.addEventListener('click', () => {
    mask.remove();
    rm(i.skill, i.target.scope, targetRef(i), i.target.agent);
  });
  btnRow.appendChild(delBtn);
  box.appendChild(btnRow);

  // 底部关闭
  const actions = el('div');
  actions.className = 'modal-actions';
  const closeBtn = el('button', '关闭');
  closeBtn.addEventListener('click', () => mask.remove());
  actions.appendChild(closeBtn);
  box.appendChild(actions);

  mask.appendChild(box);
  document.body.appendChild(mask);
}
```

- [ ] **Step 4: Verify Web UI tests pass**

Run: `cargo test web -q`
Expected: PASS (Web unit/api tests pass)

- [ ] **Step 5: Commit**

```bash
git add src/web/static/index.html
git commit -m "feat(web): 前端已安装列表补齐单项更新快捷按钮与集中管理 Modal"
```

---

### Task 5: TUI 动作菜单与删除快捷键集成

**Files:**
- Modify: `src/tui/ui.rs`
- Modify: `src/tui/mod.rs`
- Modify: `src/tui/app.rs`
- Test: `src/tui/app.rs` & `src/tui/ui.rs`

**Interfaces:**
- Consumes:
  - `core::update::update_single`
  - `core::install::switch_install_method`
  - `core::remove::remove_install`

- [ ] **Step 1: Update UI hints in `src/tui/ui.rs`**

修改 `src/tui/ui.rs` 中 Installed 视图表格标题提示：
```rust
Block::default()
    .borders(Borders::ALL)
    .title(if app.show_all {
        "已安装 [全部项目] (Enter 管理/操作，d 删除，a 切换更新，p 切换过滤)"
    } else {
        "已安装 [本项目+全局] (Enter 管理/操作，d 删除，a 切换更新，p 切换过滤)"
    }),
```

- [ ] **Step 2: Add `Action::DeleteInstalled` and hook `Enter` in `src/tui/app.rs` & `src/tui/mod.rs`**

在 `src/tui/mod.rs` 的 `event_loop` 中截获在 `View::Installed` 时的 `Enter` 与 `d` 键：
```rust
if app.view == View::Installed {
    if k.code == KeyCode::Enter {
        guard.suspend();
        let r = installed_action_menu(layout, cfg, app);
        if let Err(e) = r {
            eprintln!("操作异常: {e}");
            let _ = inquire::Text::new("按回车返回 TUI").prompt();
        }
        guard.resume()?;
        term.clear()?;
        continue;
    }
    if k.code == KeyCode::Char('d') {
        guard.suspend();
        let r = confirm_delete_installed(layout, cfg, app);
        if let Err(e) = r {
            eprintln!("删除失败: {e}");
            let _ = inquire::Text::new("按回车返回 TUI").prompt();
        }
        guard.resume()?;
        term.clear()?;
        continue;
    }
}
```

- [ ] **Step 3: Implement `confirm_delete_installed` and `installed_action_menu` in `src/tui/mod.rs`**

在 `src/tui/mod.rs` 中实现：
```rust
fn confirm_delete_installed(layout: &Layout, cfg: &Config, app: &mut AppState) -> Result<()> {
    let visible = app.visible_rows();
    let Some(row) = visible.get(app.selected) else {
        return Ok(());
    };
    let skill = row.skill.clone();
    let target = row.target.clone();
    let prompt_text = format!("确认删除 {} @ {}？", skill, target.to_target());
    if !inquire::Confirm::new(&prompt_text).with_default(false).prompt().unwrap_or(false) {
        return Ok(());
    }

    match crate::core::remove::remove_install(layout, cfg, &mut app.registry, &skill, &target) {
        Ok(_) => {
            app.registry.save(layout)?;
            println!("已删除 {skill} @ {}", target.to_target());
            let max = app.visible_rows().len().saturating_sub(1);
            app.selected = app.selected.min(max);
        }
        Err(e) => {
            eprintln!("删除失败: {e}");
        }
    }
    let _ = inquire::Text::new("按回车返回 TUI").prompt();
    Ok(())
}

fn installed_action_menu(layout: &Layout, cfg: &Config, app: &mut AppState) -> Result<()> {
    let visible = app.visible_rows();
    let Some(row) = visible.get(app.selected) else {
        return Ok(());
    };
    let skill = row.skill.clone();
    let target = row.target.clone();
    let method = row.method;
    let auto_up = row.auto_update;
    let source = row.source.clone();

    println!("\n▶ 管理技能: {} @ {}", skill, target.to_target());
    println!(
        "  当前方式: {:?} | 自动更新: {} | 来源: {}",
        method,
        match auto_up {
            Some(true) => "开",
            Some(false) => "关",
            None => "跟随包级",
        },
        source
    );

    let options = vec![
        "1. 手动更新 (普通检查更新)",
        "2. 强制覆盖更新 (忽略本地修改)",
        "3. 配置自动更新策略",
        "4. 转换安装方式 (symlink ⇄ copy)",
        "5. 删除该技能副本",
        "0. 返回",
    ];

    let choice = inquire::Select::new("选择操作", options).prompt().unwrap_or("0. 返回");

    match choice {
        s if s.starts_with("1.") => {
            match crate::core::update::update_single(layout, cfg, &mut app.registry, &skill, &target, false) {
                Ok(done) => {
                    if done.is_empty() {
                        println!("当前已是最新版本");
                    } else {
                        for l in done { println!("{l}"); }
                    }
                }
                Err(Error::Mismatch(msg)) => {
                    eprintln!("检测到本地修改:\n{msg}");
                    if inquire::Confirm::new("是否放弃本地修改强制覆盖？").with_default(false).prompt().unwrap_or(false) {
                        let done = crate::core::update::update_single(layout, cfg, &mut app.registry, &skill, &target, true)?;
                        for l in done { println!("{l}"); }
                    }
                }
                Err(e) => eprintln!("更新失败: {e}"),
            }
            let _ = inquire::Text::new("按回车返回 TUI").prompt();
        }
        s if s.starts_with("2.") => {
            if inquire::Confirm::new("确认强制更新并覆盖可能的本地修改？").with_default(false).prompt().unwrap_or(false) {
                match crate::core::update::update_single(layout, cfg, &mut app.registry, &skill, &target, true) {
                    Ok(done) => {
                        for l in done { println!("{l}"); }
                    }
                    Err(e) => eprintln!("强制更新失败: {e}"),
                }
                let _ = inquire::Text::new("按回车返回 TUI").prompt();
            }
        }
        s if s.starts_with("3.") => {
            if method == Method::Copy {
                let policy_opts = vec!["跟随包级", "开启", "关闭"];
                if let Ok(p) = inquire::Select::new("选择该副本的自动更新策略", policy_opts).prompt() {
                    let val = match p {
                        "开启" => Some(true),
                        "关闭" => Some(false),
                        _ => None,
                    };
                    if let Some(inst) = app.registry.find_mut(&skill, &target) {
                        inst.auto_update = val;
                    }
                    app.registry.save(layout)?;
                    println!("已保存自动更新策略");
                }
            } else {
                let src_au = app.registry.sources.get(&source).and_then(|s| s.auto_update).unwrap_or(false);
                println!("提示：软链接始终跟随所属仓库 [{source}]（当前包级: {}）", if src_au { "开" } else { "关" });
                if inquire::Confirm::new(&format!("是否切换所属仓库 [{source}] 的自动更新开关？")).prompt().unwrap_or(false) {
                    if let Some(s) = app.registry.sources.get_mut(&source) {
                        s.auto_update = Some(!src_au);
                    }
                    app.registry.save(layout)?;
                    println!("已更新仓库自动更新策略为: {}", if !src_au { "开" } else { "关" });
                }
            }
            let _ = inquire::Text::new("按回车返回 TUI").prompt();
        }
        s if s.starts_with("4.") => {
            let new_m = match method {
                Method::Symlink => Method::Copy,
                Method::Copy => Method::Symlink,
            };
            let prompt = format!("确认从 {:?} 转换为 {:?}？", method, new_m);
            if inquire::Confirm::new(&prompt).prompt().unwrap_or(false) {
                match crate::core::install::switch_install_method(layout, cfg, &mut app.registry, &skill, &target, new_m, false) {
                    Ok(_) => println!("已成功切换为 {:?}", new_m),
                    Err(Error::Mismatch(msg)) => {
                        eprintln!("检测到本地修改:\n{msg}");
                        if inquire::Confirm::new("转换为软连接将丢弃本地修改，是否强制转换？").with_default(false).prompt().unwrap_or(false) {
                            crate::core::install::switch_install_method(layout, cfg, &mut app.registry, &skill, &target, new_m, true)?;
                            println!("已强制切换为 {:?}", new_m);
                        }
                    }
                    Err(e) => eprintln!("切换失败: {e}"),
                }
                let _ = inquire::Text::new("按回车返回 TUI").prompt();
            }
        }
        s if s.starts_with("5.") => {
            confirm_delete_installed(layout, cfg, app)?;
        }
        _ => {}
    }

    Ok(())
}
```

- [ ] **Step 4: Run cargo test and clippy to verify**

Run: `cargo test tui -q`
Expected: PASS

- [ ] **Step 5: Commit**

```bash
git add src/tui/ui.rs src/tui/mod.rs
git commit -m "feat(tui): 已安装视图支持 Enter 管理动作菜单与 d 快捷删除"
```

---

### Task 6: 全局测试与测试机部署验证工作流

**Files:**
- Test: 全体单元测试与集成测试
- Scripts: `scripts/rsync-exclude.txt`

**Requirements:**
- 按照 `CLAUDE.md` 执行自动化测试机验证工作流：
  1. `cargo fmt` 与 `cargo clippy` 检查无警告无格式问题。
  2. 本地 `cargo test` 全部绿灯。
  3. rsync 到测试机 `test@192.168.188.210:/mnt/sda3/deploy/skills/tools/`（**严禁 `--delete`**，使用 `--exclude-from=scripts/rsync-exclude.txt`）。
  4. 测试机远程编译：`ssh test@192.168.188.210 'export PATH="$HOME/.cargo/bin:$PATH"; cd /mnt/sda3/deploy/skills/tools && cargo build'`。
  5. 在测试机上用真实二进制跑真实用户场景验证（临时 `SKILLS_HOME` 隔离：add -> list -> switch-method -> update -> remove）。
  6. 全部通过后进行自动提交（严禁自动 git push）。

- [ ] **Step 1: Run local fmt, clippy and tests**

Run:
```bash
cargo fmt -- --check
cargo clippy -- -D warnings
cargo test
```
Expected: PASS

- [ ] **Step 2: Sync code to test server**

Run:
```bash
rsync -a --exclude-from=scripts/rsync-exclude.txt ./ test@192.168.188.210:/mnt/sda3/deploy/skills/tools/
```
Expected: Sync successfully without `--delete`.

- [ ] **Step 3: Remote build on test server**

Run:
```bash
ssh test@192.168.188.210 'export PATH="$HOME/.cargo/bin:$PATH"; cd /mnt/sda3/deploy/skills/tools && cargo build'
```
Expected: Build success.

- [ ] **Step 4: Run real scenario tests on test server**

Run:
```bash
ssh test@192.168.188.210 'export PATH="$HOME/.cargo/bin:$PATH"; cd /mnt/sda3/deploy/skills/tools && export SKILLS_HOME=$(mktemp -d) && target/debug/skills add -s "*" -a "agents" -y && target/debug/skills list && rm -rf "$SKILLS_HOME"'
```
Expected: PASS

- [ ] **Step 5: Final commit if any adjustments were made**

```bash
git status
```
Ensure working tree is clean.

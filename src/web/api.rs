use axum::{
    Json, Router,
    extract::{Query, State},
    http::StatusCode,
    response::Html,
    routing::{get, post},
};
use std::sync::{Arc, Mutex};

use crate::core::{
    paths::{Layout, Scope},
    registry::{Method, Registry, TargetRec},
};

/// tmp 字段仅用于测试中持有临时目录句柄（防过早删除）。
pub struct AppState {
    pub layout: Layout,
    #[cfg(test)]
    pub tmp: Arc<tempfile::TempDir>,
}

// 非 test 构建需要无 tmp 的构造器
impl AppState {
    pub fn new(layout: Layout) -> Self {
        AppState {
            layout,
            #[cfg(test)]
            tmp: Arc::new(tempfile::tempdir().unwrap()),
        }
    }
}

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
        .route("/api/favorites", get(list_favorites).post(add_favorite))
        .route("/api/favorites/remove", post(remove_favorite))
        .route("/api/favorites/install", post(install_favorite))
        .route("/api/agents", get(list_agents))
        .with_state(Arc::new(Mutex::new(state)))
}

async fn index() -> Html<&'static str> {
    Html(include_str!("static/index.html"))
}

type S = State<Arc<Mutex<AppState>>>;

/// agent 注册表视图 + 本机检测标记：安装面板的目标多选数据源（取代旧 /api/targets）。
async fn list_agents(State(s): S) -> Json<serde_json::Value> {
    let detected = crate::core::agents::detect_installed();
    let _ = &s; // state 未用，但保持 handler 签名一致
    Json(serde_json::json!({
        "agents": crate::core::agents::all().iter().map(|a| serde_json::json!({
            "id": a.id, "name": a.name, "project_dir": a.project_dir,
            "global_supported": a.global_dir.is_some(),
            "universal": a.universal,
            "detected": detected.contains(&a.id),
        })).collect::<Vec<_>>()
    }))
}

/// 把安装请求的 agents + scope 展开为去重后的 TargetRec 列表（同 CLI add 向导的展开语义）。
/// scope 缺省 = project（web 为本机工具，项目根取服务器进程 cwd）；
/// 空 agents、非法 scope 值、未知 agent id 均为用户输入错误 → 400。
fn expand_request_targets(
    agents: &[String],
    scope: &Option<String>,
    cfg: &crate::core::config::Config,
) -> Result<Vec<TargetRec>, (StatusCode, String)> {
    if agents.is_empty() {
        return Err((StatusCode::BAD_REQUEST, "未选择目标".into()));
    }
    let scope = match scope.as_deref() {
        None | Some("project") => {
            let root = std::env::current_dir().map_err(|e| {
                (
                    StatusCode::INTERNAL_SERVER_ERROR,
                    format!("获取项目根失败: {e}"),
                )
            })?;
            Scope::Project(root)
        }
        Some("global") => Scope::Global,
        Some(other) => return Err((StatusCode::BAD_REQUEST, format!("非法 scope: {other}"))),
    };
    let targets = crate::wizard::expand_targets(agents, &scope, cfg)
        .map_err(|e| (StatusCode::BAD_REQUEST, format!("{e}")))?;
    if targets.is_empty() {
        return Err((
            StatusCode::BAD_REQUEST,
            "所选 agent 在该范围下均不可安装".into(),
        ));
    }
    Ok(targets.iter().map(crate::core::install::to_rec).collect())
}

#[derive(serde::Deserialize, Default)]
struct ListInstallsQuery {
    all: Option<bool>,
}

async fn list_installs(State(s): S, Query(q): Query<ListInstallsQuery>) -> Json<serde_json::Value> {
    let s = s.lock().unwrap();
    let reg = Registry::load(&s.layout).unwrap_or_default();
    let show_all = q.all.unwrap_or(false);
    let cwd = std::env::current_dir().ok();

    let filtered_installs: Vec<&crate::core::registry::Install> = if show_all {
        reg.installs.iter().collect()
    } else if let Some(ref d) = cwd {
        reg.installs
            .iter()
            .filter(|i| i.is_visible_in_dir(d))
            .collect()
    } else {
        reg.installs.iter().collect()
    };

    // 标签是技能级数据，不在 install 记录上；为前端展示注入每条 install 的 tags 字段
    let mut installs = serde_json::to_value(&filtered_installs).unwrap_or_default();
    if let Some(arr) = installs.as_array_mut() {
        for (jv, inst) in arr.iter_mut().zip(&filtered_installs) {
            jv["tags"] = serde_json::json!(reg.tags_of(&inst.source, &inst.skill));
        }
    }
    Json(installs)
}

async fn list_sources(State(s): S) -> Json<serde_json::Value> {
    let s = s.lock().unwrap();
    let reg = Registry::load(&s.layout).unwrap_or_default();
    Json(serde_json::json!(reg.sources))
}

#[derive(serde::Deserialize)]
struct ScanReq {
    source: String,
}

/// 扫描 source 的技能列表（安装前的预览步骤）：已缓存则复用，语义同 CLI add 的缓存复用。
async fn scan_source(
    State(s): S,
    Json(req): Json<ScanReq>,
) -> Result<Json<serde_json::Value>, (StatusCode, String)> {
    let s = s.lock().unwrap();
    let spec = crate::core::source::parse_source(&req.source)
        .map_err(|e| (StatusCode::BAD_REQUEST, format!("{e}")))?;
    let cached = crate::core::cache::ensure_cached(&s.layout, &spec)
        .map_err(|e| (StatusCode::INTERNAL_SERVER_ERROR, format!("缓存失败: {e}")))?;
    let skills = crate::core::cache::scan_skills(&cached.path)
        .map_err(|e| (StatusCode::INTERNAL_SERVER_ERROR, format!("扫描失败: {e}")))?;
    let skills: Vec<serde_json::Value> = skills
        .iter()
        .map(|e| serde_json::json!({"name": e.name, "description": e.description}))
        .collect();
    Ok(Json(serde_json::json!({
        "key": spec.key,
        "commit": cached.commit,
        "skills": skills,
    })))
}

#[derive(serde::Deserialize)]
struct InstallReq {
    source: String,
    skills: Vec<String>,
    agents: Vec<String>,
    scope: Option<String>, // "project" | "global"，默认 project
    method: Option<Method>,
    overwrite: Option<bool>,
}

/// 从仓库批量安装选中的技能，可指定一个或多个目标（技能 × 目标逐个安装）。
/// 技能存在性校验前置（任一不存在即整体 400，同 CLI add）；
/// 逐条安装逐条落盘（同 CLI install_loop：中途失败不留孤儿副本）；
/// 目标已存在 → 409 + 明细，前端确认后带 overwrite=true 重试（同 favorites/install 确认链）。
async fn install_from_source(
    State(s): S,
    Json(req): Json<InstallReq>,
) -> Result<Json<serde_json::Value>, (StatusCode, String)> {
    let s = s.lock().unwrap();
    // config 损坏必须显式 500（同 run_update：静默回退默认会把内置 target 解析到真实 home 并落盘）
    let cfg = crate::core::config::Config::load(&s.layout).map_err(|e| {
        (
            StatusCode::INTERNAL_SERVER_ERROR,
            format!("加载 config 失败: {e}"),
        )
    })?;
    let mut reg = Registry::load(&s.layout).map_err(|e| {
        (
            StatusCode::INTERNAL_SERVER_ERROR,
            format!("加载 registry 失败: {e}"),
        )
    })?;
    let spec = crate::core::source::parse_source(&req.source)
        .map_err(|e| (StatusCode::BAD_REQUEST, format!("{e}")))?;
    let cached = crate::core::cache::ensure_cached(&s.layout, &spec)
        .map_err(|e| (StatusCode::INTERNAL_SERVER_ERROR, format!("缓存失败: {e}")))?;
    let all = crate::core::cache::scan_skills(&cached.path)
        .map_err(|e| (StatusCode::INTERNAL_SERVER_ERROR, format!("扫描失败: {e}")))?;
    if req.skills.is_empty() {
        return Err((StatusCode::BAD_REQUEST, "未选择技能".into()));
    }
    let targets = expand_request_targets(&req.agents, &req.scope, &cfg)?;
    for name in &req.skills {
        if !all.iter().any(|e| &e.name == name) {
            return Err((StatusCode::BAD_REQUEST, format!("仓库中无技能 {name}")));
        }
    }
    let method = req.method.unwrap_or(cfg.default_method);
    let overwrite = req.overwrite.unwrap_or(false);
    reg.sources
        .entry(spec.key.clone())
        .or_insert(crate::core::registry::SourceRecord {
            url: spec.url.clone().unwrap_or_default(),
            commit: cached.commit.clone(),
            fetched_at: chrono::Utc::now().to_rfc3339(),
            auto_update: None,
        });
    let mut installed = Vec::new();
    for name in &req.skills {
        let entry = all.iter().find(|e| &e.name == name).expect("刚校验过存在");
        for trec in &targets {
            let target = trec.to_target();
            let result = crate::core::install::install_skill(
                &s.layout,
                &cfg,
                &mut reg,
                &spec.key,
                &entry.name,
                &entry.rel_path.to_string_lossy(),
                &target,
                method,
                &cached.commit,
            );
            match result {
                Ok(_) => {}
                Err(crate::core::error::Error::Conflict(p)) => {
                    if !overwrite {
                        return Err((StatusCode::CONFLICT, format!("{p:?} 已存在")));
                    }
                    // 与 CLI 的覆盖路径一致：先按记录删（无记录则忽略），再重装
                    let _ =
                        crate::core::remove::remove_install(&s.layout, &cfg, &mut reg, name, trec);
                    crate::core::install::install_skill(
                        &s.layout,
                        &cfg,
                        &mut reg,
                        &spec.key,
                        &entry.name,
                        &entry.rel_path.to_string_lossy(),
                        &target,
                        method,
                        &cached.commit,
                    )
                    .map_err(|e| {
                        (
                            StatusCode::INTERNAL_SERVER_ERROR,
                            format!("覆盖安装 {name} 失败: {e}"),
                        )
                    })?;
                }
                Err(e) => {
                    return Err((
                        StatusCode::INTERNAL_SERVER_ERROR,
                        format!("安装 {name} 失败: {e}"),
                    ));
                }
            }
            // 逐条落盘：之后任一技能/目标安装失败提前返回时，registry 与磁盘已写入的副本一致
            reg.save(&s.layout).map_err(|e| {
                (
                    StatusCode::INTERNAL_SERVER_ERROR,
                    format!("保存 registry 失败: {e}"),
                )
            })?;
        }
        installed.push(name.clone());
    }
    Ok(Json(serde_json::json!({ "installed": installed })))
}

#[derive(serde::Deserialize)]
struct AutoUpdateReq {
    skill: Option<String>,
    target: Option<TargetRec>,
    source: Option<String>,
    value: Option<bool>, // None = 跟随包级
}

async fn set_auto_update(State(s): S, Json(req): Json<AutoUpdateReq>) -> StatusCode {
    let s = s.lock().unwrap();
    let mut reg = match Registry::load(&s.layout) {
        Ok(r) => r,
        Err(_) => return StatusCode::INTERNAL_SERVER_ERROR,
    };
    if let Some(src) = req.source {
        if let Some(r) = reg.sources.get_mut(&src) {
            r.auto_update = req.value;
        } else {
            return StatusCode::NOT_FOUND;
        }
    } else if let (Some(skill), Some(target)) = (req.skill, req.target) {
        match reg
            .installs
            .iter_mut()
            .find(|i| i.skill == skill && i.target == target)
        {
            Some(i) => i.auto_update = req.value,
            None => return StatusCode::NOT_FOUND,
        }
    } else {
        return StatusCode::BAD_REQUEST;
    }
    match reg.save(&s.layout) {
        Ok(_) => StatusCode::OK,
        Err(_) => StatusCode::INTERNAL_SERVER_ERROR,
    }
}

#[derive(serde::Deserialize)]
struct TagsReq {
    skill: String,
    source: String,
    tags: Vec<String>,
}

async fn set_tags(State(s): S, Json(req): Json<TagsReq>) -> StatusCode {
    let s = s.lock().unwrap();
    let mut reg = match Registry::load(&s.layout) {
        Ok(r) => r,
        Err(_) => return StatusCode::INTERNAL_SERVER_ERROR,
    };
    match crate::core::tags::set_tags(&mut reg, &req.source, &req.skill, req.tags) {
        Ok(_) => match reg.save(&s.layout) {
            Ok(_) => StatusCode::OK,
            Err(_) => StatusCode::INTERNAL_SERVER_ERROR,
        },
        Err(_) => StatusCode::NOT_FOUND,
    }
}

#[derive(serde::Deserialize)]
struct RemoveReq {
    skill: String,
    target: TargetRec,
}

async fn remove_install(State(s): S, Json(req): Json<RemoveReq>) -> StatusCode {
    let s = s.lock().unwrap();
    let cfg = match crate::core::config::Config::load(&s.layout) {
        Ok(c) => c,
        Err(_) => return StatusCode::INTERNAL_SERVER_ERROR,
    };
    let mut reg = match Registry::load(&s.layout) {
        Ok(r) => r,
        Err(_) => return StatusCode::INTERNAL_SERVER_ERROR,
    };
    match crate::core::remove::remove_install(&s.layout, &cfg, &mut reg, &req.skill, &req.target) {
        Ok(_) => match reg.save(&s.layout) {
            Ok(_) => StatusCode::OK,
            Err(_) => StatusCode::INTERNAL_SERVER_ERROR,
        },
        Err(_) => StatusCode::NOT_FOUND,
    }
}

#[derive(serde::Deserialize)]
struct UpdateQuery {
    force: Option<bool>,
}

async fn run_update(
    State(s): S,
    axum::extract::Query(query): axum::extract::Query<UpdateQuery>,
) -> Result<Json<serde_json::Value>, (StatusCode, String)> {
    let s = s.lock().unwrap();
    // config 损坏时静默回退默认配置会把内置 target 解析到用户真实 home 目录并落盘，
    // 与下方损坏 registry 一样必须显式 500，由用户修复配置后重试。
    let cfg = crate::core::config::Config::load(&s.layout).map_err(|e| {
        (
            StatusCode::INTERNAL_SERVER_ERROR,
            format!("加载 config 失败: {e}"),
        )
    })?;
    // registry 损坏或更新执行（git/网络/落盘）失败必须显式 500 + 错误消息，
    // 不能吞错让前端误报“无更新”
    let mut reg = Registry::load(&s.layout).map_err(|e| {
        (
            StatusCode::INTERNAL_SERVER_ERROR,
            format!("加载 registry 失败: {e}"),
        )
    })?;
    let plan = crate::core::update::build_plan(&reg, None);
    let force = query.force.unwrap_or(false);
    let done = crate::core::update::execute_plan(&s.layout, &cfg, &mut reg, &plan, force).map_err(
        |e| match e {
            // 副本本地修改：409（用户可决策的冲突），前端确认后带 ?force=true 重试
            crate::core::error::Error::Mismatch(msg) => (StatusCode::CONFLICT, msg),
            e => (
                StatusCode::INTERNAL_SERVER_ERROR,
                format!("执行更新失败: {e}"),
            ),
        },
    )?;
    Ok(Json(serde_json::json!({ "done": done })))
}

async fn list_favorites(State(s): S) -> Result<Json<serde_json::Value>, (StatusCode, String)> {
    let s = s.lock().unwrap();
    let reg = Registry::load(&s.layout).map_err(|e| {
        (
            StatusCode::INTERNAL_SERVER_ERROR,
            format!("加载 registry 失败: {e}"),
        )
    })?;
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
    let mut reg = Registry::load(&s.layout).map_err(|e| {
        (
            StatusCode::INTERNAL_SERVER_ERROR,
            format!("加载 registry 失败: {e}"),
        )
    })?;
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
    reg.save(&s.layout).map_err(|e| {
        (
            StatusCode::INTERNAL_SERVER_ERROR,
            format!("保存 registry 失败: {e}"),
        )
    })?;
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
    let mut reg = Registry::load(&s.layout).map_err(|e| {
        (
            StatusCode::INTERNAL_SERVER_ERROR,
            format!("加载 registry 失败: {e}"),
        )
    })?;
    let key = crate::core::favorites::resolve_key(&reg, &req.source)
        .map_err(|e| (StatusCode::NOT_FOUND, format!("{e}")))?;
    crate::core::favorites::unbookmark(&mut reg, &key, &req.skill)
        .map_err(|e| (StatusCode::NOT_FOUND, format!("{e}")))?;
    reg.save(&s.layout).map_err(|e| {
        (
            StatusCode::INTERNAL_SERVER_ERROR,
            format!("保存 registry 失败: {e}"),
        )
    })?;
    Ok(StatusCode::OK)
}

#[derive(serde::Deserialize)]
struct FavInstallReq {
    source: String,
    skill: String,
    agents: Vec<String>,
    scope: Option<String>, // "project" | "global"，默认 project
    method: Option<Method>,
    overwrite: Option<bool>,
}

/// 从收藏安装，可指定一个或多个目标（逐目标安装逐条落盘）。
/// 冲突 → 409 + 明细，前端 confirm 后带 overwrite=true 重试（同 run_update 的确认链）。
async fn install_favorite(
    State(s): S,
    Json(req): Json<FavInstallReq>,
) -> Result<Json<serde_json::Value>, (StatusCode, String)> {
    let s = s.lock().unwrap();
    // config 损坏必须显式 500（同 run_update：静默回退默认会把内置 target 解析到真实 home 并落盘）
    let cfg = crate::core::config::Config::load(&s.layout).map_err(|e| {
        (
            StatusCode::INTERNAL_SERVER_ERROR,
            format!("加载 config 失败: {e}"),
        )
    })?;
    let mut reg = Registry::load(&s.layout).map_err(|e| {
        (
            StatusCode::INTERNAL_SERVER_ERROR,
            format!("加载 registry 失败: {e}"),
        )
    })?;
    let key = crate::core::favorites::resolve_key(&reg, &req.source)
        .map_err(|e| (StatusCode::NOT_FOUND, format!("{e}")))?;
    let targets = expand_request_targets(&req.agents, &req.scope, &cfg)?;
    let method = req.method.unwrap_or(cfg.default_method);
    let overwrite = req.overwrite.unwrap_or(false);
    for trec in &targets {
        let target = trec.to_target();
        let do_install = |reg: &mut Registry| {
            crate::core::favorites::fav_install(
                &s.layout, &cfg, reg, &key, &req.skill, &target, method,
            )
        };
        match do_install(&mut reg) {
            Ok(_) => {}
            Err(crate::core::error::Error::Conflict(p)) => {
                if !overwrite {
                    return Err((StatusCode::CONFLICT, format!("{p:?} 已存在")));
                }
                // 与 CLI 的覆盖路径一致：先按记录删（无记录则忽略），再重装
                let _ = crate::core::remove::remove_install(
                    &s.layout, &cfg, &mut reg, &req.skill, trec,
                );
                do_install(&mut reg).map_err(|e| {
                    (
                        StatusCode::INTERNAL_SERVER_ERROR,
                        format!("覆盖安装失败: {e}"),
                    )
                })?;
            }
            Err(crate::core::error::Error::NotBookmarked(e)) => {
                return Err((StatusCode::NOT_FOUND, e));
            }
            Err(e) => return Err((StatusCode::INTERNAL_SERVER_ERROR, format!("安装失败: {e}"))),
        }
        // 逐条落盘：之后任一目标安装失败提前返回时，registry 与磁盘已写入的副本一致
        reg.save(&s.layout).map_err(|e| {
            (
                StatusCode::INTERNAL_SERVER_ERROR,
                format!("保存 registry 失败: {e}"),
            )
        })?;
    }
    Ok(Json(serde_json::json!({ "installed": req.skill })))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::core::registry::{Install, Method, TargetRec};
    use axum::body::Body;
    use axum::http::Request;
    use tower::ServiceExt; // oneshot

    #[tokio::test]
    async fn list_agents_returns_registry_with_detected_flag() {
        let app = router(test_state());
        let resp = app
            .oneshot(
                Request::builder()
                    .uri("/api/agents")
                    .body(Body::empty())
                    .unwrap(),
            )
            .await
            .unwrap();
        assert_eq!(resp.status(), StatusCode::OK);
        let body = axum::body::to_bytes(resp.into_body(), usize::MAX)
            .await
            .unwrap();
        let v: serde_json::Value = serde_json::from_slice(&body).unwrap();
        let agents = v["agents"].as_array().unwrap();
        assert!(agents.len() >= 60);
        let claude = agents.iter().find(|a| a["id"] == "claude-code").unwrap();
        assert_eq!(claude["project_dir"], ".claude/skills");
        assert!(claude.get("detected").is_some());
        // eve 不支持全局
        let eve = agents.iter().find(|a| a["id"] == "eve").unwrap();
        assert_eq!(eve["global_supported"], false);
    }

    fn test_state() -> AppState {
        let tmp = Arc::new(tempfile::tempdir().unwrap());
        let layout = Layout::at(tmp.path().join(".skills"));
        let mut reg = Registry {
            version: 3,
            ..Default::default()
        };
        reg.installs.push(Install {
            skill: "alpha".into(),
            source: "github/o/r".into(),
            source_path: "skills/alpha".into(),
            target: TargetRec::Global {
                agent: "agents".into(),
            },
            method: Method::Copy,
            commit: "c1".into(),
            auto_update: None,
            installed_at: "t".into(),
        });
        reg.skill_tags
            .entry("github/o/r".into())
            .or_default()
            .insert("alpha".into(), vec!["frontend".into()]);
        reg.save(&layout).unwrap();
        AppState { layout, tmp }
    }

    #[tokio::test]
    async fn list_installs_returns_json() {
        let app = router(test_state());
        let resp = app
            .oneshot(
                axum::http::Request::builder()
                    .uri("/api/installs")
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
        assert_eq!(v[0]["skill"], "alpha");
        assert_eq!(v[0]["tags"][0], "frontend");
    }

    #[tokio::test]
    async fn list_installs_filters_other_project_unless_all() {
        let state = test_state();
        {
            let mut reg = Registry::load(&state.layout).unwrap();
            reg.installs.push(Install {
                skill: "beta".into(),
                source: "github/o/r".into(),
                source_path: "skills/beta".into(),
                target: TargetRec::Project {
                    agent: "agents".into(),
                    root: std::path::PathBuf::from("/nonexistent/other/project"),
                },
                method: Method::Copy,
                commit: "c1".into(),
                auto_update: None,
                installed_at: "t".into(),
            });
            reg.save(&state.layout).unwrap();
        }
        let _keep = state.tmp.clone();
        let app = router(state);

        // 默认不带 ?all=true：只返回当前项目或全局的（alpha 是 global，beta 在外部路径被过滤）
        let resp = app
            .clone()
            .oneshot(
                axum::http::Request::builder()
                    .uri("/api/installs")
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
        assert_eq!(v.as_array().unwrap().len(), 1);
        assert_eq!(v[0]["skill"], "alpha");

        // 带 ?all=true：返回全部（包含 beta）
        let resp_all = app
            .oneshot(
                axum::http::Request::builder()
                    .uri("/api/installs?all=true")
                    .body(axum::body::Body::empty())
                    .unwrap(),
            )
            .await
            .unwrap();
        assert_eq!(resp_all.status(), 200);
        let body_all = axum::body::to_bytes(resp_all.into_body(), usize::MAX)
            .await
            .unwrap();
        let v_all: serde_json::Value = serde_json::from_slice(&body_all).unwrap();
        assert_eq!(v_all.as_array().unwrap().len(), 2);
    }

    #[tokio::test]
    async fn set_auto_update_writes_registry() {
        let state = test_state();
        let layout_root = state.layout.root.clone();
        // oneshot 会消费并 drop router（连同 AppState），测试需自持 tempdir 句柄防目录被清理
        let keep = state.tmp.clone();
        let app = router(state);
        let resp = app
            .oneshot(
                axum::http::Request::builder()
                    .method("POST")
                    .uri("/api/auto-update")
                    .header("content-type", "application/json")
                    .body(axum::body::Body::from(
                        serde_json::to_string(&serde_json::json!({
                            "skill": "alpha",
                            "target": {"scope": "global", "agent": "agents"},
                            "value": true
                        }))
                        .unwrap(),
                    ))
                    .unwrap(),
            )
            .await
            .unwrap();
        assert_eq!(resp.status(), 200);
        let reg = Registry::load(&Layout::at(layout_root)).unwrap();
        assert_eq!(reg.installs[0].auto_update, Some(true));
        drop(keep);
    }

    #[tokio::test]
    async fn set_tags_binds_to_source_skill() {
        let state = test_state();
        let layout_root = state.layout.root.clone();
        let keep = state.tmp.clone();
        let app = router(state);
        let resp = app
            .oneshot(
                axum::http::Request::builder()
                    .method("POST")
                    .uri("/api/tags")
                    .header("content-type", "application/json")
                    .body(axum::body::Body::from(
                        serde_json::to_string(&serde_json::json!({
                            "skill": "alpha",
                            "source": "github/o/r",
                            "tags": ["backend"]
                        }))
                        .unwrap(),
                    ))
                    .unwrap(),
            )
            .await
            .unwrap();
        assert_eq!(resp.status(), 200);
        let reg = Registry::load(&Layout::at(layout_root)).unwrap();
        // 覆盖式写入技能级标签，原有 frontend 被替换
        assert_eq!(reg.tags_of("github/o/r", "alpha"), &["backend"]);
        drop(keep);
    }

    // ---- 从仓库安装（多技能选择） ----

    async fn body_json(resp: axum::response::Response) -> serde_json::Value {
        let body = axum::body::to_bytes(resp.into_body(), usize::MAX)
            .await
            .unwrap();
        serde_json::from_slice(&body).unwrap()
    }

    /// agents/claude 两个 target 都重定向到临时目录的 AppState，避免安装测试落盘到真实 home
    fn test_state_with_target() -> AppState {
        let tmp = Arc::new(tempfile::tempdir().unwrap());
        let layout = Layout::at(tmp.path().join(".skills"));
        std::fs::create_dir_all(&layout.root).unwrap();
        let mut targets = toml::value::Table::new();
        for name in ["agents", "claude"] {
            targets.insert(
                name.into(),
                toml::Value::String(tmp.path().join(name).to_string_lossy().into_owned()),
            );
        }
        let mut doc = toml::value::Table::new();
        doc.insert("targets".into(), toml::Value::Table(targets));
        std::fs::write(layout.config_path(), toml::to_string(&doc).unwrap()).unwrap();
        AppState { layout, tmp }
    }

    /// 多技能本地源：skills/sa、skills/sb 两个技能
    fn make_multi_skill_source(parent: &std::path::Path) -> std::path::PathBuf {
        let src = parent.join("multi-src");
        for (name, desc) in [("sa", "技能 A"), ("sb", "技能 B")] {
            let dir = src.join("skills").join(name);
            std::fs::create_dir_all(&dir).unwrap();
            std::fs::write(
                dir.join("SKILL.md"),
                format!("---\nname: {name}\ndescription: {desc}\n---\n"),
            )
            .unwrap();
        }
        src
    }

    #[tokio::test]
    async fn scan_lists_all_skills_of_multi_skill_source() {
        let state = test_state_with_target();
        let src = make_multi_skill_source(state.tmp.path());
        let app = router(state);
        let resp = app
            .oneshot(post_json(
                "/api/scan",
                serde_json::json!({"source": src.to_str().unwrap()}),
            ))
            .await
            .unwrap();
        assert_eq!(resp.status(), 200);
        let v = body_json(resp).await;
        assert_eq!(v["key"], "local/multi-src");
        let skills: Vec<(&str, &str)> = v["skills"]
            .as_array()
            .unwrap()
            .iter()
            .map(|s| {
                (
                    s["name"].as_str().unwrap(),
                    s["description"].as_str().unwrap(),
                )
            })
            .collect();
        assert!(skills.contains(&("sa", "技能 A")), "{skills:?}");
        assert!(skills.contains(&("sb", "技能 B")), "{skills:?}");
    }

    #[tokio::test]
    async fn install_selected_skills_only() {
        let state = test_state_with_target();
        let layout_root = state.layout.root.clone();
        let tmp_root = state.tmp.path().to_path_buf();
        let keep = state.tmp.clone();
        let src = make_multi_skill_source(&tmp_root);
        let app = router(state);
        let resp = app
            .oneshot(post_json(
                "/api/install",
                serde_json::json!({
                    "source": src.to_str().unwrap(),
                    "skills": ["sa"],
                    "agents": ["agents"],
                    "scope": "global",
                    "method": "copy",
                }),
            ))
            .await
            .unwrap();
        assert_eq!(resp.status(), 200);
        let v = body_json(resp).await;
        assert_eq!(v["installed"], serde_json::json!(["sa"]));
        // 只装选中的 sa：registry 与磁盘都没有 sb
        let reg = Registry::load(&Layout::at(layout_root)).unwrap();
        assert_eq!(reg.installs.len(), 1);
        assert_eq!(reg.installs[0].skill, "sa");
        assert!(tmp_root.join("agents/sa/SKILL.md").exists());
        assert!(!tmp_root.join("agents/sb").exists());
        drop(keep);
    }

    #[tokio::test]
    async fn install_rejects_unknown_skill_before_any_install() {
        let state = test_state_with_target();
        let layout_root = state.layout.root.clone();
        let tmp_root = state.tmp.path().to_path_buf();
        let keep = state.tmp.clone();
        let src = make_multi_skill_source(&tmp_root);
        let app = router(state);
        let resp = app
            .oneshot(post_json(
                "/api/install",
                serde_json::json!({
                    "source": src.to_str().unwrap(),
                    "skills": ["sa", "ghost"],
                    "agents": ["agents"],
                    "scope": "global",
                    "method": "copy",
                }),
            ))
            .await
            .unwrap();
        // 任一技能不存在即整体 400：校验前置，sa 也不得安装（同 CLI add 语义）
        assert_eq!(resp.status(), 400);
        let reg = Registry::load(&Layout::at(layout_root)).unwrap();
        assert!(reg.installs.is_empty());
        assert!(!tmp_root.join("agents/sa").exists());
        drop(keep);
    }

    #[tokio::test]
    async fn install_conflict_409_then_overwrite_succeeds() {
        let state = test_state_with_target();
        let tmp_root = state.tmp.path().to_path_buf();
        let keep = state.tmp.clone();
        let src = make_multi_skill_source(&tmp_root);
        let body = |overwrite: bool| {
            serde_json::json!({
                "source": src.to_str().unwrap(),
                "skills": ["sa"],
                "agents": ["agents"],
                "scope": "global",
                "method": "copy",
                "overwrite": overwrite,
            })
        };
        let app = router(state);
        let resp = app
            .clone()
            .oneshot(post_json("/api/install", body(false)))
            .await
            .unwrap();
        assert_eq!(resp.status(), 200);
        // 已存在 → 409，前端确认后带 overwrite=true 重试（同 favorites/install 确认链）
        let resp = app
            .clone()
            .oneshot(post_json("/api/install", body(false)))
            .await
            .unwrap();
        assert_eq!(resp.status(), 409);
        let resp = app
            .oneshot(post_json("/api/install", body(true)))
            .await
            .unwrap();
        assert_eq!(resp.status(), 200);
        drop(keep);
    }

    #[tokio::test]
    async fn install_to_multiple_targets() {
        let state = test_state_with_target();
        let layout_root = state.layout.root.clone();
        let tmp_root = state.tmp.path().to_path_buf();
        let keep = state.tmp.clone();
        let src = make_multi_skill_source(&tmp_root);
        let app = router(state);
        let resp = app
            .oneshot(post_json(
                "/api/install",
                serde_json::json!({
                    "source": src.to_str().unwrap(),
                    "skills": ["sa"],
                    "agents": ["agents", "claude"],
                    "scope": "global",
                    "method": "copy",
                }),
            ))
            .await
            .unwrap();
        assert_eq!(resp.status(), 200);
        // 同一技能装进两个目标：registry 两条记录，两边磁盘都有副本
        let reg = Registry::load(&Layout::at(layout_root)).unwrap();
        assert_eq!(reg.installs.len(), 2);
        assert!(tmp_root.join("agents/sa/SKILL.md").exists());
        assert!(tmp_root.join("claude/sa/SKILL.md").exists());
        drop(keep);
    }

    #[tokio::test]
    async fn install_rejects_empty_agents() {
        let state = test_state_with_target();
        let tmp_root = state.tmp.path().to_path_buf();
        let keep = state.tmp.clone();
        let src = make_multi_skill_source(&tmp_root);
        let app = router(state);
        let resp = app
            .oneshot(post_json(
                "/api/install",
                serde_json::json!({
                    "source": src.to_str().unwrap(),
                    "skills": ["sa"],
                    "agents": [],
                    "method": "copy",
                }),
            ))
            .await
            .unwrap();
        assert_eq!(resp.status(), 400);
        drop(keep);
    }

    #[tokio::test]
    async fn install_rejects_invalid_scope() {
        let state = test_state_with_target();
        let tmp_root = state.tmp.path().to_path_buf();
        let keep = state.tmp.clone();
        let src = make_multi_skill_source(&tmp_root);
        let app = router(state);
        let resp = app
            .oneshot(post_json(
                "/api/install",
                serde_json::json!({
                    "source": src.to_str().unwrap(),
                    "skills": ["sa"],
                    "agents": ["agents"],
                    "scope": "bogus",
                    "method": "copy",
                }),
            ))
            .await
            .unwrap();
        assert_eq!(resp.status(), 400);
        assert!(!tmp_root.join("agents/sa").exists());
        drop(keep);
    }

    #[tokio::test]
    async fn index_html_served_at_root() {
        let app = router(test_state());
        let resp = app
            .oneshot(
                axum::http::Request::builder()
                    .uri("/")
                    .body(axum::body::Body::empty())
                    .unwrap(),
            )
            .await
            .unwrap();
        assert_eq!(resp.status(), 200);
        let body = axum::body::to_bytes(resp.into_body(), usize::MAX)
            .await
            .unwrap();
        assert!(String::from_utf8_lossy(&body).contains("skills"));
    }

    #[tokio::test]
    async fn run_update_returns_500_on_corrupted_registry() {
        let state = test_state();
        // 人为写坏 registry.json：加载必须显式 500，而不是吞错返回 200 + {done: []}
        std::fs::write(state.layout.registry_path(), "{ 这不是合法 json").unwrap();
        let app = router(state);
        let resp = app
            .oneshot(
                axum::http::Request::builder()
                    .method("POST")
                    .uri("/api/update")
                    .body(axum::body::Body::empty())
                    .unwrap(),
            )
            .await
            .unwrap();
        assert_eq!(resp.status(), 500);
        let body = axum::body::to_bytes(resp.into_body(), usize::MAX)
            .await
            .unwrap();
        let text = String::from_utf8_lossy(&body);
        assert!(text.contains("加载 registry 失败"), "{text}");
    }

    #[tokio::test]
    async fn run_update_returns_500_on_corrupted_config() {
        let state = test_state();
        // 人为写坏 config.toml：若静默回退默认配置，内置 target 会解析到用户真实
        // home 目录并在 update 中对其落盘。必须与损坏 registry 一样显式 500。
        std::fs::create_dir_all(&state.layout.root).unwrap();
        std::fs::write(state.layout.config_path(), "[web]\nport = 99999\n").unwrap();
        let app = router(state);
        let resp = app
            .oneshot(
                axum::http::Request::builder()
                    .method("POST")
                    .uri("/api/update")
                    .body(axum::body::Body::empty())
                    .unwrap(),
            )
            .await
            .unwrap();
        assert_eq!(resp.status(), 500);
        let body = axum::body::to_bytes(resp.into_body(), usize::MAX)
            .await
            .unwrap();
        let text = String::from_utf8_lossy(&body);
        assert!(text.contains("加载 config 失败"), "{text}");
    }

    #[tokio::test]
    async fn run_update_ok_with_empty_plan() {
        // test_state 无任何 source 开启 auto_update → 空计划，成功路径仍返回 200 + {done: []}
        let app = router(test_state());
        let resp = app
            .oneshot(
                axum::http::Request::builder()
                    .method("POST")
                    .uri("/api/update")
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
        assert_eq!(v["done"], serde_json::json!([]));
    }

    /// 本地修改的 copy 副本：无 force 返回 409 + 明细；带 ?force=true 则覆盖更新。
    /// 需要真 git 远端（更新走 fetch 路径），与 update.rs 集成测试同型。
    #[tokio::test]
    async fn run_update_409_on_local_modification_then_force_succeeds() {
        let tmp = Arc::new(tempfile::tempdir().unwrap());
        let work = tmp.path().join("work");
        let bare = tmp.path().join("bare.git");
        std::fs::create_dir_all(work.join("skills/alpha")).unwrap();
        std::fs::write(
            work.join("skills/alpha/SKILL.md"),
            "---\nname: alpha\ndescription: A\n---\nv1\n",
        )
        .unwrap();
        let git = |args: &[&str]| {
            let st = std::process::Command::new("git")
                .args(args)
                .current_dir(&work)
                .status()
                .unwrap();
            assert!(st.success(), "git {args:?} 失败");
        };
        git(&["init", "-b", "main"]);
        git(&["add", "."]);
        git(&[
            "-c",
            "user.email=t@t",
            "-c",
            "user.name=t",
            "commit",
            "-m",
            "c1",
        ]);
        git(&["clone", "--bare", ".", bare.to_str().unwrap()]);

        let layout = Layout::at(tmp.path().join(".skills"));
        // agents target 必须指到临时目录：默认配置指向真实 home，测试绝不能落盘到那里
        std::fs::create_dir_all(&layout.root).unwrap();
        std::fs::write(
            layout.config_path(),
            format!("[targets]\nagents = {:?}\n", tmp.path().join("agents")),
        )
        .unwrap();
        let cache = layout.cache_dir("github/o/r");
        std::fs::create_dir_all(cache.parent().unwrap()).unwrap();
        let url = format!("file://{}", bare.display());
        let c1 = crate::core::git::shallow_clone(&url, &cache).unwrap();

        let cfg = crate::core::config::Config::load(&layout).unwrap();
        let mut reg = Registry {
            version: 3,
            ..Default::default()
        };
        reg.sources.insert(
            "github/o/r".into(),
            crate::core::registry::SourceRecord {
                url: url.clone(),
                commit: c1.clone(),
                fetched_at: "2026-08-20T00:00:00Z".into(),
                auto_update: Some(true),
            },
        );
        crate::core::install::install_skill(
            &layout,
            &cfg,
            &mut reg,
            "github/o/r",
            "alpha",
            "skills/alpha",
            &crate::core::paths::Target {
                agent: "agents".into(),
                scope: Scope::Global,
            },
            Method::Copy,
            &c1,
        )
        .unwrap();
        reg.installs[0].auto_update = Some(true);
        reg.save(&layout).unwrap();

        // 远端推进到 v2
        std::fs::write(
            work.join("skills/alpha/SKILL.md"),
            "---\nname: alpha\ndescription: A\n---\nv2\n",
        )
        .unwrap();
        git(&["add", "."]);
        git(&[
            "-c",
            "user.email=t@t",
            "-c",
            "user.name=t",
            "commit",
            "-m",
            "c2",
        ]);
        git(&["push", bare.to_str().unwrap(), "main"]);
        // 用户在副本里改了文件
        let dest = tmp.path().join("agents/alpha");
        std::fs::write(dest.join("SKILL.md"), "用户本地修改\n").unwrap();

        let post = |uri: &str| {
            axum::http::Request::builder()
                .method("POST")
                .uri(uri)
                .body(axum::body::Body::empty())
                .unwrap()
        };
        let app = router(AppState {
            layout: Layout::at(tmp.path().join(".skills")),
            tmp: tmp.clone(),
        });
        // 无 force：409 + 可读明细
        let resp = app.clone().oneshot(post("/api/update")).await.unwrap();
        assert_eq!(resp.status(), 409);
        let body = axum::body::to_bytes(resp.into_body(), usize::MAX)
            .await
            .unwrap();
        let text = String::from_utf8_lossy(&body);
        assert!(text.contains("SKILL.md"), "{text}");
        // 409 不得造成任何变更
        assert_eq!(
            std::fs::read_to_string(dest.join("SKILL.md")).unwrap(),
            "用户本地修改\n"
        );
        // force：覆盖更新成功
        let resp = app.oneshot(post("/api/update?force=true")).await.unwrap();
        assert_eq!(resp.status(), 200);
        let body = axum::body::to_bytes(resp.into_body(), usize::MAX)
            .await
            .unwrap();
        let v: serde_json::Value = serde_json::from_slice(&body).unwrap();
        assert!(v["done"].as_array().unwrap().len() >= 2, "{v}");
        assert!(
            std::fs::read_to_string(dest.join("SKILL.md"))
                .unwrap()
                .contains("v2")
        );
    }

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
            .body(axum::body::Body::from(
                serde_json::to_string(&body).unwrap(),
            ))
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
            .oneshot(post_json(
                "/api/favorites",
                serde_json::json!({"source": src, "skill": []}),
            ))
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
            .oneshot(post_json(
                "/api/favorites",
                serde_json::json!({"source": "noslash", "skill": []}),
            ))
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
            format!(
                "[targets]\nagents = {:?}\n",
                state.tmp.path().join("agents")
            ),
        )
        .unwrap();
        let src = make_local_source(&state.tmp);
        let agents = state.tmp.path().join("agents");
        let keep = state.tmp.clone();
        let app = router(state);
        // 先收藏
        let resp = app
            .clone()
            .oneshot(post_json(
                "/api/favorites",
                serde_json::json!({"source": src, "skill": []}),
            ))
            .await
            .unwrap();
        assert_eq!(resp.status(), 200);
        let install = |overwrite: bool| {
            post_json(
                "/api/favorites/install",
                serde_json::json!({
                    "source": "local/mysrc",
                    "skill": "alpha",
                    "agents": ["agents"],
                    "scope": "global",
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
                    "agents": ["agents"],
                    "scope": "global"
                }),
            ))
            .await
            .unwrap();
        assert_eq!(resp.status(), 404);
        drop(keep);
    }
}

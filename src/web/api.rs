use axum::{
    Json, Router,
    extract::State,
    http::StatusCode,
    response::Html,
    routing::{get, post},
};
use std::sync::{Arc, Mutex};

use crate::core::{
    paths::Layout,
    registry::{Registry, TargetRec},
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
        .route("/api/sources", get(list_sources))
        .route("/api/auto-update", post(set_auto_update))
        .route("/api/tags", post(set_tags))
        .route("/api/remove", post(remove_install))
        .route("/api/update", post(run_update))
        .with_state(Arc::new(Mutex::new(state)))
}

async fn index() -> Html<&'static str> {
    Html(include_str!("static/index.html"))
}

type S = State<Arc<Mutex<AppState>>>;

async fn list_installs(State(s): S) -> Json<serde_json::Value> {
    let s = s.lock().unwrap();
    let reg = Registry::load(&s.layout).unwrap_or_default();
    Json(serde_json::json!(reg.installs))
}

async fn list_sources(State(s): S) -> Json<serde_json::Value> {
    let s = s.lock().unwrap();
    let reg = Registry::load(&s.layout).unwrap_or_default();
    Json(serde_json::json!(reg.sources))
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
    target: TargetRec,
    tags: Vec<String>,
}

async fn set_tags(State(s): S, Json(req): Json<TagsReq>) -> StatusCode {
    let s = s.lock().unwrap();
    let mut reg = match Registry::load(&s.layout) {
        Ok(r) => r,
        Err(_) => return StatusCode::INTERNAL_SERVER_ERROR,
    };
    match crate::core::tags::set_tags(&mut reg, &req.skill, &req.target, req.tags) {
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

async fn run_update(State(s): S) -> Json<serde_json::Value> {
    let s = s.lock().unwrap();
    let cfg = crate::core::config::Config::load(&s.layout).unwrap_or_default();
    let mut reg = Registry::load(&s.layout).unwrap_or_default();
    let plan = crate::core::update::build_plan(&reg, None);
    let done =
        crate::core::update::execute_plan(&s.layout, &cfg, &mut reg, &plan).unwrap_or_default();
    Json(serde_json::json!({ "done": done }))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::core::registry::{Install, Method, TargetRec};
    use tower::ServiceExt; // oneshot

    fn test_state() -> AppState {
        let tmp = Arc::new(tempfile::tempdir().unwrap());
        let layout = Layout::at(tmp.path().join(".skills"));
        let mut reg = Registry {
            version: 1,
            ..Default::default()
        };
        reg.installs.push(Install {
            skill: "alpha".into(),
            source: "github/o/r".into(),
            source_path: "skills/alpha".into(),
            target: TargetRec::Global {
                name: "agents".into(),
            },
            method: Method::Copy,
            commit: "c1".into(),
            tags: vec!["frontend".into()],
            auto_update: None,
            installed_at: "t".into(),
        });
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
                            "target": {"kind": "global", "name": "agents"},
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
}

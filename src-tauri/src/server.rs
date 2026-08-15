use axum::{
    body::Body,
    extract::{Path, State},
    http::{header, StatusCode, Uri},
    response::{IntoResponse, Response},
    routing::{get, post, put},
    Json, Router,
};
use rust_embed::RustEmbed;
use serde::{Deserialize, Serialize};
use serde_json::{json, Value};
use std::{path::PathBuf, sync::Arc};

#[derive(RustEmbed)]
#[folder = "../frontend/"]
struct FrontendAsset;

/// 加速配置(阶段 4,任务书 §五)。mode ∈ off|official|custom;custom_node 为用户自定义节点地址。
/// 持久化到 `{codex_home}/2xapi-settings.json` 的 `accel` 段(camelCase 与前端契约一致)。
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct AccelCfg {
    #[serde(default)]
    pub mode: String,
    #[serde(default)]
    pub custom_node: String,
}

impl Default for AccelCfg {
    fn default() -> Self {
        AccelCfg { mode: "off".into(), custom_node: String::new() }
    }
}

#[derive(Clone)]
pub struct AppState {
    pub config_path: PathBuf,
    pub backup_dir: PathBuf,
    pub providers_path: PathBuf,
    pub codex_home: PathBuf,
    pub launcher: std::sync::Arc<crate::launcher::LauncherState>,
    /// 加速线路健康状态(启动时由 load_lines 填充;健康循环每 30s 刷新)。
    pub health: std::sync::Arc<crate::acclines::HealthState>,
    /// 加速开关配置(mode + 自定义节点;内存态 + 2xapi-settings.json 持久化)。
    pub accel: std::sync::Arc<std::sync::Mutex<AccelCfg>>,
}

pub fn build_router(state: AppState) -> Router {
    let state = Arc::new(state);
    Router::new()
        // --- Static frontend ---
        .route("/", get(serve_index))
        .fallback(serve_static)
        // --- 网关健康（FR-4.1，不走统一响应信封）---
        .route("/health", get(handle_gateway_health))
        // --- 网关代理 /v1/* 和 /*（Codex 可能带或不带 /v1 前缀）---
        .route("/v1/responses", post(crate::gateway::proxy_responses))
        .route("/responses", post(crate::gateway::proxy_responses))
        .route("/v1/chat/completions", post(crate::gateway::proxy_chat))
        .route("/chat/completions", post(crate::gateway::proxy_chat))
        .route("/v1/models", get(crate::gateway::proxy_models))
        .route("/models", get(crate::gateway::proxy_models))
        // --- Health & session ---
        .route("/api/health", get(handle_health))
        .route("/api/session", get(handle_session))
        // --- Auth ---
        .route("/api/auth/captcha", get(handle_auth_captcha))
        .route("/api/auth/login", post(handle_auth_login))
        .route("/api/auth/logout", post(handle_auth_logout))
        .route("/api/auth/remembered", get(handle_auth_remembered))
        .route("/api/auth/remember", post(handle_auth_remember))
        .route("/api/auth/forget", post(handle_auth_forget))
        .route("/api/key-groups", get(handle_key_groups))
        .route("/api/auth/api-keys", get(handle_auth_api_keys))
        .route("/api/auth/me", get(handle_auth_me))
        // --- Providers（04 契约）---
        .route("/api/providers", get(handle_providers_list).post(handle_providers_create))
        .route("/api/providers/active", get(handle_providers_active))
        .route("/api/providers/reorder", put(handle_providers_reorder))
        .route("/api/providers/activate", post(handle_providers_activate))
        .route("/api/providers/activate-official", post(handle_providers_activate_official))
        .route("/api/providers/preview-config", post(handle_providers_preview))
        .route("/api/providers/fetch-models", post(handle_providers_fetch_models))
        .route("/api/providers/fetch-balance", post(handle_providers_fetch_balance))
        .route("/api/providers/diagnose", post(handle_providers_diagnose))
        .route("/api/providers/:id", put(handle_providers_update).delete(handle_providers_delete))
        // --- Codex 启动器（M7，直连版）---
        .route("/api/launcher/preflight", post(handle_launcher_preflight))
        .route("/api/launcher/start", post(handle_launcher_start))
        .route("/api/launcher/stop", post(handle_launcher_stop))
        .route("/api/launcher/status", get(handle_launcher_status))
        // --- 桌面版托管开关(阶段 1,任务书 §1.1)---
        .route("/api/desktop/state", get(handle_desktop_state))
        .route("/api/desktop/host", post(handle_desktop_host))
        .route("/api/desktop/unhost", post(handle_desktop_unhost))
        // --- Backups & history ---
        .route("/api/backups", get(handle_backups))
        .route("/api/history/inspect", get(handle_history))
        .route("/api/sessions", get(handle_sessions_list))
        .route("/api/sessions/repair", post(handle_sessions_repair))
        .route("/api/sessions/settings", get(handle_sessions_settings).post(handle_sessions_settings_set))
        // --- 加速线路(阶段 4,任务书 §五)---
        .route("/api/accel/state", get(handle_accel_state))
        .route("/api/accel/mode", post(handle_accel_mode))
        .route("/api/accel/custom-node", post(handle_accel_custom_node))
        .route("/api/accel/test-node", post(handle_accel_test_node))
        .route("/api/config/snapshot", post(handle_config_snapshot))
        .route("/api/config/restore", post(handle_config_restore))
        .with_state(state)
}

// --- Helpers ---

fn ok_json(data: Value) -> Response {
    (StatusCode::OK, Json(data)).into_response()
}

fn err_json(status: StatusCode, msg: &str) -> Response {
    (
        status,
        Json(json!({ "error": msg })),
    )
        .into_response()
}

// 统一响应信封（04 §0）：{ok:true,data} / {ok:false,error:{code,message,fields?}}
fn ok_env(data: Value) -> Response {
    (StatusCode::OK, Json(json!({ "ok": true, "data": data }))).into_response()
}

fn err_env(status: StatusCode, code: &str, message: &str, fields: Option<Vec<String>>) -> Response {
    let mut error = json!({ "code": code, "message": message });
    if let Some(f) = fields {
        error["fields"] = json!(f);
    }
    (status, Json(json!({ "ok": false, "error": error }))).into_response()
}

fn val_errs_env(errs: &[crate::providers::ValidationError]) -> Response {
    let fields: Vec<String> = errs.iter().map(|e| e.field.clone()).collect();
    err_env(
        StatusCode::UNPROCESSABLE_ENTITY,
        "E_VALIDATION",
        &crate::providers::format_errors(errs),
        Some(fields),
    )
}

async fn serve_index() -> Response {
    match FrontendAsset::get("index.html") {
        Some(file) => Response::builder()
            .header(header::CONTENT_TYPE, "text/html; charset=utf-8")
            .body(Body::from(file.data.into_owned()))
            .unwrap(),
        None => (StatusCode::NOT_FOUND, "index.html not found").into_response(),
    }
}

async fn serve_static(uri: Uri) -> Response {
    let path = uri.path().trim_start_matches('/');
    if path.starts_with("api/") {
        return err_json(StatusCode::NOT_FOUND, "route not found");
    }
    let file = if path.is_empty() {
        FrontendAsset::get("index.html")
    } else {
        FrontendAsset::get(path)
    };
    match file {
        Some(f) => {
            let mime = mime_from_path(path);
            Response::builder()
                .header(header::CONTENT_TYPE, mime)
                .header(header::CACHE_CONTROL, "no-store")
                .body(Body::from(f.data.into_owned()))
                .unwrap()
        }
        None => match FrontendAsset::get("index.html") {
            Some(index) => Response::builder()
                .header(header::CONTENT_TYPE, "text/html; charset=utf-8")
                .body(Body::from(index.data.into_owned()))
                .unwrap(),
            None => (StatusCode::NOT_FOUND, "not found").into_response(),
        },
    }
}

fn mime_from_path(path: &str) -> &'static str {
    if path.ends_with(".js") {
        "application/javascript; charset=utf-8"
    } else if path.ends_with(".css") {
        "text/css; charset=utf-8"
    } else if path.ends_with(".html") {
        "text/html; charset=utf-8"
    } else if path.ends_with(".svg") {
        "image/svg+xml"
    } else if path.ends_with(".png") {
        "image/png"
    } else if path.ends_with(".json") {
        "application/json; charset=utf-8"
    } else {
        "application/octet-stream"
    }
}

// --- Health & session ---

async fn handle_health(State(s): State<Arc<AppState>>) -> Response {
    let cfg = crate::config::read_toml(&s.config_path);
    let provider = cfg.get("model_provider").and_then(|v| v.as_str()).unwrap_or("openai");
    let model = cfg.get("model").and_then(|v| v.as_str()).unwrap_or("");
    let auth_exists = s.codex_home.join("auth.json").exists();
    ok_json(json!({
        "ok": true,
        "provider": { "providerId": provider },
        "model": model,
        "configPath": s.config_path.to_string_lossy(),
        "codexHome": s.codex_home.to_string_lossy(),
        "officialAuthPresent": auth_exists,
    }))
}

// 网关健康检查（FR-4.1）。
// 注意：`/health` 不走统一响应信封；按 04 §2 直接返回 {status, active_provider_id, access_mode}。
// 动态读 active provider（供前端顶栏同步：active 状态变更后刷新 /health）。
async fn handle_gateway_health(State(s): State<Arc<AppState>>) -> Response {
    let (active_id, access_mode) = match crate::providers::get_active(&s.providers_path) {
        Some(p) => (json!(p.id), serde_json::to_value(p.access_mode).unwrap_or(json!(null))),
        None => (json!(null), json!(null)),
    };
    ok_json(json!({
        "status": "ok",
        "active_provider_id": active_id,
        "access_mode": access_mode,
    }))
}

async fn handle_session(State(s): State<Arc<AppState>>) -> Response {
    if let Some(session) = crate::auth::load_session(&s.codex_home) {
        return ok_json(json!({ "authenticated": true, "user": session.user }));
    }
    // 过期:refresh_token 免验证码自动续期(「保存登录」;滑块登录只需一次)
    match crate::auth::refresh_session(&s.codex_home).await {
        Some(session) => ok_json(json!({ "authenticated": true, "user": session.user, "refreshed": true })),
        None => ok_json(json!({ "authenticated": false })),
    }
}

// --- Auth ---

async fn handle_auth_captcha(State(_s): State<Arc<AppState>>) -> Response {
    match crate::auth::fetch_captcha_settings().await {
        Ok(settings) => ok_json(settings),
        Err(_) => ok_json(json!({ "enabled": false, "provider": null })),
    }
}

async fn handle_auth_login(State(s): State<Arc<AppState>>, Json(body): Json<Value>) -> Response {
    let email = body.get("email").and_then(|v| v.as_str()).unwrap_or("");
    let password = body.get("password").and_then(|v| v.as_str()).unwrap_or("");
    // 腾讯滑块票据(前端人工完成后随请求带上;未开启验证码的站点为空)
    let ticket = body.get("captchaTicket").and_then(|v| v.as_str()).unwrap_or("");
    let randstr = body.get("captchaRandstr").and_then(|v| v.as_str()).unwrap_or("");
    if email.is_empty() || password.is_empty() {
        return err_json(StatusCode::BAD_REQUEST, "邮箱和密码不能为空");
    }
    match crate::auth::login(email, password, ticket, randstr).await {
        Ok(result) => {
            crate::auth::save_session(&s.codex_home, &result);
            ok_json(json!({ "authenticated": true, "user": result.user }))
        }
        Err(e) => err_json(StatusCode::UNAUTHORIZED, &format!("登录失败: {}", e)),
    }
}

async fn handle_auth_logout(State(s): State<Arc<AppState>>) -> Response {
    crate::auth::clear_session(&s.codex_home);
    ok_json(json!({ "ok": true }))
}

async fn handle_auth_remembered(State(s): State<Arc<AppState>>) -> Response {
    match crate::auth::load_remembered(&s.codex_home) {
        Some((email, password)) => ok_json(json!({ "remembered": true, "email": email, "password": password })),
        None => ok_json(json!({ "remembered": false, "email": "", "password": "" })),
    }
}

async fn handle_auth_remember(State(s): State<Arc<AppState>>, Json(body): Json<Value>) -> Response {
    let email = body.get("email").and_then(|v| v.as_str()).unwrap_or("");
    let password = body.get("password").and_then(|v| v.as_str()).unwrap_or("");
    crate::auth::save_remembered(&s.codex_home, email, password);
    ok_json(json!({ "ok": true }))
}

async fn handle_auth_forget(State(s): State<Arc<AppState>>) -> Response {
    crate::auth::clear_remembered(&s.codex_home);
    ok_json(json!({ "ok": true }))
}

async fn handle_key_groups(State(s): State<Arc<AppState>>) -> Response {
    match crate::auth::load_session(&s.codex_home) {
        Some(session) => match crate::auth::fetch_key_groups(&session.access_token).await {
            Ok(groups) => ok_json(groups),
            Err(e) => err_json(StatusCode::INTERNAL_SERVER_ERROR, &format!("获取分组失败: {}", e)),
        },
        None => err_json(StatusCode::UNAUTHORIZED, "请先登录 2xapi 账号"),
    }
}

// GET /api/auth/me —— 实时账号信息(余额);失败退回 session 快照
async fn handle_auth_me(State(s): State<Arc<AppState>>) -> Response {
    let session = match crate::auth::load_session(&s.codex_home) {
        Some(sess) => sess,
        None => match crate::auth::refresh_session(&s.codex_home).await {
            Some(sess) => sess,
            None => return err_json(StatusCode::UNAUTHORIZED, "请先登录 2xapi 账号"),
        },
    };
    match crate::auth::fetch_me(&session.access_token).await {
        Ok(user) if !user.is_null() => ok_json(json!({ "user": user })),
        _ => ok_json(json!({ "user": session.user })), // 外呼失败退回快照(余额可能滞后)
    }
}

// GET /api/auth/api-keys —— 一键导入数据源:用户 Key 列表 + relay 上游地址
async fn handle_auth_api_keys(State(s): State<Arc<AppState>>) -> Response {
    // session 过期自动续期(与 /api/session 同策略)
    let session = match crate::auth::load_session(&s.codex_home) {
        Some(sess) => Some(sess),
        None => crate::auth::refresh_session(&s.codex_home).await,
    };
    let Some(session) = session else {
        return err_json(StatusCode::UNAUTHORIZED, "请先登录 2xapi 账号");
    };
    let keys = match crate::auth::fetch_api_keys(&session.access_token).await {
        Ok(v) => {
            let d = v.get("data").cloned().unwrap_or(json!([]));
            // 部署版为 {items:[...]}(main 分支为直接数组)——两种都兼容
            if d.is_array() { d } else { d.get("items").cloned().unwrap_or(json!([])) }
        }
        Err(e) => return err_json(StatusCode::INTERNAL_SERVER_ERROR, &format!("获取 Key 列表失败: {}", e)),
    };
    let base_url = crate::auth::fetch_relay_base_url().await.unwrap_or_else(|_| "https://2xa.cc.cd".into());
    ok_json(json!({ "keys": keys, "baseUrl": base_url }))
}

// --- Providers（04 契约：统一信封 + 错误码）---

// GET /api/providers
async fn handle_providers_list(State(s): State<Arc<AppState>>) -> Response {
    let data = crate::providers::load(&s.providers_path);
    let providers: Vec<Value> = data.providers.iter().map(|p| crate::providers::public_provider(p)).collect();
    ok_env(json!({ "providers": providers, "active_provider_id": data.active_provider_id }))
}

// POST /api/providers
async fn handle_providers_create(State(s): State<Arc<AppState>>, Json(body): Json<Value>) -> Response {
    let input = crate::providers::value_to_input(&body);
    match crate::providers::create(&s.providers_path, input) {
        Ok(p) => ok_env(crate::providers::public_provider(&p)),
        Err(errs) => val_errs_env(&errs),
    }
}

// PUT /api/providers/:id
async fn handle_providers_update(State(s): State<Arc<AppState>>, Path(id): Path<String>, Json(body): Json<Value>) -> Response {
    let input = crate::providers::value_to_input(&body);
    match crate::providers::update(&s.providers_path, &id, input) {
        Ok(p) => ok_env(crate::providers::public_provider(&p)),
        Err(errs) => {
            if errs.len() == 1 && errs[0].field == "id" {
                err_env(StatusCode::NOT_FOUND, "E_NOT_FOUND", "供应商不存在", None)
            } else {
                val_errs_env(&errs)
            }
        }
    }
}

// DELETE /api/providers/:id
async fn handle_providers_delete(State(s): State<Arc<AppState>>, Path(id): Path<String>) -> Response {
    crate::providers::delete(&s.providers_path, &id);
    ok_env(json!({ "id": id, "deleted": true }))
}

// PUT /api/providers/reorder { ids }
async fn handle_providers_reorder(State(s): State<Arc<AppState>>, Json(body): Json<Value>) -> Response {
    let ids: Vec<String> = body
        .get("ids")
        .and_then(|v| v.as_array())
        .map(|a| a.iter().filter_map(|x| x.as_str().map(String::from)).collect())
        .unwrap_or_default();
    crate::providers::reorder(&s.providers_path, &ids);
    ok_env(json!({ "reordered": true, "count": ids.len() }))
}

// GET /api/providers/active
async fn handle_providers_active(State(s): State<Arc<AppState>>) -> Response {
    match crate::providers::get_active(&s.providers_path) {
        Some(p) => ok_env(crate::providers::public_provider(&p)),
        None => ok_env(Value::Null),
    }
}

// POST /api/providers/activate { id }
async fn handle_providers_activate(State(s): State<Arc<AppState>>, Json(body): Json<Value>) -> Response {
    let id = body.get("id").and_then(|v| v.as_str()).unwrap_or("");
    if id.is_empty() {
        return err_env(StatusCode::BAD_REQUEST, "E_BAD_REQUEST", "缺少 id", None);
    }
    match crate::config::activate(&s.config_path, &s.backup_dir, &s.providers_path, &s.codex_home, id) {
        Ok(r) => ok_env(json!({
            "active_provider_id": r.active_provider_id,
            "config_written": r.config_written,
            "auth_changed": r.auth_changed,
            "backup_created": r.backup_created,
        })),
        Err(e) => {
            if e.contains("不存在") {
                err_env(StatusCode::NOT_FOUND, "E_NOT_FOUND", &e, None)
            } else {
                err_env(StatusCode::INTERNAL_SERVER_ERROR, "E_INTERNAL", &e, None)
            }
        }
    }
}

// POST /api/providers/activate-official
async fn handle_providers_activate_official(State(s): State<Arc<AppState>>) -> Response {
    match crate::config::activate_official(&s.config_path, &s.backup_dir, &s.providers_path, &s.codex_home) {
        Ok(r) => ok_env(json!({
            "active_provider_id": Value::Null,
            "config_written": r.config_written,
            "auth_restored": r.auth_restored,
        })),
        Err(e) => err_env(StatusCode::INTERNAL_SERVER_ERROR, "E_INTERNAL", &e, None),
    }
}

// POST /api/providers/preview-config { id? 或临时 provider 对象 }
async fn handle_providers_preview(State(s): State<Arc<AppState>>, Json(body): Json<Value>) -> Response {
    let provider = if let Some(id) = body.get("id").and_then(|v| v.as_str()).filter(|x| !x.is_empty()) {
        match crate::providers::load(&s.providers_path).providers.into_iter().find(|p| p.id == id) {
            Some(p) => p,
            None => return err_env(StatusCode::NOT_FOUND, "E_NOT_FOUND", "供应商不存在", None),
        }
    } else {
        crate::providers::input_to_provider(crate::providers::value_to_input(&body))
    };
    match crate::config::preview_provider(&s.config_path, &s.codex_home, &provider) {
        Ok(o) => ok_env(json!({
            "config_toml": o.config_toml,
            "auth_action": o.auth_action,
            "auth_diff": o.auth_diff,
            "backup_will_create": o.backup_will_create,
        })),
        Err(e) => err_env(StatusCode::INTERNAL_SERVER_ERROR, "E_INTERNAL", &e, None),
    }
}

// POST /api/providers/fetch-models { id? 或 baseUrl+apiKey（新建未保存时也能拉）}
async fn handle_providers_fetch_models(State(s): State<Arc<AppState>>, Json(body): Json<Value>) -> Response {
    let id = body.get("id").and_then(|v| v.as_str()).unwrap_or("");
    let (base_url, api_key, write_back_id): (String, String, Option<String>) = if !id.is_empty() {
        let data = crate::providers::load(&s.providers_path);
        match data.providers.iter().find(|p| p.id == id).cloned() {
            Some(p) => (p.base_url, p.api_key, Some(id.to_string())),
            None => return err_env(StatusCode::NOT_FOUND, "E_NOT_FOUND", "供应商不存在", None),
        }
    } else {
        let b = body.get("baseUrl").or_else(|| body.get("base_url")).and_then(|v| v.as_str()).unwrap_or("").to_string();
        let k = body.get("apiKey").or_else(|| body.get("api_key")).and_then(|v| v.as_str()).unwrap_or("").to_string();
        (b, k, None)
    };
    if base_url.trim().is_empty() || api_key.trim().is_empty() {
        return err_env(StatusCode::BAD_REQUEST, "E_BAD_REQUEST", "需要 baseUrl + apiKey", None);
    }
    let probed = crate::probe::probe_endpoint(&base_url, &api_key).await;
    let models: Vec<crate::providers::ModelConfig> = probed
        .iter()
        .map(|(n, ctx)| crate::providers::ModelConfig { name: n.clone(), context_window: *ctx, ..Default::default() })
        .collect();
    // reasoning levels 探测已移出同步路径(2026-08-15 真机:2xa 上游对该探测挂满 15s 超时,
    // 把拉模型拖到 25s+,用户感知"拉取用不了")。levels 为空时 catalog 用默认 5 级,
    // 真机对话已验证无影响;显式探测挪到阶段 2 preflight。写回仅更新 models,保留已存 levels。
    let levels: Vec<String> = if let Some(wid) = &write_back_id {
        crate::providers::load(&s.providers_path)
            .providers.iter().find(|p| p.id == *wid)
            .and_then(|p| p.reasoning_levels.clone())
            .unwrap_or_default()
    } else { Vec::new() };
    if let Some(wid) = write_back_id {
        let mut data = crate::providers::load(&s.providers_path);
        if let Some(p) = data.providers.iter_mut().find(|p| p.id == wid) {
            p.models = models.clone();
        }
        let _ = crate::providers::store(&s.providers_path, &data);
    }
    ok_env(json!({ "models": models, "reasoning_levels": levels }))
}

// POST /api/providers/fetch-balance（01-D6 stub）
async fn handle_providers_fetch_balance() -> Response {
    ok_env(json!({ "balance": Value::Null, "note": "stub" }))
}

// POST /api/providers/diagnose { id }
async fn handle_providers_diagnose(State(s): State<Arc<AppState>>, Json(body): Json<Value>) -> Response {
    let id = body.get("id").and_then(|v| v.as_str()).unwrap_or("");
    let data = crate::providers::load(&s.providers_path);
    let provider = match data.providers.iter().find(|p| p.id == id).cloned() {
        Some(p) => p,
        None => return err_env(StatusCode::NOT_FOUND, "E_NOT_FOUND", "供应商不存在", None),
    };
    let result = crate::diagnose::diagnose(&provider).await;
    ok_env(serde_json::to_value(&result).unwrap_or(json!({})))
}

// --- Backups & history ---

async fn handle_backups(State(s): State<Arc<AppState>>) -> Response {
    let entries = crate::backups::list(&s.backup_dir);
    ok_json(json!({ "backups": entries }))
}

async fn handle_history(State(s): State<Arc<AppState>>) -> Response {
    let result = crate::history::inspect(&s.codex_home);
    ok_json(result)
}

// ── 加速线路(阶段 4,任务书 §五)─────────────────────────

fn accel_settings_path(codex_home: &std::path::Path) -> std::path::PathBuf {
    codex_home.join("2xapi-settings.json")
}

/// 读 `{codex_home}/2xapi-settings.json` 的 `accel` 段;缺失/非法 → 默认(off)。
/// 复用 sessions 读写该文件的模式(autoRepairBeforeHost 同文件,互不覆盖)。
pub fn load_accel_cfg(codex_home: &std::path::Path) -> AccelCfg {
    let raw = std::fs::read_to_string(accel_settings_path(codex_home)).unwrap_or_default();
    let v: Value = serde_json::from_str(&raw).unwrap_or(Value::Null);
    v.get("accel")
        .and_then(|a| serde_json::from_value(a.clone()).ok())
        .unwrap_or_default()
}

/// 写 `accel` 段(保留文件其余段)。
pub fn save_accel_cfg(codex_home: &std::path::Path, cfg: &AccelCfg) {
    std::fs::create_dir_all(codex_home).ok();
    let raw = std::fs::read_to_string(accel_settings_path(codex_home)).unwrap_or_default();
    let mut v: Value = serde_json::from_str(&raw).unwrap_or(json!({}));
    if let Some(o) = v.as_object_mut() {
        o.insert("accel".into(), serde_json::to_value(cfg).unwrap_or(json!({})));
    }
    let _ = std::fs::write(accel_settings_path(codex_home), serde_json::to_string_pretty(&v).unwrap_or_default());
}

/// scopeNote 纯函数(供单测):mode=official 且 active 供应商 base_url 未被任何线路命中
/// → 提示「不在官方线路范围,已直连」;命中或无 active → 空串;off/custom → 空串。
fn compute_scope_note(mode: &str, active_base_url: Option<&str>, lines: &[crate::acclines::AccLine]) -> String {
    if mode != "official" {
        return String::new();
    }
    let Some(base) = active_base_url else { return String::new() };
    if crate::acclines::match_line(base, lines).is_none() {
        "该供应商不在官方线路范围,已直连".to_string()
    } else {
        String::new()
    }
}

/// 加速路由错误信封:{ok:false, error} (与前端画师契约一致,非统一信封)。
fn err_accel(status: StatusCode, msg: &str) -> Response {
    (status, Json(json!({ "ok": false, "error": msg }))).into_response()
}

// GET /api/accel/state → {mode, customNode, lines, scopeNote}
async fn handle_accel_state(State(s): State<Arc<AppState>>) -> Response {
    let (mode, custom_node) = {
        let cfg = s.accel.lock().unwrap();
        (cfg.mode.clone(), cfg.custom_node.clone())
    };
    let lines: Vec<Value> = {
        let ls = s.health.lines.lock().unwrap();
        let table = s.health.table.lock().unwrap();
        ls.iter()
            .map(|l| {
                let h = table.get(&l.id);
                json!({
                    "id": l.id,
                    "name": l.name,
                    "endpoint": l.endpoint,
                    "scope": l.scope,
                    "priority": l.priority,
                    "enabled": l.enabled,
                    "latency": h.map(|h| h.latency_ms).unwrap_or(0),
                    "fails": h.map(|h| h.fails).unwrap_or(0),
                })
            })
            .collect()
    };
    let active_base = crate::providers::get_active(&s.providers_path).map(|p| p.base_url);
    let scope_note = {
        let ls = s.health.lines.lock().unwrap();
        compute_scope_note(&mode, active_base.as_deref(), &ls)
    };
    ok_json(json!({
        "mode": mode,
        "customNode": custom_node,
        "lines": lines,
        "scopeNote": scope_note,
    }))
}

// POST /api/accel/mode {mode}
async fn handle_accel_mode(State(s): State<Arc<AppState>>, Json(body): Json<Value>) -> Response {
    let mode = body.get("mode").and_then(|v| v.as_str()).unwrap_or("").trim().to_string();
    if !matches!(mode.as_str(), "off" | "official" | "custom") {
        return err_accel(StatusCode::BAD_REQUEST, "mode 须为 off/official/custom");
    }
    let mut cfg = s.accel.lock().unwrap();
    if mode == "custom" && cfg.custom_node.trim().is_empty() {
        return err_accel(StatusCode::BAD_REQUEST, "请先配置自定义加速节点");
    }
    cfg.mode = mode;
    save_accel_cfg(&s.codex_home, &cfg);
    ok_json(json!({ "ok": true }))
}

// POST /api/accel/custom-node {endpoint}
async fn handle_accel_custom_node(State(s): State<Arc<AppState>>, Json(body): Json<Value>) -> Response {
    let endpoint = body.get("endpoint").and_then(|v| v.as_str()).unwrap_or("").trim().to_string();
    if !(endpoint.starts_with("http://") || endpoint.starts_with("https://")) {
        return err_accel(StatusCode::BAD_REQUEST, "节点地址须为 http(s):// 开头");
    }
    let mut cfg = s.accel.lock().unwrap();
    cfg.custom_node = endpoint;
    save_accel_cfg(&s.codex_home, &cfg);
    ok_json(json!({ "ok": true }))
}

// POST /api/accel/test-node {endpoint}
async fn handle_accel_test_node(State(s): State<Arc<AppState>>, Json(body): Json<Value>) -> Response {
    let endpoint = body.get("endpoint").and_then(|v| v.as_str()).unwrap_or("").trim().to_string();
    if !(endpoint.starts_with("http://") || endpoint.starts_with("https://")) {
        return err_accel(StatusCode::BAD_REQUEST, "节点地址须为 http(s):// 开头");
    }
    let cred = crate::acclines::load_credentials(&s.codex_home);
    let outcome = crate::gateway::test_node_via(
        &endpoint,
        "https://api.2xa.cc.cd/models",
        cred.as_ref(),
        std::time::Duration::from_secs(5),
    )
    .await;
    match outcome {
        crate::gateway::NodeTestOutcome::Ok { latency_ms } => ok_json(json!({ "ok": true, "latencyMs": latency_ms })),
        crate::gateway::NodeTestOutcome::Timeout => err_accel(StatusCode::BAD_GATEWAY, "连不上:检查地址或网络"),
        crate::gateway::NodeTestOutcome::Auth => err_accel(StatusCode::BAD_GATEWAY, "节点凭证无效"),
        crate::gateway::NodeTestOutcome::Unavailable => err_accel(StatusCode::BAD_GATEWAY, "节点不可用"),
    }
}

// ── 历史会话管理(阶段 3,任务书 §四)─────────────────────

// GET /api/sessions?page=&size=&provider= → {total, items, db}
async fn handle_sessions_list(State(s): State<Arc<AppState>>, query: axum::extract::Query<Value>) -> Response {
    let page = query.get("page").and_then(|v| v.as_str()).and_then(|v| v.parse::<usize>().ok()).unwrap_or(1);
    let size = query.get("size").and_then(|v| v.as_str()).and_then(|v| v.parse::<usize>().ok()).unwrap_or(50);
    let provider = query.get("provider").and_then(|v| v.as_str()).unwrap_or("").to_string();
    ok_env(crate::sessions::list_sessions(&s.codex_home, page, size, &provider))
}

// POST /api/sessions/repair → {fixed, scanned}
async fn handle_sessions_repair(State(s): State<Arc<AppState>>) -> Response {
    ok_env(crate::sessions::repair_sessions(&s.codex_home, &s.backup_dir))
}

// GET/POST /api/sessions/settings
async fn handle_sessions_settings(State(s): State<Arc<AppState>>) -> Response {
    ok_env(crate::sessions::get_settings(&s.codex_home))
}
async fn handle_sessions_settings_set(State(s): State<Arc<AppState>>, Json(body): Json<Value>) -> Response {
    let v = body.get("autoRepairBeforeHost").and_then(|x| x.as_bool()).unwrap_or(true);
    ok_env(crate::sessions::set_settings(&s.codex_home, v))
}

async fn handle_config_snapshot(State(s): State<Arc<AppState>>) -> Response {
    match crate::config::create_snapshot(&s.config_path, &s.backup_dir) {
        Ok(entry) => ok_json(entry),
        Err(e) => err_json(StatusCode::INTERNAL_SERVER_ERROR, &e),
    }
}

async fn handle_config_restore(State(s): State<Arc<AppState>>, Json(body): Json<Value>) -> Response {
    let backup_path = body.get("backupPath").and_then(|v| v.as_str()).unwrap_or("");
    if !backup_path.ends_with(".toml") {
        return err_json(StatusCode::BAD_REQUEST, "只能恢复 TOML 配置备份");
    }
    match crate::config::restore(&s.config_path, backup_path) {
        Ok(_) => ok_json(json!({ "written": true, "restored": backup_path })),
        Err(e) => err_json(StatusCode::INTERNAL_SERVER_ERROR, &e),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use axum::http::Request;
    use tower::ServiceExt;

    fn dummy_state() -> AppState {
        AppState {
            config_path: PathBuf::from("/tmp/2xapi-m0-cfg.toml"),
            backup_dir: PathBuf::from("/tmp/2xapi-m0-bk"),
            providers_path: PathBuf::from("/tmp/2xapi-m0-providers.json"),
            codex_home: PathBuf::from("/tmp/2xapi-m0-codex-home"),
            launcher: Default::default(),
            health: std::sync::Arc::new(crate::acclines::HealthState::new(vec![])),
            accel: std::sync::Arc::new(std::sync::Mutex::new(AccelCfg::default())),
        }
    }

    /// M0 DoD③ 证据：GET /health 返回 200 + {status:"ok", active_provider_id:null, access_mode:null}
    #[tokio::test]
    async fn gateway_health_returns_ok_with_null_active() {
        let app = build_router(dummy_state());
        let resp = app
            .oneshot(Request::builder().uri("/health").body(Body::empty()).unwrap())
            .await
            .unwrap();
        assert_eq!(resp.status(), StatusCode::OK);
        let bytes = axum::body::to_bytes(resp.into_body(), usize::MAX).await.unwrap();
        let v: Value = serde_json::from_slice(&bytes).unwrap();
        assert_eq!(v["status"], "ok");
        assert_eq!(v["active_provider_id"], Value::Null);
        assert_eq!(v["access_mode"], Value::Null);
    }

    /// M0 DoD③ 实端口证据：真实绑定 127.0.0.1:8787 + 真实 HTTP GET（headless，无需启动 GUI）。
    /// 若 8787 已被占用（app 正在跑），跳过而不误报失败。
    #[tokio::test]
    async fn gateway_health_served_on_real_port_8787() {
        let listener = match tokio::net::TcpListener::bind("127.0.0.1:8787").await {
            Ok(l) => l,
            Err(e) => {
                eprintln!("[skip] 127.0.0.1:8787 已被占用({e})，假设 app 在跑");
                return;
            }
        };
        let app = build_router(dummy_state());
        tokio::spawn(async move {
            let _ = axum::serve(listener, app).await;
        });
        tokio::time::sleep(std::time::Duration::from_millis(150)).await;

        let resp = reqwest::get("http://127.0.0.1:8787/health").await.unwrap();
        assert_eq!(resp.status(), reqwest::StatusCode::OK);
        let v: Value = resp.json().await.unwrap();
        assert_eq!(v["status"], "ok");
        assert_eq!(v["active_provider_id"], Value::Null);
        assert_eq!(v["access_mode"], Value::Null);
    }

    // ── M4 路由（04 契约：统一信封 + 错误码）──

    fn unique_state(label: &str) -> (AppState, std::path::PathBuf) {
        use std::sync::atomic::{AtomicU64, Ordering};
        static N: AtomicU64 = AtomicU64::new(0);
        let n = N.fetch_add(1, Ordering::SeqCst);
        let root = std::env::temp_dir().join(format!("2xapi-m4-{label}-{}-{n}", std::process::id()));
        let _ = std::fs::remove_dir_all(&root);
        std::fs::create_dir_all(&root).unwrap();
        std::fs::create_dir_all(root.join("backups")).unwrap();
        let state = AppState {
            config_path: root.join("config.toml"),
            backup_dir: root.join("backups"),
            providers_path: root.join("providers.json"),
            codex_home: root.join("codex"),
            launcher: Default::default(),
            health: std::sync::Arc::new(crate::acclines::HealthState::new(vec![])),
            accel: std::sync::Arc::new(std::sync::Mutex::new(AccelCfg::default())),
        };
        (state, root)
    }

    async fn body_json(resp: axum::response::Response) -> Value {
        let bytes = axum::body::to_bytes(resp.into_body(), usize::MAX).await.unwrap();
        serde_json::from_slice(&bytes).unwrap()
    }

    #[tokio::test]
    async fn create_then_list_uses_envelope() {
        let (state, root) = unique_state("crud");
        let app = build_router(state.clone());
        let body = json!({"name":"T","baseUrl":"https://up.test","apiKey":"sk","model":"m","accessMode":"pure_api"});
        let resp = app
            .oneshot(
                Request::builder()
                    .method("POST")
                    .uri("/api/providers")
                    .header("content-type", "application/json")
                    .body(Body::from(body.to_string()))
                    .unwrap(),
            )
            .await
            .unwrap();
        assert_eq!(resp.status(), StatusCode::OK);
        let v = body_json(resp).await;
        assert_eq!(v["ok"], true);
        assert_eq!(v["data"]["name"], "T");
        assert_eq!(v["data"]["accessMode"], "pure_api");

        let app2 = build_router(state.clone());
        let resp = app2.oneshot(Request::builder().method("GET").uri("/api/providers").body(Body::empty()).unwrap()).await.unwrap();
        let v = body_json(resp).await;
        assert_eq!(v["data"]["providers"].as_array().unwrap().len(), 1);
        let _ = std::fs::remove_dir_all(&root);
    }

    #[tokio::test]
    async fn create_invalid_returns_422_validation() {
        let (state, root) = unique_state("valid");
        let app = build_router(state);
        let body = json!({"name":"","accessMode":"official"});
        let resp = app
            .oneshot(
                Request::builder()
                    .method("POST")
                    .uri("/api/providers")
                    .header("content-type", "application/json")
                    .body(Body::from(body.to_string()))
                    .unwrap(),
            )
            .await
            .unwrap();
        assert_eq!(resp.status(), StatusCode::UNPROCESSABLE_ENTITY);
        let v = body_json(resp).await;
        assert_eq!(v["ok"], false);
        assert_eq!(v["error"]["code"], "E_VALIDATION");
        assert!(v["error"]["fields"].as_array().unwrap().iter().any(|f| f == "name"));
        let _ = std::fs::remove_dir_all(&root);
    }

    #[tokio::test]
    async fn fetch_balance_stub() {
        let (state, root) = unique_state("bal");
        let app = build_router(state);
        let resp = app.oneshot(Request::builder().method("POST").uri("/api/providers/fetch-balance").body(Body::empty()).unwrap()).await.unwrap();
        assert_eq!(resp.status(), StatusCode::OK);
        let v = body_json(resp).await;
        assert_eq!(v["ok"], true);
        assert_eq!(v["data"]["balance"], Value::Null);
        assert_eq!(v["data"]["note"], "stub");
        let _ = std::fs::remove_dir_all(&root);
    }

    /// M7 后端 E2E：create → activate → /health 同步（验前端顶栏契约）。
    #[tokio::test]
    async fn e2e_create_activate_health_reflects() {
        let (state, root) = unique_state("e2e");
        // create
        let app = build_router(state.clone());
        let body = json!({"name":"E2E","baseUrl":"https://up.test","apiKey":"sk","model":"m","accessMode":"mixed"});
        let resp = app
            .oneshot(
                Request::builder().method("POST").uri("/api/providers").header("content-type", "application/json")
                    .body(Body::from(body.to_string())).unwrap(),
            )
            .await
            .unwrap();
        let v = body_json(resp).await;
        assert_eq!(v["ok"], true);
        let id = v["data"]["id"].as_str().unwrap().to_string();

        // activate（写 config.toml + 设 active）
        let app = build_router(state.clone());
        let resp = app
            .oneshot(
                Request::builder().method("POST").uri("/api/providers/activate").header("content-type", "application/json")
                    .body(Body::from(json!({"id": id}).to_string())).unwrap(),
            )
            .await
            .unwrap();
        let v = body_json(resp).await;
        assert_eq!(v["ok"], true);
        assert_eq!(v["data"]["active_provider_id"], id);

        // GET /health 反映 active
        let app = build_router(state.clone());
        let resp = app.oneshot(Request::builder().method("GET").uri("/health").body(Body::empty()).unwrap()).await.unwrap();
        let bytes = axum::body::to_bytes(resp.into_body(), usize::MAX).await.unwrap();
        let h: Value = serde_json::from_slice(&bytes).unwrap();
        assert_eq!(h["active_provider_id"], id);
        assert_eq!(h["access_mode"], "mixed");
        let _ = std::fs::remove_dir_all(&root);
    }

    /// 阶段 1 E2E（任务书 §1.3）：state → host → state → 换供应商 → direct 拒绝 → unhost → state 全链。
    #[tokio::test]
    async fn e2e_desktop_host_unhost_full_chain() {
        let (state, root) = unique_state("desk-e2e");
        std::fs::create_dir_all(&state.codex_home).unwrap();
        // 无官方登录：auth 只有别家 key（还原后应回到它）
        std::fs::write(state.codex_home.join("auth.json"), r#"{"OPENAI_API_KEY":"sk-old"}"#).unwrap();

        // 建两个供应商
        let app = build_router(state.clone());
        let resp = app
            .oneshot(
                Request::builder().method("POST").uri("/api/providers").header("content-type", "application/json")
                    .body(Body::from(json!({"name":"A","baseUrl":"https://a.test","apiKey":"sk-1","model":"m-a","accessMode":"mixed"}).to_string())).unwrap(),
            )
            .await
            .unwrap();
        let id1 = body_json(resp).await["data"]["id"].as_str().unwrap().to_string();

        let app = build_router(state.clone());
        let resp = app
            .oneshot(
                Request::builder().method("POST").uri("/api/providers").header("content-type", "application/json")
                    .body(Body::from(json!({"name":"B","baseUrl":"https://b.test","apiKey":"sk-2","model":"m-b","accessMode":"mixed"}).to_string())).unwrap(),
            )
            .await
            .unwrap();
        let id2 = body_json(resp).await["data"]["id"].as_str().unwrap().to_string();

        // 初始 state：未托管、无官方登录
        let app = build_router(state.clone());
        let v = body_json(app.oneshot(Request::builder().method("GET").uri("/api/desktop/state").body(Body::empty()).unwrap()).await.unwrap()).await;
        assert_eq!(v["ok"], true);
        assert_eq!(v["data"]["hasOfficial"], false);
        assert!(v["data"]["hosting"].is_null());

        // host gateway
        let app = build_router(state.clone());
        let resp = app
            .oneshot(
                Request::builder().method("POST").uri("/api/desktop/host").header("content-type", "application/json")
                    .body(Body::from(json!({"providerId": id1, "way": "gateway"}).to_string())).unwrap(),
            )
            .await
            .unwrap();
        assert_eq!(resp.status(), StatusCode::OK);
        let v = body_json(resp).await;
        assert_eq!(v["data"]["hosted"], true);

        let cfg_after_host = std::fs::read_to_string(&state.config_path).unwrap();
        assert!(cfg_after_host.contains("base_url = \"http://127.0.0.1:8787\""));
        assert!(cfg_after_host.contains("requires_openai_auth = false"));
        assert!(cfg_after_host.contains("experimental_bearer_token") == false);
        let auth = std::fs::read_to_string(state.codex_home.join("auth.json")).unwrap();
        assert!(auth.contains("sk-1"), "无账号应写供应商 key:\n{auth}");

        // state 反映托管
        let app = build_router(state.clone());
        let v = body_json(app.oneshot(Request::builder().method("GET").uri("/api/desktop/state").body(Body::empty()).unwrap()).await.unwrap()).await;
        assert_eq!(v["data"]["hosting"]["way"], "gateway");
        assert_eq!(v["data"]["hosting"]["providerId"], id1.as_str());

        // 换供应商：仅 set_active，config 不变
        let app = build_router(state.clone());
        let v = body_json(
            app.oneshot(
                Request::builder().method("POST").uri("/api/desktop/host").header("content-type", "application/json")
                    .body(Body::from(json!({"providerId": id2, "way": "gateway"}).to_string())).unwrap(),
            )
            .await
            .unwrap(),
        )
        .await;
        assert_eq!(v["data"]["switched"], true);
        let cfg_after_switch = std::fs::read_to_string(&state.config_path).unwrap();
        assert!(cfg_after_switch.contains("base_url = \"http://127.0.0.1:8787\""), "custom 段(网关指向)不变");
        assert!(cfg_after_switch.contains("model = \"m-b\""), "model 同步为新供应商(真机故障修复)");

        // direct 未开放：400 + E_DIRECT_UNAVAILABLE
        let app = build_router(state.clone());
        let resp = app
            .oneshot(
                Request::builder().method("POST").uri("/api/desktop/host").header("content-type", "application/json")
                    .body(Body::from(json!({"providerId": id2, "way": "direct"}).to_string())).unwrap(),
            )
            .await
            .unwrap();
        assert_eq!(resp.status(), StatusCode::BAD_REQUEST);
        let v = body_json(resp).await;
        assert_eq!(v["error"], "E_DIRECT_UNAVAILABLE");

        // unhost：回到干净态
        let app = build_router(state.clone());
        let v = body_json(
            app.oneshot(Request::builder().method("POST").uri("/api/desktop/unhost").body(Body::empty()).unwrap()).await.unwrap(),
        )
        .await;
        assert_eq!(v["data"]["restored"], true);

        let cfg_after = std::fs::read_to_string(&state.config_path).unwrap();
        assert!(!cfg_after.contains("[model_providers.custom]"));
        assert_eq!(
            std::fs::read_to_string(state.codex_home.join("auth.json")).unwrap(),
            r#"{"OPENAI_API_KEY":"sk-old"}"#,
            "auth 应恢复 host 前状态"
        );

        let app = build_router(state.clone());
        let v = body_json(app.oneshot(Request::builder().method("GET").uri("/api/desktop/state").body(Body::empty()).unwrap()).await.unwrap()).await;
        assert!(v["data"]["hosting"].is_null());
        let _ = std::fs::remove_dir_all(&root);
    }

    // ── 阶段 4 加速线路路由(任务书 §五)──

    fn accel_line(id: &str, scope: &[&str]) -> crate::acclines::AccLine {
        crate::acclines::AccLine {
            id: id.into(),
            name: id.into(),
            endpoint: "http://line.test:1".into(),
            scope: scope.iter().map(|s| s.to_string()).collect(),
            priority: 1,
            enabled: true,
            credential: None,
        }
    }

    fn accel_state(mode: &str, custom_node: &str, lines: Vec<crate::acclines::AccLine>) -> (AppState, std::path::PathBuf) {
        let (state, root) = unique_state("accel");
        *state.accel.lock().unwrap() = AccelCfg { mode: mode.into(), custom_node: custom_node.into() };
        state.health.set_lines(lines);
        (state, root)
    }

    async fn accel_get(app: &Router, uri: &str) -> Value {
        body_json(
            app.clone()
                .oneshot(Request::builder().method("GET").uri(uri).body(Body::empty()).unwrap())
                .await
                .unwrap(),
        )
        .await
    }

    async fn accel_post(app: &Router, uri: &str, body: &Value) -> (StatusCode, Value) {
        let resp = app
            .clone()
            .oneshot(
                Request::builder()
                    .method("POST")
                    .uri(uri)
                    .header("content-type", "application/json")
                    .body(Body::from(body.to_string()))
                    .unwrap(),
            )
            .await
            .unwrap();
        (resp.status(), body_json(resp).await)
    }

    #[tokio::test]
    async fn accel_state_default_off_no_scope_note() {
        let (state, root) = accel_state("off", "", vec![accel_line("l1", &["2xa.cc.cd"])]);
        let app = build_router(state.clone());
        let v = accel_get(&app, "/api/accel/state").await;
        assert_eq!(v["mode"], "off");
        assert_eq!(v["customNode"], "");
        assert_eq!(v["scopeNote"], "");
        let lines = v["lines"].as_array().unwrap();
        assert_eq!(lines.len(), 1);
        assert_eq!(lines[0]["id"], "l1");
        assert_eq!(lines[0]["latency"], 0, "未探测 latency 为 0");
        assert_eq!(lines[0]["fails"], 0);
        let _ = std::fs::remove_dir_all(&root);
    }

    #[tokio::test]
    async fn accel_mode_roundtrip_persists_to_settings() {
        let (state, root) = accel_state("off", "", vec![]);
        // 先建 codex_home,验证写 2xapi-settings.json
        std::fs::create_dir_all(&state.codex_home).unwrap();

        let (st, v) = accel_post(&build_router(state.clone()), "/api/accel/mode", &json!({"mode": "official"})).await;
        assert_eq!(st, StatusCode::OK);
        assert_eq!(v["ok"], true);
        // 落盘
        let saved: Value = serde_json::from_str(&std::fs::read_to_string(state.codex_home.join("2xapi-settings.json")).unwrap()).unwrap();
        assert_eq!(saved["accel"]["mode"], "official");
        assert_eq!(saved["accel"]["customNode"], "");
        // GET 反映
        let v = accel_get(&build_router(state.clone()), "/api/accel/state").await;
        assert_eq!(v["mode"], "official");
        // 往返回 off
        let (st, v) = accel_post(&build_router(state.clone()), "/api/accel/mode", &json!({"mode": "off"})).await;
        assert_eq!(st, StatusCode::OK);
        assert_eq!(v["ok"], true);
        let v = accel_get(&build_router(state.clone()), "/api/accel/state").await;
        assert_eq!(v["mode"], "off");
        let _ = std::fs::remove_dir_all(&root);
    }

    #[tokio::test]
    async fn accel_custom_node_roundtrip_persists() {
        let (state, root) = accel_state("off", "", vec![]);
        std::fs::create_dir_all(&state.codex_home).unwrap();
        let (st, v) = accel_post(&build_router(state.clone()), "/api/accel/custom-node", &json!({"endpoint": "http://node.test:1"})).await;
        assert_eq!(st, StatusCode::OK);
        assert_eq!(v["ok"], true);
        let v = accel_get(&build_router(state.clone()), "/api/accel/state").await;
        assert_eq!(v["customNode"], "http://node.test:1");
        let saved: Value = serde_json::from_str(&std::fs::read_to_string(state.codex_home.join("2xapi-settings.json")).unwrap()).unwrap();
        assert_eq!(saved["accel"]["customNode"], "http://node.test:1");
        // 非法地址 400
        let (st, v) = accel_post(&build_router(state.clone()), "/api/accel/custom-node", &json!({"endpoint": "ftp://bad"})).await;
        assert_eq!(st, StatusCode::BAD_REQUEST);
        assert_eq!(v["ok"], false);
        assert!(!v["error"].as_str().unwrap_or("").is_empty(), "400 应带人话 error");
        let _ = std::fs::remove_dir_all(&root);
    }

    #[tokio::test]
    async fn accel_mode_invalid_returns_400() {
        let (state, root) = accel_state("off", "", vec![]);
        let (st, v) = accel_post(&build_router(state), "/api/accel/mode", &json!({"mode": "bogus"})).await;
        assert_eq!(st, StatusCode::BAD_REQUEST);
        assert_eq!(v["ok"], false);
        let _ = std::fs::remove_dir_all(&root);
    }

    #[tokio::test]
    async fn accel_custom_mode_without_node_returns_400() {
        let (state, root) = accel_state("off", "", vec![]); // 无 custom_node
        let (st, v) = accel_post(&build_router(state.clone()), "/api/accel/mode", &json!({"mode": "custom"})).await;
        assert_eq!(st, StatusCode::BAD_REQUEST);
        assert!(v["error"].as_str().unwrap_or("").contains("自定义"), "应提示先配节点: {v}");
        // 已配节点 → 成功
        let (st, _v) = accel_post(&build_router(state.clone()), "/api/accel/custom-node", &json!({"endpoint": "http://node.test:1"})).await;
        assert_eq!(st, StatusCode::OK);
        let (st, v) = accel_post(&build_router(state.clone()), "/api/accel/mode", &json!({"mode": "custom"})).await;
        assert_eq!(st, StatusCode::OK);
        assert_eq!(v["ok"], true);
        let _ = std::fs::remove_dir_all(&root);
    }

    #[test]
    fn accel_scope_note_hit_and_miss() {
        let lines = vec![accel_line("l1", &["2xa.cc.cd"])];
        // official + 未命中 → 提示
        assert_eq!(
            compute_scope_note("official", Some("https://openai.com"), &lines),
            "该供应商不在官方线路范围,已直连"
        );
        // official + 命中 → 空串
        assert_eq!(compute_scope_note("official", Some("https://api.2xa.cc.cd"), &lines), "");
        // official + 无 active → 空串
        assert_eq!(compute_scope_note("official", None, &lines), "");
        // off / custom → 空串
        assert_eq!(compute_scope_note("off", Some("https://openai.com"), &lines), "");
        assert_eq!(compute_scope_note("custom", Some("https://openai.com"), &lines), "");
    }

    #[tokio::test]
    async fn accel_state_scope_note_from_active_provider() {
        let (state, root) = accel_state("official", "", vec![accel_line("l1", &["2xa.cc.cd"])]);
        // active provider 的 base_url 未命中(openai.com)
        let app = build_router(state.clone());
        let body = json!({"name":"Miss","baseUrl":"https://openai.com","apiKey":"sk","model":"m","accessMode":"mixed"});
        let resp = app.clone().oneshot(
            Request::builder().method("POST").uri("/api/providers").header("content-type", "application/json")
                .body(Body::from(body.to_string())).unwrap(),
        ).await.unwrap();
        assert_eq!(resp.status(), StatusCode::OK);
        let id = body_json(resp).await["data"]["id"].as_str().unwrap().to_string();
        let app = build_router(state.clone());
        let resp = app.clone().oneshot(
            Request::builder().method("POST").uri("/api/providers/activate").header("content-type", "application/json")
                .body(Body::from(json!({"id": id}).to_string())).unwrap(),
        ).await.unwrap();
        assert_eq!(resp.status(), StatusCode::OK);
        let v = accel_get(&build_router(state.clone()), "/api/accel/state").await;
        assert_eq!(v["scopeNote"], "该供应商不在官方线路范围,已直连");
        let _ = std::fs::remove_dir_all(&root);
    }
}

// ── 桌面版托管开关（阶段 1，任务书 §1.1）────────────────────

// GET /api/desktop/state
async fn handle_desktop_state(State(s): State<Arc<AppState>>) -> Response {
    ok_env(crate::desktop::state(&s.config_path, &s.providers_path, &s.codex_home))
}

// POST /api/desktop/host {providerId, way}
async fn handle_desktop_host(State(s): State<Arc<AppState>>, Json(body): Json<Value>) -> Response {
    let provider_id = body.get("providerId").and_then(|v| v.as_str()).unwrap_or("").trim();
    let way = body.get("way").and_then(|v| v.as_str()).unwrap_or("").trim();
    if provider_id.is_empty() || way.is_empty() {
        return err_env(StatusCode::BAD_REQUEST, "E_BAD_REQUEST", "缺少 providerId 或 way", None);
    }
    match crate::desktop::host(&s.config_path, &s.backup_dir, &s.codex_home, &s.providers_path, provider_id, way) {
        Ok(v) => ok_env(v),
        Err((status, code, msg)) => (
            StatusCode::from_u16(status).unwrap_or(StatusCode::BAD_REQUEST),
            Json(json!({ "error": code, "message": msg })),
        )
            .into_response(),
    }
}

// POST /api/desktop/unhost
async fn handle_desktop_unhost(State(s): State<Arc<AppState>>) -> Response {
    match crate::desktop::unhost(&s.config_path, &s.backup_dir, &s.codex_home, &s.providers_path) {
        Ok(v) => ok_env(v),
        Err((status, code, msg)) => (
            StatusCode::from_u16(status).unwrap_or(StatusCode::BAD_REQUEST),
            Json(json!({ "error": code, "message": msg })),
        )
            .into_response(),
    }
}

// ── Codex 启动器（M7，直连版）──────────────────────────────

// POST /api/launcher/preflight { providerId } | { baseUrl, apiKey } —— 测试连接(阶段 2,任务书 §三)
async fn handle_launcher_preflight(State(s): State<Arc<AppState>>, Json(body): Json<Value>) -> Response {
    let (base_url, api_key, model_hint): (String, String, String) = {
        let id = body.get("providerId").and_then(|v| v.as_str()).unwrap_or("");
        if !id.is_empty() {
            let data = crate::providers::load(&s.providers_path);
            match data.providers.iter().find(|p| p.id == id) {
                Some(p) => (p.base_url.clone(), p.api_key.clone(), p.model.clone()),
                None => return err_env(StatusCode::NOT_FOUND, "E_NOT_FOUND", "供应商不存在", None),
            }
        } else {
            let b = body.get("baseUrl").or_else(|| body.get("base_url")).and_then(|v| v.as_str()).unwrap_or("");
            let k = body.get("apiKey").or_else(|| body.get("api_key")).and_then(|v| v.as_str()).unwrap_or("");
            let m = body.get("model").and_then(|v| v.as_str()).unwrap_or("");
            if b.is_empty() || k.is_empty() {
                return err_env(StatusCode::BAD_REQUEST, "E_BAD_REQUEST", "需要 providerId,或 baseUrl+apiKey", None);
            }
            (b.to_string(), k.to_string(), m.to_string())
        }
    };

    let r = crate::probe::preflight(&base_url, &api_key, &model_hint).await;

    // 人话错误映射(任务书 §三):timeout/auth/notfound
    let human_error: Option<&str> = match r.error {
        Some("timeout") => Some("连不上:检查地址或网络"),
        Some("auth") => Some("Key 无效或未充值"),
        Some("notfound") => Some("地址不对,或该站不支持这个协议"),
        _ => None,
    };

    ok_env(json!({
        "keyOk": r.key_ok,
        "models": r.models.iter().map(|(n, c)| json!({ "name": n, "contextWindow": c })).collect::<Vec<_>>(),
        "responsesCompat": r.responses_compat,
        "chatOk": r.chat_ok,
        "latencyMs": r.latency_ms,
        "suggest": r.suggest,
        "error": r.error,          // 机器码:timeout|auth|notfound|null
        "message": human_error,    // 人话提示(前端展示;失败时高亮具体字段)
    }))
}

// POST /api/launcher/start { providerId?, baseUrl?, apiKey?, model?, projectDir, wireApi? }
async fn handle_launcher_start(State(s): State<Arc<AppState>>, Json(body): Json<Value>) -> Response {
    match crate::launcher::start(&s.launcher, &body, &s.providers_path) {
        Ok(data) => ok_env(data),
        Err(msg) => err_env(StatusCode::BAD_REQUEST, "E_LAUNCHER", &msg, None),
    }
}

// POST /api/launcher/stop { sessionId }
async fn handle_launcher_stop(State(s): State<Arc<AppState>>, Json(body): Json<Value>) -> Response {
    let id = body.get("sessionId").and_then(|v| v.as_str()).unwrap_or("");
    match crate::launcher::stop(&s.launcher, id) {
        Ok(data) => ok_env(data),
        Err(msg) => err_env(StatusCode::BAD_REQUEST, "E_LAUNCHER", &msg, None),
    }
}

// GET /api/launcher/status
async fn handle_launcher_status(State(s): State<Arc<AppState>>) -> Response {
    ok_env(crate::launcher::status(&s.launcher))
}

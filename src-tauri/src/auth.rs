use serde::{Deserialize, Serialize};
use serde_json::{json, Value};
use std::path::Path;

const MANAGEMENT_URL: &str = "https://2xapi.com/api/v1";
const MANAGEMENT_FALLBACK: &str = "https://2xa.cc.cd/api/v1";

#[derive(Clone, Serialize, Deserialize)]
pub struct Session {
    pub access_token: String,
    pub refresh_token: String,
    pub expires_at: i64,
    pub user: Value,
}

fn session_path(codex_home: &Path) -> std::path::PathBuf {
    codex_home.join("2xapi-session.json")
}
fn remembered_path(codex_home: &Path) -> std::path::PathBuf {
    codex_home.join("2xapi-remembered.json")
}

pub fn load_session(codex_home: &Path) -> Option<Session> {
    let raw = std::fs::read_to_string(session_path(codex_home)).ok()?;
    let s: Session = serde_json::from_str(&raw).ok()?;
    if s.expires_at > 0 && chrono::Local::now().timestamp_millis() > s.expires_at {
        return None;
    }
    Some(s)
}

pub fn save_session(codex_home: &Path, result: &LoginResult) {
    let session = Session {
        access_token: result.access_token.clone(),
        refresh_token: result.refresh_token.clone(),
        expires_at: chrono::Local::now().timestamp_millis() + result.expires_in * 1000,
        user: result.user.clone(),
    };
    let raw = serde_json::to_string_pretty(&session).unwrap_or_default();
    let _ = std::fs::write(session_path(codex_home), raw);
}

pub fn clear_session(codex_home: &Path) {
    let _ = std::fs::remove_file(session_path(codex_home));
}

pub fn load_remembered(codex_home: &Path) -> Option<(String, String)> {
    let raw = std::fs::read_to_string(remembered_path(codex_home)).ok()?;
    let v: Value = serde_json::from_str(&raw).ok()?;
    let email = v.get("email").and_then(|e| e.as_str()).unwrap_or("").to_string();
    let password = v.get("password").and_then(|p| p.as_str()).unwrap_or("").to_string();
    if email.is_empty() { None } else { Some((email, password)) }
}

pub fn save_remembered(codex_home: &Path, email: &str, password: &str) {
    let raw = serde_json::to_string_pretty(&json!({ "email": email, "password": password })).unwrap_or_default();
    let _ = std::fs::write(remembered_path(codex_home), raw);
}

pub fn clear_remembered(codex_home: &Path) {
    let _ = std::fs::remove_file(remembered_path(codex_home));
}

pub struct LoginResult {
    pub access_token: String,
    pub refresh_token: String,
    pub expires_in: i64,
    pub user: Value,
}

fn api_client() -> reqwest::Client {
    reqwest::Client::builder()
        .timeout(std::time::Duration::from_secs(20))
        .no_proxy()
        .build()
        .expect("failed to build HTTP client")
}

async fn xapi_request(path: &str, method: reqwest::Method, body: &Value, access_token: &str) -> Result<Value, String> {
    let urls = [MANAGEMENT_URL, MANAGEMENT_FALLBACK];
    let mut last_err = String::new();
    for base in &urls {
        let url = format!("{}{}", base.trim_end_matches('/'), path);
        let host = base.trim_start_matches("https://").split('/').next().unwrap_or(base);
        let mut req = api_client().request(method.clone(), &url);
        if !access_token.is_empty() {
            req = req.header("Authorization", format!("Bearer {}", access_token));
        }
        if body != &json!({}) {
            req = req.json(body);
        }
        match req.send().await {
            Ok(resp) => {
                let status = resp.status();
                let json: Value = resp.json().await.unwrap_or(json!({}));
                if status.is_success() {
                    return Ok(json);
                }
                let err = json.get("error").or_else(|| json.get("message"))
                    .and_then(|e| e.as_str()).unwrap_or("unknown error");
                last_err = format!("[{}] {}", host, err);
            }
            Err(e) => last_err = format!("[{}] 连接失败: {}", host, e),
        }
    }
    Err(last_err)
}

/// 验证码设置(Sub2API settings/public 为扁平字段:tencent_captcha_enabled / tencent_captcha_app_id)。
/// 旧实现按嵌套 captcha 段读,恒得 enabled=false —— 已按 Sub2API 实际结构修正。
pub async fn fetch_captcha_settings() -> Result<Value, String> {
    let result = xapi_request("/settings/public?timezone=UTC", reqwest::Method::GET, &json!({}), "").await?;
    let d = result.get("data").unwrap_or(&result);
    let app_id = d
        .get("tencent_captcha_app_id")
        .map(|v| match v {
            Value::String(s) => s.clone(),
            Value::Number(n) => n.to_string(),
            _ => String::new(),
        })
        .unwrap_or_default();
    Ok(json!({
        "enabled": d.get("tencent_captcha_enabled").and_then(|v| v.as_bool()).unwrap_or(false),
        "appId": app_id,
    }))
}

pub async fn login(email: &str, password: &str, captcha_ticket: &str, captcha_randstr: &str) -> Result<LoginResult, String> {
    // Sub2API LoginRequest 字段:tencent_captcha_ticket / tencent_captcha_randstr(源码 auth_handler.go)
    let body = json!({
        "email": email,
        "password": password,
        "tencent_captcha_ticket": captcha_ticket,
        "tencent_captcha_randstr": captcha_randstr,
    });
    let result = xapi_request("/auth/login", reqwest::Method::POST, &body, "").await?;

    if result.get("requires_2fa").and_then(|v| v.as_bool()).unwrap_or(false) {
        return Err("requires 2fa".into());
    }

    let access_token = result.get("access_token")
        .or_else(|| result.get("accessToken"))
        .and_then(|v| v.as_str())
        .ok_or("登录响应未包含 access token")?
        .to_string();

    let refresh_token = result.get("refresh_token")
        .and_then(|v| v.as_str())
        .unwrap_or("")
        .to_string();

    let expires_in = result.get("expires_in")
        .and_then(|v| v.as_i64())
        .unwrap_or(3600);

    let user = result.get("user").cloned().unwrap_or(json!({}));

    Ok(LoginResult { access_token, refresh_token, expires_in, user })
}

pub async fn fetch_key_groups(access_token: &str) -> Result<Value, String> {
    xapi_request("/groups", reqwest::Method::GET, &json!({}), access_token).await
}

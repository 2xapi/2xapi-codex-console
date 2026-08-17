//! http 型插件接线(超融合 A 线二期,媒体组 M3 契约 v1)。
//!
//! - 登记:`POST /api/plugins {endpoint}` → GET {endpoint}/manifest 校验 → registry plugin 条目
//! - 调用:`POST /api/plugins/:id/invoke {op,…}` → POST {endpoint}/invoke,按 manifest.timeout_ms 断流;
//!   插件侧失败也走 200 包错误({ok:false,error:{code,message,human}})→ 透传;
//!   5xx/超时 → 网关侧人话 E_MEDIA_PLUGIN_DOWN / E_MEDIA_PLUGIN_TIMEOUT(媒体关卡人话原则)
//! - 四挂载点声明永久冻结:media_parse | tool_exec | proto_convert | dispatch

use crate::registry;
use axum::http::StatusCode;
use axum::response::{IntoResponse, Response};
use serde_json::{json, Map, Value};
use std::sync::Arc;

pub const MOUNTS: &[&str] = &["media_parse", "tool_exec", "proto_convert", "dispatch"];
const DEFAULT_TIMEOUT_MS: u64 = 30_000;

fn err_env(status: StatusCode, code: &str, msg: &str) -> Response {
    crate::server::err_env(status, code, msg, None)
}

/// 透传插件原始 JSON(不套统一信封——invoke 契约是插件响应原样到达调用方)。
fn raw_json(status: StatusCode, v: &Value) -> Response {
    (status, axum::Json(v.clone())).into_response()
}

fn plugin_client() -> reqwest::Client {
    reqwest::Client::builder()
        .no_proxy()
        .build()
        .unwrap_or_default()
}

/// 校验 manifest(M3 契约):必填 id/name/version/mount/input/output;mount 须四挂载点之一。
pub fn validate_manifest(m: &Map<String, Value>) -> Result<(), String> {
    let req = ["id", "name", "version", "mount", "input", "output"];
    for k in req {
        if m.get(k).is_none() {
            return Err(format!("manifest 缺必填字段 {k}"));
        }
    }
    let id = m["id"].as_str().unwrap_or("");
    if id.is_empty() || !id.chars().all(|c| c.is_ascii_alphanumeric() || c == '-') {
        return Err("id 须为字母数字连字符".into());
    }
    let mount = m["mount"].as_str().unwrap_or("");
    if !MOUNTS.contains(&mount) {
        return Err(format!("mount 须为 {MOUNTS:?} 之一,得到 {mount:?}"));
    }
    Ok(())
}

/// 拉取并校验 manifest;Ok = manifest 全量(含 endpoint 回填)。
pub async fn fetch_manifest(endpoint: &str) -> Result<Map<String, Value>, String> {
    let base = endpoint.trim_end_matches('/');
    let resp = plugin_client()
        .get(format!("{base}/manifest"))
        .timeout(std::time::Duration::from_secs(10))
        .send()
        .await
        .map_err(|e| format!("插件不可达: {e}"))?;
    if !resp.status().is_success() {
        return Err(format!("manifest 拉取失败(HTTP {})", resp.status()));
    }
    let v: Value = resp
        .json()
        .await
        .map_err(|e| format!("manifest 非合法 JSON: {e}"))?;
    let mut m = v.as_object().cloned().ok_or("manifest 须为 JSON 对象")?;
    validate_manifest(&m)?;
    m.insert("endpoint".into(), json!(base));
    Ok(m)
}

/// 调用插件 /invoke,透传其 200 包错误;网络层失败人话映射。
pub async fn invoke_plugin(entry: &registry::Entry, body: &Value) -> Response {
    let Some(endpoint) = entry.meta.get("endpoint").and_then(|v| v.as_str()) else {
        return err_env(
            StatusCode::INTERNAL_SERVER_ERROR,
            "E_PLUGIN_NO_ENDPOINT",
            "插件条目缺 endpoint",
        );
    };
    let timeout_ms = entry
        .meta
        .get("timeout_ms")
        .and_then(|v| v.as_u64())
        .unwrap_or(DEFAULT_TIMEOUT_MS);
    let base = endpoint.trim_end_matches('/');
    match plugin_client()
        .post(format!("{base}/invoke"))
        .timeout(std::time::Duration::from_millis(timeout_ms))
        .json(body)
        .send()
        .await
    {
        Ok(r) => {
            let status =
                StatusCode::from_u16(r.status().as_u16()).unwrap_or(StatusCode::BAD_GATEWAY);
            match r.json::<Value>().await {
                // 契约:成功/插件侧失败都是 200 + {ok:bool,...};网关原样透传
                Ok(v) => raw_json(status, &v),
                Err(e) => err_env(
                    StatusCode::BAD_GATEWAY,
                    "E_PLUGIN_BAD_RESP",
                    &format!("插件响应非 JSON: {e}"),
                ),
            }
        }
        Err(e) if e.is_timeout() => err_env(
            StatusCode::GATEWAY_TIMEOUT,
            "E_MEDIA_PLUGIN_TIMEOUT",
            &format!("插件响应超时(上限 {}ms),请检查插件服务", timeout_ms),
        ),
        Err(e) => err_env(
            StatusCode::BAD_GATEWAY,
            "E_MEDIA_PLUGIN_DOWN",
            &format!("插件不可达: {e}"),
        ),
    }
}

// ── HTTP handlers(server.rs build_router 注册)───────────────

use axum::extract::{Path, State};
use axum::routing::{get, post};
use axum::{Json, Router};

pub fn routes() -> Router<Arc<crate::server::AppState>> {
    Router::new()
        .route("/api/plugins", get(handle_list).post(handle_register))
        .route("/api/plugins/:id", axum::routing::delete(handle_remove))
        .route("/api/plugins/:id/toggle", post(handle_toggle))
        .route("/api/plugins/:id/invoke", post(handle_invoke))
}

async fn handle_list(State(s): State<Arc<crate::server::AppState>>) -> Response {
    let entries = registry::list_json(&s.codex_home);
    let plugins: Vec<Value> = entries["entries"]
        .as_array()
        .map(|a| {
            a.iter()
                .filter(|e| e["kind"] == "plugin")
                .cloned()
                .collect()
        })
        .unwrap_or_default();
    raw_json(StatusCode::OK, &json!({ "plugins": plugins }))
}

async fn handle_register(
    State(s): State<Arc<crate::server::AppState>>,
    Json(body): Json<Value>,
) -> Response {
    let Some(endpoint) = body.get("endpoint").and_then(|v| v.as_str()).map(str::trim) else {
        return err_env(
            StatusCode::BAD_REQUEST,
            "E_ARGS",
            "endpoint 必填(http://host:port)",
        );
    };
    if !(endpoint.starts_with("http://") || endpoint.starts_with("https://")) {
        return err_env(
            StatusCode::BAD_REQUEST,
            "E_ARGS",
            "endpoint 须为 http(s):// 开头",
        );
    }
    let source_id = body.get("sourceId").and_then(|v| v.as_str()).unwrap_or("");
    match fetch_manifest(endpoint).await {
        Ok(mut m) => {
            if !source_id.is_empty() {
                m.insert("source_id".into(), json!(source_id));
            }
            let id = m
                .get("id")
                .and_then(|v| v.as_str())
                .unwrap_or("")
                .to_string();
            registry::upsert_plugin(&s.codex_home, &m);
            raw_json(
                StatusCode::OK,
                &json!({ "ok": true, "id": id, "manifest": m }),
            )
        }
        Err(e) => err_env(StatusCode::BAD_REQUEST, "E_PLUGIN_MANIFEST", &e),
    }
}

async fn handle_remove(
    State(s): State<Arc<crate::server::AppState>>,
    Path(id): Path<String>,
) -> Response {
    registry::remove(&s.codex_home, &id);
    raw_json(StatusCode::OK, &json!({ "ok": true }))
}

async fn handle_toggle(
    State(s): State<Arc<crate::server::AppState>>,
    Path(id): Path<String>,
    Json(body): Json<Value>,
) -> Response {
    let enabled = body
        .get("enabled")
        .and_then(|v| v.as_bool())
        .unwrap_or(true);
    registry::set_enabled(&s.codex_home, &id, enabled);
    raw_json(
        StatusCode::OK,
        &json!({ "ok": true, "id": id, "enabled": enabled }),
    )
}

async fn handle_invoke(
    State(s): State<Arc<crate::server::AppState>>,
    Path(id): Path<String>,
    Json(body): Json<Value>,
) -> Response {
    let Some(entry) = registry::get_plugin(&s.codex_home, &id) else {
        return err_env(StatusCode::NOT_FOUND, "E_NO_PLUGIN", "插件不存在");
    };
    if !entry.enabled {
        return err_env(
            StatusCode::FORBIDDEN,
            "E_PLUGIN_DISABLED",
            "插件已停用,请先在插件管理启用",
        );
    }
    invoke_plugin(&entry, &body).await
}

#[cfg(test)]
mod tests {
    use super::*;
    use tokio::io::{AsyncReadExt, AsyncWriteExt};
    use tokio::net::TcpListener;

    /// mock 插件:/manifest(M3 形态)+ /invoke(成功 op 与未知 op 两形态)。
    async fn spawn_plugin() -> String {
        let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
        let addr = listener.local_addr().unwrap();
        tokio::spawn(async move {
            loop {
                let Ok((mut sock, _)) = listener.accept().await else {
                    break;
                };
                tokio::spawn(async move {
                    let mut buf = vec![0u8; 65536];
                    let Ok(n) = sock.read(&mut buf).await else {
                        return;
                    };
                    let req = String::from_utf8_lossy(&buf[..n]).into_owned();
                    let (path, body) = match req.find("\r\n\r\n") {
                        Some(i) => {
                            let first = &req[..i];
                            let p = first.split_whitespace().nth(1).unwrap_or("");
                            (p.to_string(), req[i + 4..].to_string())
                        }
                        None => (String::new(), String::new()),
                    };
                    let resp_body = if path == "/manifest" {
                        r#"{"id":"ffmpeg-frame-extract","name":"抽帧","version":"1.0.0","mount":"media_parse","input":{"required":["media_url"],"properties":{}},"output":{"properties":{}},"timeout_ms":2000}"#.to_string()
                    } else if body.contains("\"known\"") {
                        r#"{"ok":true,"data":{"image_b64":"x"}}"#.to_string()
                    } else {
                        r#"{"ok":false,"error":{"code":"E_OP","message":"unknown","human":"未知操作"}}"#.to_string()
                    };
                    let resp = format!(
                        "HTTP/1.1 200 OK\r\nContent-Type: application/json\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{}",
                        resp_body.len(),
                        resp_body
                    );
                    let _ = sock.write_all(resp.as_bytes()).await;
                });
            }
        });
        format!("http://{addr}")
    }

    fn entry_with(endpoint: &str, timeout_ms: u64) -> registry::Entry {
        let mut meta = Map::new();
        meta.insert("endpoint".into(), json!(endpoint));
        meta.insert("timeout_ms".into(), json!(timeout_ms));
        registry::Entry {
            id: "t".into(),
            kind: registry::Kind::Plugin,
            provider_id: None,
            model: None,
            enabled: true,
            meta,
        }
    }

    #[test]
    fn manifest_validation() {
        let good: Map<String, Value> = serde_json::from_value(json!({
            "id":"a-b","name":"n","version":"1.0.0","mount":"media_parse","input":{},"output":{}
        }))
        .unwrap();
        assert!(validate_manifest(&good).is_ok());
        assert!(validate_manifest(&{
            let mut m = good.clone();
            m.remove("mount");
            m
        })
        .is_err());
        assert!(validate_manifest(&{
            let mut m = good.clone();
            m.insert("mount".into(), json!("nope"));
            m
        })
        .is_err());
    }

    #[tokio::test]
    async fn register_validate_and_invoke_roundtrip() {
        let base = spawn_plugin().await;
        // 登记:manifest 拉取+校验+endpoint 回填
        let m = fetch_manifest(&base).await.unwrap();
        assert_eq!(m["id"], "ffmpeg-frame-extract");
        assert_eq!(m["endpoint"], base);
        // 调用:成功 op 透传
        let e = entry_with(&base, 2000);
        let resp = invoke_plugin(&e, &json!({"op":"known"})).await;
        assert_eq!(resp.status(), StatusCode::OK);
        let bytes = axum::body::to_bytes(resp.into_body(), usize::MAX)
            .await
            .unwrap();
        let v: Value = serde_json::from_slice(&bytes).unwrap();
        assert_eq!(v["ok"], true);
        // 插件侧失败也 200 包错误,透传
        let resp = invoke_plugin(&e, &json!({"op":"zzz"})).await;
        assert_eq!(resp.status(), StatusCode::OK);
        let v: Value = serde_json::from_slice(
            &axum::body::to_bytes(resp.into_body(), usize::MAX)
                .await
                .unwrap(),
        )
        .unwrap();
        assert_eq!(v["ok"], false);
        assert_eq!(v["error"]["human"], "未知操作");
    }

    #[tokio::test]
    async fn invoke_timeout_maps_human_error() {
        let e = entry_with("http://127.0.0.1:9", 300); // 不可达端口
        let resp = invoke_plugin(&e, &json!({"op":"x"})).await;
        assert_eq!(resp.status(), StatusCode::BAD_GATEWAY);
        let v: Value = serde_json::from_slice(
            &axum::body::to_bytes(resp.into_body(), usize::MAX)
                .await
                .unwrap(),
        )
        .unwrap();
        assert_eq!(v["error"]["code"], "E_MEDIA_PLUGIN_DOWN");
    }
}

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
        .route("/api/plugin-market", get(handle_market_list))
        .route("/api/plugin-market/sources", post(handle_market_source_add))
        .route(
            "/api/plugin-market/sources/:id",
            axum::routing::delete(handle_market_source_remove),
        )
        .route("/api/plugin-market/install", post(handle_market_install))
}

async fn handle_list(State(s): State<Arc<crate::server::AppState>>) -> Response {
    let entries = registry::list_json(&s.codex_home);
    // plugin=http 型,tool=官方内置能力(C 段条目靠此可见);model 条目(探测登记)不混入
    let plugins: Vec<Value> = entries["entries"]
        .as_array()
        .map(|a| {
            a.iter()
                .filter(|e| e["kind"] == "plugin" || e["kind"] == "tool")
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
    // 官方内置 tool:按条目 id 分发本机实现(注册表统一管理,走同一 invoke 入口,非特权通道)
    if entry.kind == registry::Kind::Tool
        && entry
            .meta
            .get("builtin")
            .and_then(|v| v.as_bool())
            .unwrap_or(false)
    {
        return match entry.id.as_str() {
            "ffmpeg-frame-extract" => builtin_frame_extract(&s.codex_home, &body).await,
            "image-describe" => crate::media_tools::image_describe(&s, &body).await,
            "image-generate" => crate::media_tools::image_generate(&s, &body).await,
            "image-edit" => crate::media_tools::image_edit(&s, &body).await,
            _ => err_env(StatusCode::NOT_FOUND, "E_NO_PLUGIN", "未知内置能力"),
        };
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

// ── 插件市场(M5 定案:静态 JSON 清单;官方源内置,第三方仅 http(s) 清单拉取)──

pub const OFFICIAL_SOURCE: &str = "official";

/// 官方源内置清单:官方内置能力(tool 条目,invoke 分发本机实现,不设 http 依赖)。
fn official_market() -> Value {
    // 官方内置能力条目(媒体组 C 段,2026-08-17 一手实测定案):
    // - 图出三态:generations/edits 端点在(模型限 gpt-image 系),当前 2xa Key 组未开通权限 → desc 如实注
    // - ASR/TTS 上游路由级 404 → 不列条目(不造假能力)
    json!({
        "schema_version": 1,
        "source": { "id": OFFICIAL_SOURCE, "name": "官方源" },
        "plugins": [
            {
                "id": "ffmpeg-frame-extract",
                "name": "ffmpeg 抽帧",
                "version": "1.0.0",
                "mount": "media_parse",
                "builtin": true,
                "desc": "从视频抽一帧为 JPEG 并入媒体暂存(管内 URL 引用,可喂识图)",
                "short_desc": "视频抽帧",
                "input": { "required": ["media_url"], "properties": { "media_url": "string", "t": "number" } },
                "output": { "properties": { "media_url": "string", "mime": "string" } },
                "timeout_ms": 60000
            },
            {
                "id": "image-describe",
                "name": "识图",
                "version": "1.0.0",
                "mount": "media_parse",
                "builtin": true,
                "desc": "把图片交给上游多模态模型理解,返回文字描述;按 image_in 能力标签前置拦截,不支持识图的模型人话报错",
                "short_desc": "图片转文字描述",
                "input": { "required": ["media_url"], "properties": { "media_url": "string", "prompt": "string", "provider_id": "string", "model": "string" } },
                "output": { "properties": { "text": "string", "provider_id": "string", "model": "string" } },
                "timeout_ms": 120000
            },
            {
                "id": "image-generate",
                "name": "文生图",
                "version": "1.0.0",
                "mount": "media_parse",
                "builtin": true,
                "desc": "按文字描述生成图片(gpt-image 系)入媒体暂存,回管内 URL;上游 Key 组需开通图生成权限,未开通时人话提示",
                "short_desc": "文字生成图片",
                "input": { "required": ["prompt"], "properties": { "prompt": "string", "model": "string", "provider_id": "string", "size": "string", "n": "number" } },
                "output": { "properties": { "media_urls": "string[]", "mime": "string" } },
                "timeout_ms": 240000
            },
            {
                "id": "image-edit",
                "name": "图编辑",
                "version": "1.0.0",
                "mount": "media_parse",
                "builtin": true,
                "desc": "按编辑指令修改暂存原图(gpt-image 系),产出新图入媒体暂存;上游 Key 组需开通图生成权限",
                "short_desc": "按指令编辑图片",
                "input": { "required": ["media_url", "prompt"], "properties": { "media_url": "string", "prompt": "string", "model": "string", "provider_id": "string", "size": "string" } },
                "output": { "properties": { "media_url": "string", "mime": "string" } },
                "timeout_ms": 240000
            }
        ]
    })
}

fn sources_path(codex_home: &std::path::Path) -> std::path::PathBuf {
    codex_home.join("market-sources.json")
}

fn load_sources(codex_home: &std::path::Path) -> Vec<Value> {
    std::fs::read_to_string(sources_path(codex_home))
        .ok()
        .and_then(|raw| serde_json::from_str::<Value>(&raw).ok())
        .and_then(|v| v.get("sources").and_then(|s| s.as_array().cloned()))
        .unwrap_or_default()
}

fn save_sources(codex_home: &std::path::Path, sources: &[Value]) {
    let p = sources_path(codex_home);
    let body = json!({ "version": 1, "sources": sources });
    let tmp = p.with_extension("json.tmp");
    if std::fs::write(&tmp, body.to_string()).is_ok() {
        let _ = std::fs::rename(&tmp, &p);
    }
}

/// 拉取第三方源清单并校验(M5:schema_version 须识别;失败拒收整源)。
pub async fn fetch_market(url: &str) -> Result<Value, String> {
    let resp = plugin_client()
        .get(url.trim_end_matches('/'))
        .timeout(std::time::Duration::from_secs(10))
        .send()
        .await
        .map_err(|e| format!("清单拉取失败: {e}"))?;
    if !resp.status().is_success() {
        return Err(format!("清单拉取失败(HTTP {})", resp.status()));
    }
    let v: Value = resp.json().await.map_err(|e| format!("清单非 JSON: {e}"))?;
    if v.get("schema_version").and_then(|x| x.as_u64()) != Some(1) {
        return Err("schema_version 不识别(须为 1),整源拒收".into());
    }
    if v.get("plugins").and_then(|p| p.as_array()).is_none() {
        return Err("清单缺 plugins 数组".into());
    }
    Ok(v)
}

// ── 内置抽帧 tool(官方转正:注册表 tool 条目,invoke 分发本机 ffmpeg)──

/// 从媒体暂存 URL / 本地路径抽一帧。ffmpeg 缺失/失败 → 200 包错误(插件契约同形)。
async fn builtin_frame_extract(codex_home: &std::path::Path, body: &Value) -> Response {
    let Some(media_url) = body
        .get("media_url")
        .and_then(|v| v.as_str())
        .map(str::trim)
    else {
        return raw_json(
            StatusCode::OK,
            &json!({"ok": false, "error": {"code": "E_ARGS", "message": "media_url 必填", "human": "请提供视频的媒体地址"}}),
        );
    };
    let t = body.get("t").and_then(|v| v.as_f64()).unwrap_or(0.0);
    // 源定位:网关暂存 URL(尾段 {id}.{ext})或绝对本地路径;id 限 hex 防路径穿越
    let src: String = match media_url.rsplit('/').next() {
        Some(tail)
            if tail.contains('.')
                && tail
                    .split('.')
                    .next()
                    .unwrap_or("")
                    .chars()
                    .all(|c| c.is_ascii_hexdigit()) =>
        {
            let p = crate::media::media_root(codex_home).join(tail);
            if !p.exists() {
                return raw_json(
                    StatusCode::OK,
                    &json!({"ok": false, "error": {"code": "E_MEDIA_NOT_FOUND", "message": "media_url 不在暂存", "human": "该媒体地址不在本机暂存,请先上传"}}),
                );
            }
            p.to_string_lossy().into_owned()
        }
        _ if media_url.starts_with('/') && !media_url.contains("..") => media_url.to_string(),
        _ => {
            return raw_json(
                StatusCode::OK,
                &json!({"ok": false, "error": {"code": "E_ARGS", "message": "media_url 形态不支持", "human": "支持网关暂存地址(/media/{id}.{ext})或本机绝对路径"}}),
            );
        }
    };
    let home = codex_home.to_path_buf();
    let extracted = tokio::task::spawn_blocking(move || {
        std::process::Command::new("ffmpeg")
            .args([
                "-hide_banner",
                "-loglevel",
                "error",
                "-ss",
                &format!("{t}"),
                "-i",
                &src,
                "-frames:v",
                "1",
                "-f",
                "image2",
                "-",
            ])
            .output()
    })
    .await;
    let out = match extracted {
        Ok(Ok(o)) if o.status.success() => o.stdout,
        Ok(Ok(o)) => {
            return raw_json(
                StatusCode::OK,
                &json!({"ok": false, "error": {"code": "E_MEDIA_FFMPEG", "message": String::from_utf8_lossy(&o.stderr).chars().take(200).collect::<String>(), "human": "ffmpeg 抽帧失败,请确认视频文件可解码"}}),
            );
        }
        Ok(Err(e)) => {
            return raw_json(
                StatusCode::OK,
                &json!({"ok": false, "error": {"code": "E_MEDIA_FFMPEG", "message": e.to_string(), "human": "本机未安装 ffmpeg 或无法执行,请安装 ffmpeg(如 brew install ffmpeg)"}}),
            );
        }
        Err(e) => {
            return err_env(
                StatusCode::INTERNAL_SERVER_ERROR,
                "E_INTERNAL",
                &format!("内部执行失败: {e}"),
            );
        }
    };
    match crate::media::store_upload(&home, &out, "image/jpeg", "ffmpeg-frame-extract") {
        Ok(item) => raw_json(
            StatusCode::OK,
            &json!({"ok": true, "data": {"media_url": format!("/media/{}.{}", item.id, item.ext), "mime": item.mime}}),
        ),
        Err((code, msg)) => raw_json(
            StatusCode::OK,
            &json!({"ok": false, "error": {"code": code, "message": msg, "human": "抽帧结果存储失败"}}),
        ),
    }
}

// ── 市场 handlers ────────────────────────────────────────────

async fn handle_market_list(State(s): State<Arc<crate::server::AppState>>) -> Response {
    let mut sources = vec![json!({ "id": OFFICIAL_SOURCE, "name": "官方源", "builtin": true })];
    sources.extend(load_sources(&s.codex_home));
    raw_json(
        StatusCode::OK,
        &json!({ "sources": sources, "official": official_market() }),
    )
}

async fn handle_market_source_add(
    State(s): State<Arc<crate::server::AppState>>,
    Json(body): Json<Value>,
) -> Response {
    let id = body
        .get("id")
        .and_then(|v| v.as_str())
        .unwrap_or("")
        .trim()
        .to_string();
    let name = body
        .get("name")
        .and_then(|v| v.as_str())
        .unwrap_or("")
        .trim()
        .to_string();
    let url = body
        .get("url")
        .and_then(|v| v.as_str())
        .unwrap_or("")
        .trim()
        .to_string();
    if id.is_empty()
        || name.is_empty()
        || !(url.starts_with("http://") || url.starts_with("https://"))
    {
        return err_env(
            StatusCode::BAD_REQUEST,
            "E_ARGS",
            "id/name/url 必填(url 须 http(s) 清单地址)",
        );
    }
    if id == OFFICIAL_SOURCE
        || load_sources(&s.codex_home)
            .iter()
            .any(|x| x["id"] == id.as_str())
    {
        return err_env(StatusCode::BAD_REQUEST, "E_SOURCE_DUP", "源 id 已存在");
    }
    match fetch_market(&url).await {
        Ok(_) => {
            let mut sources = load_sources(&s.codex_home);
            sources.push(json!({ "id": id, "name": name, "url": url }));
            save_sources(&s.codex_home, &sources);
            raw_json(StatusCode::OK, &json!({ "ok": true, "id": id }))
        }
        Err(e) => err_env(StatusCode::BAD_REQUEST, "E_SOURCE_FETCH", &e),
    }
}

async fn handle_market_source_remove(
    State(s): State<Arc<crate::server::AppState>>,
    Path(id): Path<String>,
) -> Response {
    if id == OFFICIAL_SOURCE {
        return err_env(StatusCode::FORBIDDEN, "E_OFFICIAL", "官方源不可删除");
    }
    let mut sources = load_sources(&s.codex_home);
    let before = sources.len();
    sources.retain(|x| x["id"] != id.as_str());
    save_sources(&s.codex_home, &sources);
    raw_json(
        StatusCode::OK,
        &json!({ "ok": true, "removed": before != sources.len() }),
    )
}

/// 安装:官方源→内置条目直接登记;第三方源→拉清单→逐条目 manifest 校验→登记。
async fn handle_market_install(
    State(s): State<Arc<crate::server::AppState>>,
    Json(body): Json<Value>,
) -> Response {
    let source_id = body
        .get("sourceId")
        .and_then(|v| v.as_str())
        .unwrap_or(OFFICIAL_SOURCE)
        .to_string();
    let plugin_id = body
        .get("pluginId")
        .and_then(|v| v.as_str())
        .unwrap_or("")
        .to_string();
    if plugin_id.is_empty() {
        return err_env(StatusCode::BAD_REQUEST, "E_ARGS", "pluginId 必填");
    }
    let manifest: Map<String, Value> = if source_id == OFFICIAL_SOURCE {
        match official_market()["plugins"]
            .as_array()
            .and_then(|a| a.iter().find(|p| p["id"] == plugin_id.as_str()).cloned())
            .and_then(|p| p.as_object().cloned())
        {
            Some(m) => m,
            None => return err_env(StatusCode::NOT_FOUND, "E_NO_PLUGIN", "官方源无该插件"),
        }
    } else {
        let Some(src) = load_sources(&s.codex_home)
            .into_iter()
            .find(|x| x["id"] == source_id.as_str())
            .and_then(|x| x["url"].as_str().map(String::from))
        else {
            return err_env(StatusCode::NOT_FOUND, "E_NO_SOURCE", "源不存在,请先添加");
        };
        let market = match fetch_market(&src).await {
            Ok(v) => v,
            Err(e) => return err_env(StatusCode::BAD_REQUEST, "E_SOURCE_FETCH", &e),
        };
        let Some(entry) = market["plugins"]
            .as_array()
            .and_then(|a| a.iter().find(|p| p["id"] == plugin_id.as_str()).cloned())
        else {
            return err_env(StatusCode::NOT_FOUND, "E_NO_PLUGIN", "该源无此插件");
        };
        let Some(endpoint) = entry.get("endpoint").and_then(|v| v.as_str()) else {
            return err_env(
                StatusCode::BAD_REQUEST,
                "E_PLUGIN_MANIFEST",
                "清单条目缺 endpoint",
            );
        };
        let mut m = match fetch_manifest(endpoint).await {
            Ok(m) => m,
            Err(e) => return err_env(StatusCode::BAD_REQUEST, "E_PLUGIN_MANIFEST", &e),
        };
        m.insert("source_id".into(), json!(source_id));
        m
    };
    // 内置 tool 条目做 manifest 校验;http 型已在 fetch_manifest 内校验
    if manifest
        .get("builtin")
        .and_then(|v| v.as_bool())
        .unwrap_or(false)
    {
        if let Err(e) = validate_manifest(&manifest) {
            return err_env(StatusCode::BAD_REQUEST, "E_PLUGIN_MANIFEST", &e);
        }
    }
    let pid = manifest
        .get("id")
        .and_then(|v| v.as_str())
        .unwrap_or("")
        .to_string();
    registry::upsert_plugin(&s.codex_home, &manifest);
    raw_json(StatusCode::OK, &json!({ "ok": true, "id": pid }))
}

#[cfg(test)]
mod market_tests {
    use super::*;

    fn tmp_home(tag: &str) -> std::path::PathBuf {
        let h = std::env::temp_dir().join(format!("2xapi-mkt-{tag}-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&h);
        std::fs::create_dir_all(&h).unwrap();
        h
    }

    #[test]
    fn official_install_creates_tool_entry() {
        let home = tmp_home("kind");
        let entry_manifest = official_market()["plugins"][0]
            .as_object()
            .cloned()
            .unwrap();
        validate_manifest(&entry_manifest).unwrap();
        registry::upsert_plugin(&home, &entry_manifest);
        let e = registry::get_plugin(&home, "ffmpeg-frame-extract").unwrap();
        assert_eq!(e.kind, registry::Kind::Tool, "内置能力=tool 条目");
        assert!(e.enabled);
        let _ = std::fs::remove_dir_all(&home);
    }

    /// ffmpeg 端到端(本机无 ffmpeg 时验证人话错误分支)。
    #[tokio::test]
    async fn builtin_frame_extract_end_to_end() {
        let home = tmp_home("ffmpeg");
        let has_ffmpeg = std::process::Command::new("ffmpeg")
            .args(["-hide_banner", "-loglevel", "error", "-version"])
            .output()
            .map(|o| o.status.success())
            .unwrap_or(false);
        if !has_ffmpeg {
            let resp = builtin_frame_extract(&home, &json!({"media_url": "/media/x.mp4"})).await;
            let v: Value = serde_json::from_slice(
                &axum::body::to_bytes(resp.into_body(), usize::MAX)
                    .await
                    .unwrap(),
            )
            .unwrap();
            assert_eq!(v["ok"], false);
            assert_eq!(
                v["error"]["code"], "E_MEDIA_NOT_FOUND",
                "缺 ffmpeg 机器也应先过暂存校验"
            );
            let _ = std::fs::remove_dir_all(&home);
            return;
        }
        // ffmpeg testsrc 生成 1s mp4 → 入暂存 → invoke 抽帧
        let mp4 = std::env::temp_dir().join(format!("2xapi-fe-{}.mp4", std::process::id()));
        let gen = std::process::Command::new("ffmpeg")
            .args([
                "-hide_banner",
                "-loglevel",
                "error",
                "-f",
                "lavfi",
                "-i",
                "testsrc=duration=1:size=128x96:rate=10",
                "-y",
            ])
            .arg(&mp4)
            .output()
            .unwrap();
        assert!(gen.status.success(), "生成测试视频失败: {:?}", gen.stderr);
        let data = std::fs::read(&mp4).unwrap();
        let item = crate::media::store_upload(&home, &data, "video/mp4", "test").unwrap();
        let resp = builtin_frame_extract(
            &home,
            &json!({"media_url": format!("http://127.0.0.1:8787/media/{}.{}", item.id, item.ext), "t": 0.5}),
        )
        .await;
        let v: Value = serde_json::from_slice(
            &axum::body::to_bytes(resp.into_body(), usize::MAX)
                .await
                .unwrap(),
        )
        .unwrap();
        assert_eq!(v["ok"], true, "抽帧应成功: {v}");
        let url = v["data"]["media_url"].as_str().unwrap();
        assert!(url.starts_with("/media/") && url.ends_with(".jpg"), "{url}");
        // 产物落暂存可读
        let id_ext = url.trim_start_matches("/media/");
        let out = std::fs::read(crate::media::media_root(&home).join(id_ext)).unwrap();
        assert_eq!(&out[..3], b"\xFF\xD8\xFF", "JPEG magic");
        let _ = std::fs::remove_file(&mp4);
        let _ = std::fs::remove_dir_all(&home);
    }

    /// 坏输入两形态:media_url 缺失 / 非 hex id 路径穿越拒绝。
    #[tokio::test]
    async fn builtin_frame_extract_bad_input() {
        let home = tmp_home("bad");
        for (body, code) in [
            (json!({}), "E_ARGS"),
            (
                json!({"media_url": "/media/../../etc/passwd.mp4"}),
                "E_ARGS",
            ),
        ] {
            let resp = builtin_frame_extract(&home, &body).await;
            let v: Value = serde_json::from_slice(
                &axum::body::to_bytes(resp.into_body(), usize::MAX)
                    .await
                    .unwrap(),
            )
            .unwrap();
            assert_eq!(v["error"]["code"], code, "{body}");
        }
        let _ = std::fs::remove_dir_all(&home);
    }

    /// 市场清单 schema 校验(mock 清单源:合法/坏 schema_version 两形态)。
    #[tokio::test]
    async fn fetch_market_schema_validation() {
        use tokio::io::{AsyncReadExt, AsyncWriteExt};
        use tokio::net::TcpListener;
        async fn serve(body: &'static str) -> String {
            let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
            let addr = listener.local_addr().unwrap();
            tokio::spawn(async move {
                loop {
                    let Ok((mut sock, _)) = listener.accept().await else {
                        break;
                    };
                    let body = body;
                    tokio::spawn(async move {
                        let mut buf = [0u8; 1024];
                        let _ = sock.read(&mut buf).await;
                        let resp = format!(
                            "HTTP/1.1 200 OK\r\nContent-Type: application/json\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{}",
                            body.len(), body
                        );
                        let _ = sock.write_all(resp.as_bytes()).await;
                    });
                }
            });
            format!("http://{addr}/list.json")
        }
        let good = serve(r#"{"schema_version":1,"source":{"id":"s1"},"plugins":[{"id":"p","endpoint":"http://x"}]}"#).await;
        let v = fetch_market(&good).await.unwrap();
        assert_eq!(v["plugins"][0]["id"], "p");
        let bad = serve(r#"{"schema_version":2,"plugins":[]}"#).await;
        assert!(
            fetch_market(&bad).await.is_err(),
            "schema_version 不识别整源拒收"
        );
    }
}

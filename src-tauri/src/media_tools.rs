//! 多模态能力条目(媒体组 C 段,2026-08-17 一手实测定案):
//! - 识图 image-describe:暂存图片 → 上游 chat 通道带图请求(image_url 形态)→ 文本;
//!   按 image_in 能力标签前置拦截(No→人话拒,与网关媒体关卡同口径,Yes/Unknown 放行)。
//! - 文生图 image-generate / 图编辑 image-edit:上游 /v1/images/generations|edits,
//!   模型限 gpt-image 系(上游实测 dall-e 会被 400 拒);产物 b64 → 入媒体暂存回管内 URL。
//!   当前 2xa Key 组未开通图生成权限(403 permission_error)——人话透出,后台开通即用。
//! - ASR(/v1/audio/transcriptions)与 TTS(/v1/audio/speech)上游路由级 404,条目不做。
//!
//! 全部为注册表 tool 条目,与 ffmpeg 抽帧同一 invoke 入口非特权;错误一律 200 包错误+human。

use axum::http::StatusCode;
use axum::response::{IntoResponse, Response};
use serde_json::{json, Value};
use std::path::{Path, PathBuf};

use crate::providers::{AccessMode, Provider};
use crate::server::AppState;

fn ok_data(data: Value) -> Response {
    (
        StatusCode::OK,
        axum::Json(json!({ "ok": true, "data": data })),
    )
        .into_response()
}

/// M3 契约:插件侧失败也走 200 包错误,human 必填。
fn tool_err(code: &str, message: &str, human: &str) -> Response {
    (
        StatusCode::OK,
        axum::Json(
            json!({ "ok": false, "error": { "code": code, "message": message, "human": human } }),
        ),
    )
        .into_response()
}

fn api_url(base_url: &str, path: &str) -> String {
    let base = base_url.trim_end_matches('/');
    if base.ends_with("/v1") {
        format!("{base}{path}")
    } else {
        format!("{base}/v1{path}")
    }
}

/// 端点覆盖:body.api(内置工具故障转移链注入的备用端点)优先,否则供应商 base_url。
fn api_base<'a>(p: &'a Provider, body: &'a Value) -> &'a str {
    body.get("api")
        .and_then(|v| v.as_str())
        .filter(|s| !s.trim().is_empty())
        .unwrap_or(&p.base_url)
}

/// 供应商解析:body.provider_id 指定,否则 active。
fn resolve_provider(s: &AppState, body: &Value) -> Result<Provider, Box<Response>> {
    let pid = body
        .get("provider_id")
        .and_then(|v| v.as_str())
        .unwrap_or("");
    let p = if pid.is_empty() {
        crate::providers::get_active(&s.providers_path)
    } else {
        crate::providers::list(&s.providers_path)
            .into_iter()
            .find(|p| p.id == pid)
    };
    let Some(p) = p else {
        return Err(Box::new(tool_err(
            "E_NO_PROVIDER",
            "provider not found",
            "未找到可用供应商,请先在控制台启用",
        )));
    };
    if p.access_mode == AccessMode::Official {
        return Err(Box::new(tool_err(
            "E_OFFICIAL",
            "official mode",
            "Official 模式供应商不走上游多模态能力,请切换为网关模式供应商",
        )));
    }
    Ok(p)
}

fn provider_key(p: &Provider) -> Result<String, Box<Response>> {
    crate::keypool::effective_keys(p)
        .into_iter()
        .next()
        .ok_or_else(|| Box::new(tool_err("E_NO_KEY", "no api key", "该供应商未配置 API Key")))
}

/// 暂存定位:网关暂存 URL(尾段 {hex}.{ext})或本机绝对路径;id 限 hex 防穿越(与抽帧同款)。
fn media_path(codex_home: &Path, media_url: &str) -> Result<PathBuf, Box<Response>> {
    let media_url = media_url.trim();
    match media_url.rsplit('/').next() {
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
            if p.exists() {
                Ok(p)
            } else {
                Err(Box::new(tool_err(
                    "E_MEDIA_NOT_FOUND",
                    "media_url 不在暂存",
                    "该媒体不在本机暂存,请先上传",
                )))
            }
        }
        _ if media_url.starts_with('/') && !media_url.contains("..") => {
            Ok(PathBuf::from(media_url))
        }
        _ => Err(Box::new(tool_err(
            "E_ARGS",
            "media_url 形态不支持",
            "支持网关暂存地址(/media/{id}.{ext})或本机绝对路径",
        ))),
    }
}

fn mime_of(path: &Path) -> &'static str {
    match path.extension().and_then(|e| e.to_str()).unwrap_or("") {
        "png" => "image/png",
        "jpg" | "jpeg" => "image/jpeg",
        "webp" => "image/webp",
        "gif" => "image/gif",
        _ => "application/octet-stream",
    }
}

/// base64 标准编码(与 media.rs 手写解码同风格,不引依赖)。
fn b64_encode(data: &[u8]) -> String {
    const T: &[u8; 64] = b"ABCDEFGHIJKLMNOPQRSTUVWXYZabcdefghijklmnopqrstuvwxyz0123456789+/";
    let mut out = String::with_capacity(data.len().div_ceil(3) * 4);
    for chunk in data.chunks(3) {
        let b = [
            *chunk.first().unwrap_or(&0),
            *chunk.get(1).unwrap_or(&0),
            *chunk.get(2).unwrap_or(&0),
        ];
        let n = (u32::from(b[0]) << 16) | (u32::from(b[1]) << 8) | u32::from(b[2]);
        out.push(T[(n >> 18 & 63) as usize] as char);
        out.push(T[(n >> 12 & 63) as usize] as char);
        out.push(if chunk.len() > 1 {
            T[(n >> 6 & 63) as usize] as char
        } else {
            '='
        });
        out.push(if chunk.len() > 2 {
            T[(n & 63) as usize] as char
        } else {
            '='
        });
    }
    out
}

/// 图出端点错误 → 人话(403 权限/404 无端点/其余带原文)。
fn human_for_image_status(status: StatusCode, body_text: &str) -> String {
    if status == StatusCode::FORBIDDEN
        && (body_text.contains("not enabled") || body_text.contains("permission"))
    {
        return "上游已识别到图像端点,但当前 Key 组未开通图像生成权限(Image generation is not enabled),请到上游后台(如 2xa)为该账号组开通后重试".into();
    }
    if status == StatusCode::NOT_FOUND {
        return "上游无该图像端点(404),该供应商不支持图像生成/编辑".into();
    }
    format!("上游返回 HTTP {status}:{body_text}")
        .chars()
        .take(200)
        .collect()
}

fn content_text(v: &Value) -> Option<String> {
    v.get("choices")?
        .get(0)?
        .get("message")?
        .get("content")?
        .as_str()
        .map(String::from)
}

/// 识图:暂存图片 → 上游多模态 chat → 文本描述。
pub async fn image_describe(s: &AppState, body: &Value) -> Response {
    let Some(media_url) = body.get("media_url").and_then(|v| v.as_str()) else {
        return tool_err("E_ARGS", "media_url 必填", "请提供图片的媒体地址");
    };
    let prompt = body
        .get("prompt")
        .and_then(|v| v.as_str())
        .unwrap_or("请描述这张图片的内容。");
    let p = match resolve_provider(s, body) {
        Ok(p) => p,
        Err(r) => return *r,
    };
    let model = body
        .get("model")
        .and_then(|v| v.as_str())
        .map(String::from)
        .unwrap_or_else(|| p.model.clone());
    if model.is_empty() {
        return tool_err(
            "E_NO_MODEL",
            "model empty",
            "该供应商未设置默认模型,请在调用参数中指定 model",
        );
    }
    // 媒体关卡(与网关同口径):标签 No 拦,Unknown/Yes 放行
    if crate::capprobe::tri_of(&s.codex_home, &p.id, &model, "image_in") == crate::capprobe::Tri::No
    {
        return tool_err(
            "E_CAP_IMAGE_IN",
            "image_in=no",
            "该模型经探测不支持识图(看不到图片),请换多模态模型,或在供应商详情卡手动更正能力标签后重试",
        );
    }
    let path = match media_path(&s.codex_home, media_url) {
        Ok(p) => p,
        Err(r) => return *r,
    };
    let bytes = match std::fs::read(&path) {
        Ok(b) => b,
        Err(e) => {
            return tool_err(
                "E_MEDIA_READ",
                &e.to_string(),
                "读取媒体文件失败,请确认文件存在且可读",
            )
        }
    };
    let data_url = format!("data:{};base64,{}", mime_of(&path), b64_encode(&bytes));
    let key = match provider_key(&p) {
        Ok(k) => k,
        Err(r) => return *r,
    };
    let client = reqwest::Client::builder()
        .timeout(std::time::Duration::from_secs(120))
        .build()
        .unwrap_or_default();
    let url = api_url(api_base(&p, body), "/chat/completions");
    let resp = client
        .post(&url)
        .header(reqwest::header::AUTHORIZATION, format!("Bearer {key}"))
        .json(&json!({
            "model": model, "max_tokens": 1024, "stream": false,
            "messages": [{ "role": "user", "content": [
                { "type": "text", "text": prompt },
                { "type": "image_url", "image_url": { "url": data_url } }
            ]}]
        }))
        .send()
        .await;
    let resp = match resp {
        Ok(r) => r,
        Err(e) => {
            return tool_err(
                "E_UPSTREAM",
                &e.to_string(),
                "上游不可达,请检查网络或供应商地址",
            )
        }
    };
    let status = resp.status();
    let v: Value = match resp.json().await {
        Ok(v) => v,
        Err(e) => {
            return tool_err(
                "E_UPSTREAM",
                &e.to_string(),
                &format!("上游响应非 JSON: {e}"),
            )
        }
    };
    if !status.is_success() {
        return tool_err(
            "E_UPSTREAM",
            &format!("HTTP {status}"),
            &human_for_image_status(status, &v.to_string()),
        );
    }
    match content_text(&v) {
        Some(text) if !text.is_empty() => {
            ok_data(json!({ "text": text, "provider_id": p.id, "model": model }))
        }
        _ => tool_err(
            "E_EMPTY",
            "empty content",
            "模型未返回文本内容(可能纯推理输出或上游吞图),请重试或换模型",
        ),
    }
}

/// 图出响应 → 暂存回管内 URL:b64_json 优先;url 形态(dall-e/官方 CDN)下载后落暂存。
async fn store_image_out(
    client: &reqwest::Client,
    s: &AppState,
    v: &Value,
    origin: &str,
) -> Result<Vec<String>, Box<Response>> {
    let mut urls = Vec::new();
    let items = v
        .get("data")
        .and_then(|d| d.as_array())
        .cloned()
        .unwrap_or_default();
    for item in items {
        let bytes: Vec<u8> = if let Some(b64) = item.get("b64_json").and_then(|x| x.as_str()) {
            match crate::media::b64_decode(b64) {
                Ok(b) => b,
                Err(e) => {
                    return Err(Box::new(tool_err(
                        "E_MEDIA_B64",
                        &e,
                        "上游图片数据(base64)解码失败",
                    )))
                }
            }
        } else if let Some(u) = item.get("url").and_then(|x| x.as_str()) {
            match client.get(u).send().await {
                Ok(r) if r.status().is_success() => match r.bytes().await {
                    Ok(b) => b.to_vec(),
                    Err(e) => {
                        return Err(Box::new(tool_err(
                            "E_UPSTREAM",
                            &e.to_string(),
                            "下载上游图片失败",
                        )))
                    }
                },
                Ok(r) => {
                    return Err(Box::new(tool_err(
                        "E_UPSTREAM",
                        &format!("HTTP {}", r.status()),
                        &format!("下载上游图片失败(HTTP {})", r.status()),
                    )));
                }
                Err(e) => {
                    return Err(Box::new(tool_err(
                        "E_UPSTREAM",
                        &e.to_string(),
                        "下载上游图片失败",
                    )))
                }
            }
        } else {
            return Err(Box::new(tool_err(
                "E_UPSTREAM",
                "no image in data",
                "上游 200 但 data 内无 b64_json/url 图片",
            )));
        };
        match crate::media::store_upload(&s.codex_home, &bytes, "image/png", origin) {
            Ok(m) => urls.push(format!("/media/{}.{}", m.id, m.ext)),
            Err((code, msg)) => {
                return Err(Box::new(tool_err(code, &msg, "生成图片入媒体暂存失败")))
            }
        }
    }
    if urls.is_empty() {
        return Err(Box::new(tool_err(
            "E_UPSTREAM",
            "data empty",
            "上游未返回任何图片",
        )));
    }
    Ok(urls)
}

/// 文生图:{prompt, size?, n?, model?, provider_id?}(model 默认 gpt-image-1)。
pub async fn image_generate(s: &AppState, body: &Value) -> Response {
    let Some(prompt) = body.get("prompt").and_then(|v| v.as_str()).map(str::trim) else {
        return tool_err("E_ARGS", "prompt 必填", "请提供图片的文字描述");
    };
    if prompt.is_empty() {
        return tool_err("E_ARGS", "prompt 空", "请提供图片的文字描述");
    }
    let p = match resolve_provider(s, body) {
        Ok(p) => p,
        Err(r) => return *r,
    };
    let model = body
        .get("model")
        .and_then(|v| v.as_str())
        .unwrap_or("gpt-image-1")
        .to_string();
    let key = match provider_key(&p) {
        Ok(k) => k,
        Err(r) => return *r,
    };
    let mut req_body = json!({ "model": model, "prompt": prompt });
    for k in ["size", "n"] {
        if let Some(x) = body.get(k) {
            req_body[k] = x.clone();
        }
    }
    let client = reqwest::Client::builder()
        .timeout(std::time::Duration::from_secs(240))
        .build()
        .unwrap_or_default();
    let resp = match client
        .post(api_url(api_base(&p, body), "/images/generations"))
        .header(reqwest::header::AUTHORIZATION, format!("Bearer {key}"))
        .json(&req_body)
        .send()
        .await
    {
        Ok(r) => r,
        Err(e) => {
            return tool_err(
                "E_UPSTREAM",
                &e.to_string(),
                "上游不可达,请检查网络或供应商地址",
            )
        }
    };
    let status = resp.status();
    let text = resp.text().await.unwrap_or_default();
    if !status.is_success() {
        return tool_err(
            "E_UPSTREAM",
            &format!("HTTP {status}"),
            &human_for_image_status(status, &text),
        );
    }
    let v: Value = match serde_json::from_str(&text) {
        Ok(v) => v,
        Err(e) => {
            return tool_err(
                "E_UPSTREAM",
                &e.to_string(),
                &format!("上游响应非 JSON: {e}"),
            )
        }
    };
    match store_image_out(&client, s, &v, "image-generate").await {
        Ok(urls) => {
            // 首次成功即实证标 image_out(与网关 /v1/images/generations 入口同标记)
            crate::capprobe::mark_dim(
                &s.codex_home,
                &p.id,
                &model,
                "image_out",
                crate::capprobe::Tri::Yes,
            );
            ok_data(json!({ "media_urls": urls, "mime": "image/png" }))
        }
        Err(r) => *r,
    }
}

/// 图编辑:{media_url, prompt, size?, model?, provider_id?}——原图取自暂存,multipart 上送。
pub async fn image_edit(s: &AppState, body: &Value) -> Response {
    let Some(media_url) = body.get("media_url").and_then(|v| v.as_str()) else {
        return tool_err("E_ARGS", "media_url 必填", "请提供原图的媒体地址");
    };
    let Some(prompt) = body.get("prompt").and_then(|v| v.as_str()).map(str::trim) else {
        return tool_err("E_ARGS", "prompt 必填", "请提供编辑指令");
    };
    if prompt.is_empty() {
        return tool_err("E_ARGS", "prompt 空", "请提供编辑指令");
    }
    let p = match resolve_provider(s, body) {
        Ok(p) => p,
        Err(r) => return *r,
    };
    let model = body
        .get("model")
        .and_then(|v| v.as_str())
        .unwrap_or("gpt-image-1")
        .to_string();
    let key = match provider_key(&p) {
        Ok(k) => k,
        Err(r) => return *r,
    };
    let path = match media_path(&s.codex_home, media_url) {
        Ok(p) => p,
        Err(r) => return *r,
    };
    let bytes = match std::fs::read(&path) {
        Ok(b) => b,
        Err(e) => {
            return tool_err(
                "E_MEDIA_READ",
                &e.to_string(),
                "读取原图失败,请确认文件存在且可读",
            )
        }
    };
    let name = path
        .file_name()
        .and_then(|n| n.to_str())
        .unwrap_or("image.png")
        .to_string();
    let mime = mime_of(&path).to_string();
    let part = match reqwest::multipart::Part::bytes(bytes)
        .file_name(name)
        .mime_str(&mime)
    {
        Ok(part) => part,
        Err(e) => return tool_err("E_INTERNAL", &e.to_string(), "构造上送表单失败"),
    };
    let mut form = reqwest::multipart::Form::new()
        .text("model", model.clone())
        .text("prompt", prompt.to_string())
        .part("image", part);
    if let Some(size) = body.get("size").and_then(|v| v.as_str()) {
        form = form.text("size", size.to_string());
    }
    let client = reqwest::Client::builder()
        .timeout(std::time::Duration::from_secs(240))
        .build()
        .unwrap_or_default();
    let resp = match client
        .post(api_url(api_base(&p, body), "/images/edits"))
        .header(reqwest::header::AUTHORIZATION, format!("Bearer {key}"))
        .multipart(form)
        .send()
        .await
    {
        Ok(r) => r,
        Err(e) => {
            return tool_err(
                "E_UPSTREAM",
                &e.to_string(),
                "上游不可达,请检查网络或供应商地址",
            )
        }
    };
    let status = resp.status();
    let text = resp.text().await.unwrap_or_default();
    if !status.is_success() {
        return tool_err(
            "E_UPSTREAM",
            &format!("HTTP {status}"),
            &human_for_image_status(status, &text),
        );
    }
    let v: Value = match serde_json::from_str(&text) {
        Ok(v) => v,
        Err(e) => {
            return tool_err(
                "E_UPSTREAM",
                &e.to_string(),
                &format!("上游响应非 JSON: {e}"),
            )
        }
    };
    match store_image_out(&client, s, &v, "image-edit").await {
        Ok(urls) => {
            crate::capprobe::mark_dim(
                &s.codex_home,
                &p.id,
                &model,
                "image_out",
                crate::capprobe::Tri::Yes,
            );
            ok_data(json!({ "media_url": urls[0], "mime": "image/png" }))
        }
        Err(r) => *r,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use tokio::io::{AsyncReadExt, AsyncWriteExt};
    use tokio::net::TcpListener;

    /// PNG 魔数+载荷(够 store_upload 嗅探为 image/png 即可,网关不解析图内容)。
    const PNG_BYTES: &[u8] = &[
        0x89, 0x50, 0x4E, 0x47, 0x0D, 0x0A, 0x1A, 0x0A, 0x00, 0x00, 0x00, 0x0D, 0x49, 0x48, 0x44,
        0x52, 0x00, 0x00, 0x00, 0x01, 0x00, 0x00, 0x00, 0x01,
    ];

    /// 简易上游:按 path 匹配路由回固定响应;记录收到的原始请求供断言。
    async fn mock_upstream(
        routes: Vec<(String, u16, String)>,
    ) -> (String, std::sync::Arc<std::sync::Mutex<Vec<String>>>) {
        let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
        let addr = listener.local_addr().unwrap();
        let seen = std::sync::Arc::new(std::sync::Mutex::new(Vec::new()));
        let routes = std::sync::Arc::new(routes);
        let seen_c = seen.clone();
        tokio::spawn(async move {
            loop {
                let Ok((mut sock, _)) = listener.accept().await else {
                    break;
                };
                let routes = routes.clone();
                let seen_c = seen_c.clone();
                tokio::spawn(async move {
                    let mut buf = Vec::new();
                    let mut tmp = [0u8; 8192];
                    // 读满(Content-Length 或断开)
                    loop {
                        match sock.read(&mut tmp).await {
                            Ok(0) | Err(_) => break,
                            Ok(n) => {
                                buf.extend_from_slice(&tmp[..n]);
                                let s = String::from_utf8_lossy(&buf);
                                if let Some(i) = s.find("\r\n\r\n") {
                                    let cl = s[..i]
                                        .lines()
                                        .find(|l| l.to_lowercase().starts_with("content-length:"))
                                        .and_then(|l| l.split(':').nth(1))
                                        .and_then(|v| v.trim().parse::<usize>().ok())
                                        .unwrap_or(0);
                                    if buf.len() >= i + 4 + cl {
                                        break;
                                    }
                                }
                            }
                        }
                    }
                    let req = String::from_utf8_lossy(&buf).into_owned();
                    seen_c.lock().unwrap().push(req.clone());
                    let path = req.split_whitespace().nth(1).unwrap_or("").to_string();
                    let (status, body) = routes
                        .iter()
                        .find(|(p, _, _)| path.contains(p.as_str()))
                        .map(|(_, s, b)| (*s, b.clone()))
                        .unwrap_or((404, "{\"error\":{\"message\":\"page not found\"}}".into()));
                    let resp = format!(
                        "HTTP/1.1 {status} {}\r\nContent-Type: application/json\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{}",
                        if status == 200 { "OK" } else if status == 403 { "Forbidden" } else { "Not Found" },
                        body.len(),
                        body
                    );
                    let _ = sock.write_all(resp.as_bytes()).await;
                });
            }
        });
        (format!("http://{addr}"), seen)
    }

    fn mk_state(root: &Path, base_url: &str) -> AppState {
        let providers_path = root.join("providers.json");
        std::fs::write(
            &providers_path,
            json!({
                "schema_version": 3, "active_provider_id": "p1",
                "providers": [{ "id": "p1", "name": "t", "base_url": base_url, "api_key": "sk-t", "model": "m1" }]
            })
            .to_string(),
        )
        .unwrap();
        AppState {
            config_path: root.join("config.toml"),
            backup_dir: root.join("backups"),
            providers_path,
            codex_home: root.join("codex"),
            wb_home: root.to_path_buf(),
            hermes_home: root.join("hermes"),
            gem_home: root.to_path_buf(),
            grok_home: root.join("grok"),
            oc_home: root.join("ochome"),
            oclaw_home: root.join("oclaw"),
            cd_home: root.join("cdsupport"),
            cursor_home: root.join("cursorhome"),
            trae_home: root.join("traehome"),
            keypool: std::sync::Arc::new(crate::keypool::KeyPool::new()),
            tray_gate_enabled: std::sync::Arc::new(std::sync::atomic::AtomicBool::new(true)),
            launcher: Default::default(),
            health: std::sync::Arc::new(crate::acclines::HealthState::new(vec![])),
            accel: std::sync::Arc::new(std::sync::Mutex::new(crate::server::AccelCfg::default())),
            nodecreds: std::sync::Arc::new(
                std::sync::RwLock::new(crate::nodecreds::Store::empty()),
            ),
        }
    }

    fn temp_root(tag: &str) -> PathBuf {
        let r = std::env::temp_dir().join(format!("2xapi-mt-{tag}-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&r);
        std::fs::create_dir_all(&r).unwrap();
        r
    }

    async fn body_of(r: Response) -> Value {
        let b = axum::body::to_bytes(r.into_body(), usize::MAX)
            .await
            .unwrap();
        serde_json::from_slice(&b).unwrap()
    }

    #[test]
    fn b64_known_vectors() {
        assert_eq!(b64_encode(b"hi"), "aGk=");
        assert_eq!(b64_encode(b"hi!"), "aGkh");
        assert_eq!(b64_encode(b"abc"), "YWJj");
    }

    #[tokio::test]
    async fn describe_roundtrip_and_gating() {
        let root = temp_root("desc");
        let chat =
            r#"{"choices":[{"message":{"content":"红色"},"finish_reason":"stop"}]}"#.to_string();
        let (base, _seen) = mock_upstream(vec![("/chat/completions".into(), 200, chat)]).await;
        let s = mk_state(&root, &base);
        let img = root.join("pic.png");
        std::fs::write(&img, PNG_BYTES).unwrap();

        // 通:绝对路径形态 + 默认 active 供应商/默认模型
        let v = body_of(
            image_describe(
                &s,
                &json!({ "media_url": img.to_str().unwrap(), "prompt": "什么颜色?" }),
            )
            .await,
        )
        .await;
        assert_eq!(v["ok"], true, "describe 应成功: {v}");
        assert_eq!(v["data"]["text"], "红色");
        assert_eq!(v["data"]["model"], "m1");

        // 关卡:image_in=no → 人话拒
        let mut caps = crate::capprobe::Caps::unknown();
        caps.image_in = crate::capprobe::Tri::No;
        crate::capprobe::store_probe(&s.codex_home, "p1", "m1", &caps);
        let v =
            body_of(image_describe(&s, &json!({ "media_url": img.to_str().unwrap() })).await).await;
        assert_eq!(v["ok"], false);
        assert!(v["error"]["code"] == "E_CAP_IMAGE_IN", "应走识图关卡: {v}");
        assert!(v["error"]["human"].as_str().unwrap().contains("识图"));

        let _ = std::fs::remove_dir_all(&root);
    }

    #[tokio::test]
    async fn generate_403_maps_human_and_success_stores() {
        let root = temp_root("gen");
        let png_b64 = b64_encode(PNG_BYTES);
        let ok_body = format!(r#"{{"created":1,"data":[{{"b64_json":"{png_b64}"}}]}}"#);
        let (base, _seen) = mock_upstream(vec![
            (
                "/v1/images/generations".into(),
                403,
                r#"{"error":{"message":"Image generation is not enabled for this group","type":"permission_error"}}"#.into(),
            ),
            ("/images/generations".into(), 200, ok_body),
        ])
        .await;
        let s = mk_state(&root, &base);

        let v = body_of(image_generate(&s, &json!({ "prompt": "a red circle" })).await).await;
        assert_eq!(v["ok"], false, "403 应包错误: {v}");
        assert!(
            v["error"]["human"]
                .as_str()
                .unwrap()
                .contains("图像生成权限"),
            "403 人话: {v}"
        );

        // mock 单路由表里 403 优先命中——换独立实例测成功路径
        let root2 = temp_root("gen2");
        let ok_body2 = format!(r#"{{"created":1,"data":[{{"b64_json":"{png_b64}"}}]}}"#);
        let (base2, _seen2) =
            mock_upstream(vec![("/images/generations".into(), 200, ok_body2)]).await;
        let s2 = mk_state(&root2, &base2);
        let v =
            body_of(image_generate(&s2, &json!({ "prompt": "a red circle", "n": 1 })).await).await;
        assert_eq!(v["ok"], true, "生成应成功: {v}");
        let url = v["data"]["media_urls"][0].as_str().unwrap().to_string();
        assert!(url.starts_with("/media/"), "产物应为管内 URL: {url}");
        let file = crate::media::media_root(&s2.codex_home).join(url.trim_start_matches("/media/"));
        assert!(file.exists(), "产物应已落暂存: {file:?}");

        let _ = std::fs::remove_dir_all(&root);
        let _ = std::fs::remove_dir_all(&root2);
    }

    #[tokio::test]
    async fn edit_roundtrip_multipart() {
        let root = temp_root("edit");
        let png_b64 = b64_encode(PNG_BYTES);
        let ok_body = format!(r#"{{"created":1,"data":[{{"b64_json":"{png_b64}"}}]}}"#);
        let (base, seen) = mock_upstream(vec![("/images/edits".into(), 200, ok_body)]).await;
        let s = mk_state(&root, &base);
        let item = crate::media::store_upload(&s.codex_home, PNG_BYTES, "image/png", "ut").unwrap();
        let media_url = format!("/media/{}.{}", item.id, item.ext);

        let v = body_of(
            image_edit(
                &s,
                &json!({ "media_url": media_url, "prompt": "make it blue" }),
            )
            .await,
        )
        .await;
        assert_eq!(v["ok"], true, "编辑应成功: {v}");
        assert!(v["data"]["media_url"]
            .as_str()
            .unwrap()
            .starts_with("/media/"));

        let reqs = seen.lock().unwrap();
        let edits_req = reqs
            .iter()
            .find(|r| r.contains("/images/edits"))
            .expect("应有 edits 请求");
        assert!(
            edits_req.contains("multipart/form-data"),
            "应为 multipart: {}",
            &edits_req[..60]
        );
        assert!(
            edits_req.contains(&format!("filename=\"{}.{}\"", item.id, item.ext))
                || edits_req.contains("filename="),
            "应带原文件名"
        );
        assert!(edits_req.contains("make it blue"), "应带 prompt");

        let _ = std::fs::remove_dir_all(&root);
    }

    /// 真机 e2e(#[ignore],手动 `cargo test --ignored media_tools_real`):
    /// 真实 ~/.codex 供应商+真实 2xa 上游。describe 期望真通(红图描述含色词);
    /// generate 当前期望 403 组权限人话(上游已实证未开通)。零写入零污染(全失败/只读路径)。
    #[tokio::test]
    #[ignore]
    async fn media_tools_real() {
        let home = std::path::PathBuf::from(std::env::var("HOME").unwrap()).join(".codex");
        let providers_path = home.join("providers.json");
        let s = AppState {
            config_path: home.join("config.toml"),
            backup_dir: home.join("backups"),
            providers_path: providers_path.clone(),
            codex_home: home.clone(),
            wb_home: home.parent().unwrap().to_path_buf(),
            hermes_home: home.parent().unwrap().join(".hermes"),
            gem_home: home.parent().unwrap().to_path_buf(),
            grok_home: home.parent().unwrap().join(".grok"),
            oc_home: home.parent().unwrap().to_path_buf(),
            oclaw_home: home.parent().unwrap().join(".openclaw"),
            cd_home: home.parent().unwrap().to_path_buf(),
            cursor_home: home.parent().unwrap().to_path_buf(),
            trae_home: home.parent().unwrap().to_path_buf(),
            keypool: std::sync::Arc::new(crate::keypool::KeyPool::new()),
            tray_gate_enabled: std::sync::Arc::new(std::sync::atomic::AtomicBool::new(true)),
            launcher: Default::default(),
            health: std::sync::Arc::new(crate::acclines::HealthState::new(vec![])),
            accel: std::sync::Arc::new(std::sync::Mutex::new(crate::server::AccelCfg::default())),
            nodecreds: std::sync::Arc::new(
                std::sync::RwLock::new(crate::nodecreds::Store::empty()),
            ),
        };
        let img = "/tmp/red64.png";
        assert!(
            std::path::Path::new(img).exists(),
            "真机资产 /tmp/red64.png 缺失"
        );
        // describe:真实供应商+gpt-5.6(已实证识图)
        let prov = crate::providers::get_active(&providers_path).expect("须有 active 供应商");
        let v = body_of(
            image_describe(&s, &json!({ "media_url": img, "model": "gpt-5.6", "provider_id": prov.id, "prompt": "这张图是什么颜色?只回答颜色词。" })).await,
        )
        .await;
        assert_eq!(v["ok"], true, "识图应真通: {v}");
        let t = v["data"]["text"].as_str().unwrap().to_lowercase();
        assert!(t.contains('红') || t.contains("red"), "描述应含色词: {t}");
        // generate:sk-568df 组(active 所在组对 images 恒 502)——用已实证 403 形态的组验人话
        let Some(p568) = crate::providers::list(&providers_path)
            .into_iter()
            .find(|p| p.api_key.starts_with("sk-568df"))
        else {
            panic!("须有 sk-568df 组供应商");
        };
        let v = body_of(
            image_generate(
                &s,
                &json!({ "prompt": "a red circle", "provider_id": p568.id }),
            )
            .await,
        )
        .await;
        assert_eq!(v["ok"], false, "当前应因组权限失败: {v}");
        assert!(
            v["error"]["human"]
                .as_str()
                .unwrap()
                .contains("图像生成权限"),
            "403 人话: {v}"
        );
    }
}

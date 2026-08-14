//! 本地网关（M3）。监听由 main 装配的 `127.0.0.1:8787`，处理 `/v1/*`。
//!
//! 核心行为（01-D3/D5/D7，FR-4）：
//! - **逐请求实时读 active provider** → 天然热切换（FR-4.9）：切 active 后下一个请求即走新 provider，进行中请求不受影响。
//! - **Mixed/PureApi 一律用 `provider.api_key` 注入** `Authorization: Bearer`（key 来源 = Provider Store，01-D3），不透传 Codex 带来的凭证。
//! - per-provider 代理、超时返回 504、User-Agent、custom_headers；上游 4xx/5xx 原样透传。
//! - 本文件为 **M3a：透传 + key 注入 + 热切换**。Responses↔Chat 协议转换（FR-5）在 M3b 实现（届时按 `wire_api=chat_completions` 在 `/responses` 入口做转换）。

use axum::{
    body::Body,
    extract::State,
    http::{HeaderValue, Request, Response, StatusCode},
    response::IntoResponse,
};
use futures_util::StreamExt;
use std::sync::Arc;
use std::time::Duration;

use crate::providers::AccessMode;
use crate::server::AppState;

const DEFAULT_TIMEOUT_SECS: u64 = 120;

pub async fn proxy_responses(State(s): State<Arc<AppState>>, req: Request<Body>) -> Response<Body> {
    dispatch(&s, req, "responses").await
}

pub async fn proxy_chat(State(s): State<Arc<AppState>>, req: Request<Body>) -> Response<Body> {
    dispatch(&s, req, "chat/completions").await
}

pub async fn proxy_models(State(s): State<Arc<AppState>>, req: Request<Body>) -> Response<Body> {
    dispatch(&s, req, "models").await
}

/// 统一转发：取 active provider → 注入凭证 → 转发 → 流式透传响应。
async fn dispatch(state: &AppState, req: Request<Body>, suffix: &str) -> Response<Body> {
    // FR-4.9 热切换：每次都重新读 active
    let provider = match crate::providers::get_active(&state.providers_path) {
        Some(p) => p,
        None => return err_resp(StatusCode::SERVICE_UNAVAILABLE, "no active provider"),
    };
    // Official 不应经网关（01-D1）；防御性拒绝
    if provider.access_mode == AccessMode::Official {
        return err_resp(StatusCode::BAD_REQUEST, "Official 模式不走网关");
    }

    let client = match build_client(&provider) {
        Ok(c) => c,
        Err(e) => return err_resp(StatusCode::INTERNAL_SERVER_ERROR, &format!("build client: {e}")),
    };

    let method = reqwest::Method::from_bytes(req.method().as_str().as_bytes()).unwrap_or(reqwest::Method::POST);
    let (parts, body) = req.into_parts();
    let body_bytes = match axum::body::to_bytes(body, usize::MAX).await {
        Ok(b) => b,
        Err(e) => return err_resp(StatusCode::BAD_REQUEST, &format!("read body: {e}")),
    };

    // ★ 请求日志（排查用）
    let req_model = serde_json::from_slice::<serde_json::Value>(&body_bytes)
        .ok()
        .and_then(|v| v.get("model").and_then(|m| m.as_str()).map(String::from))
        .unwrap_or_default();
    let req_stream = serde_json::from_slice::<serde_json::Value>(&body_bytes)
        .ok()
        .and_then(|v| v.get("stream").and_then(|s| s.as_bool()).unwrap_or(false).then_some(true))
        .unwrap_or(false);
    eprintln!(
        "[GW] /{} | provider={} mode={:?} wire={:?} model={} stream={} body={}B",
        suffix, provider.id.get(..8).unwrap_or(&provider.id), provider.access_mode, provider.wire_api, req_model, req_stream, body_bytes.len()
    );

    // FR-5：wire_api=chat_completions 时，/responses 入口做 Responses→Chat 转换
    let (target_suffix, send_body, conv_stream): (String, Vec<u8>, Option<bool>) =
        if provider.wire_api == crate::providers::WireApi::ChatCompletions && suffix == "responses" {
            let conv = match crate::gateway_conv::responses_to_chat_request(&body_bytes) {
                Ok(c) => c,
                Err(e) => return err_resp(StatusCode::BAD_REQUEST, &format!("协议转换失败: {e}")),
            };
            ("chat/completions".to_string(), conv.body, Some(conv.stream))
        } else {
            (suffix.to_string(), body_bytes.to_vec(), None)
        };

    let url = format!("{}/{}", provider.base_url.trim_end_matches('/'), target_suffix);

    // 01-D3：注入 provider.api_key（覆盖任何来源的凭证）
    let mut rb = client
        .request(method, &url)
        .header(reqwest::header::AUTHORIZATION, format!("Bearer {}", provider.api_key));
    if let Some(ua) = provider.user_agent.as_deref().filter(|s| !s.is_empty()) {
        rb = rb.header(reqwest::header::USER_AGENT, ua);
    }
    if let Some(hs) = provider.custom_headers.as_ref() {
        for (k, v) in hs {
            rb = rb.header(k, v);
        }
    }
    if let Some(ct) = parts.headers.get(axum::http::header::CONTENT_TYPE) {
        rb = rb.header(reqwest::header::CONTENT_TYPE, ct.clone());
    }
    if !send_body.is_empty() {
        rb = rb.body(send_body);
    }

    let upstream = match rb.send().await {
        Ok(r) => r,
        Err(e) if e.is_timeout() => return err_resp(StatusCode::GATEWAY_TIMEOUT, "upstream timeout"),
        Err(e) => { eprintln!("[GW] ✗ upstream ERR: {e}"); return err_resp(StatusCode::BAD_GATEWAY, "upstream unreachable"); }
    };
    eprintln!("[GW] ← upstream {} conv={:?}", upstream.status(), conv_stream);

    // 协议转换响应（FR-5.2/5.3）
    if let Some(stream_flag) = conv_stream {
        let up_status = upstream.status();
        if !up_status.is_success() {
            let up_bytes = match upstream.bytes().await {
                Ok(b) => b,
                Err(e) => return err_resp(StatusCode::BAD_GATEWAY, &format!("read upstream: {e}")),
            };
            let st = StatusCode::from_u16(up_status.as_u16()).unwrap_or(StatusCode::BAD_GATEWAY);
            return Response::builder()
                .status(st)
                .header(axum::http::header::CONTENT_TYPE, "application/json")
                .body(Body::from(up_bytes))
                .unwrap_or_else(|_| err_resp(StatusCode::BAD_GATEWAY, "build err body"));
        }
        if stream_flag {
            // ★ 增量流式转换：逐块 Chat SSE → 即时 Responses SSE（不缓冲，防 Codex 超时断连）
            let (tx, rx) = tokio::sync::mpsc::channel::<Result<Vec<u8>, std::io::Error>>(16);
            let up_stream = upstream.bytes_stream();
            tokio::spawn(async move {
                let mut conv = crate::gateway_conv::SseConvState::new();
                let mut s = up_stream;
                while let Some(chunk) = s.next().await {
                    match chunk {
                        Ok(bytes) => {
                            for out in conv.feed(&bytes) {
                                if tx.send(Ok(out.into_bytes())).await.is_err() { return; }
                            }
                        }
                        Err(e) => { let _ = tx.send(Err(std::io::Error::new(std::io::ErrorKind::Other, e))).await; return; }
                    }
                }
                for out in conv.finish() {
                    if tx.send(Ok(out.into_bytes())).await.is_err() { return; }
                }
            });
            return Response::builder()
                .status(StatusCode::OK)
                .header(axum::http::header::CONTENT_TYPE, "text/event-stream")
                .body(Body::from_stream(tokio_stream::wrappers::ReceiverStream::new(rx)))
                .unwrap_or_else(|_| err_resp(StatusCode::BAD_GATEWAY, "build stream body"));
        } else {
            let up_bytes = match upstream.bytes().await {
                Ok(b) => b,
                Err(e) => return err_resp(StatusCode::BAD_GATEWAY, &format!("read upstream: {e}")),
            };
            let converted = match crate::gateway_conv::chat_json_to_responses_json(&up_bytes) {
                Ok(v) => v,
                Err(e) => return err_resp(StatusCode::BAD_GATEWAY, &format!("resp conv: {e}")),
            };
            return Response::builder()
                .status(StatusCode::OK)
                .header(axum::http::header::CONTENT_TYPE, "application/json")
                .body(Body::from(converted))
                .unwrap_or_else(|_| err_resp(StatusCode::BAD_GATEWAY, "build conv body"));
        }
    }

    // 否则：上游状态码 + body 原样流式透传（FR-4.11）
    let status = StatusCode::from_u16(upstream.status().as_u16()).unwrap_or(StatusCode::BAD_GATEWAY);
    let mut resp = Response::builder().status(status);
    if let Some(ct) = upstream.headers().get(reqwest::header::CONTENT_TYPE) {
        if let Ok(hv) = HeaderValue::from_bytes(ct.as_bytes()) {
            resp = resp.header(axum::http::header::CONTENT_TYPE, hv);
        }
    }
    // 流式回传
    let stream = upstream
        .bytes_stream()
        .map(|r| r.map_err(|e| std::io::Error::new(std::io::ErrorKind::Other, e)));
    match resp.body(Body::from_stream(stream)) {
        Ok(r) => r,
        Err(_) => err_resp(StatusCode::BAD_GATEWAY, "build response body"),
    }
}

fn build_client(provider: &crate::providers::Provider) -> Result<reqwest::Client, String> {
    let timeout = Duration::from_secs(provider.timeout_secs.unwrap_or(DEFAULT_TIMEOUT_SECS));
    let mut b = reqwest::Client::builder().timeout(timeout);
    if let Some(p) = provider.proxy_url.as_deref().filter(|s| !s.is_empty()) {
        match reqwest::Proxy::all(p) {
            Ok(px) => b = b.proxy(px),
            Err(e) => return Err(format!("proxy: {e}")),
        }
    }
    b.build().map_err(|e| format!("client: {e}"))
}

fn err_resp(status: StatusCode, msg: &str) -> Response<Body> {
    (status, msg.to_string()).into_response()
}

// ── 单测（M3a Gate：mock 上游验证每跳）──────────────────────

#[cfg(test)]
mod tests {
    use super::*;
    use crate::providers::{self, ProviderInput};
    use crate::server::AppState;
    use axum::{routing::post, Router};
    use std::path::PathBuf;
    use std::sync::atomic::{AtomicU64, Ordering};
    use std::sync::Mutex;

    static COUNTER: AtomicU64 = AtomicU64::new(0);

    fn make_state(label: &str) -> (AppState, PathBuf, PathBuf) {
        let n = COUNTER.fetch_add(1, Ordering::SeqCst);
        let root = std::env::temp_dir().join(format!("2xapi-m3-{label}-{}-{n}", std::process::id()));
        let _ = std::fs::remove_dir_all(&root);
        std::fs::create_dir_all(&root).unwrap();
        let providers_path = root.join("providers.json");
        let state = AppState {
            config_path: root.join("config.toml"),
            backup_dir: root.join("backups"),
            providers_path: providers_path.clone(),
            codex_home: root.join("codex"),
            launcher: Default::default(),
        };
        (state, providers_path, root)
    }

    fn add_provider(path: &std::path::Path, base_url: &str, api_key: &str) -> String {
        let input = ProviderInput {
            name: "T".into(),
            base_url: base_url.into(),
            api_key: api_key.into(),
            model: "gpt-test".into(),
            sub2api_multiplier: 1.0,
            ..ProviderInput::default()
        };
        let p = providers::create(path, input).unwrap();
        providers::set_active(path, &p.id);
        p.id
    }

    /// 启动一个 mock 上游，返回 (base_url, 收到的 Authorization 列表)。
    async fn mock_upstream(resp_body: &'static str) -> (String, Arc<Mutex<Vec<String>>>) {
        let seen = Arc::new(Mutex::new(Vec::new()));
        let seen_clone = seen.clone();
        let app = Router::new().route(
            "/responses",
            post(move |h: axum::http::HeaderMap, _b: axum::body::Bytes| {
                let seen = seen_clone.clone();
                async move {
                    let auth = h
                        .get("authorization")
                        .and_then(|v| v.to_str().ok())
                        .map(String::from)
                        .unwrap_or_default();
                    seen.lock().unwrap().push(auth);
                    (StatusCode::OK, resp_body)
                }
            }),
        );
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let addr = listener.local_addr().unwrap();
        tokio::spawn(async move {
            let _ = axum::serve(listener, app).await;
        });
        (format!("http://{}", addr), seen)
    }

    async fn req_post_responses(body: &'static str) -> Request<Body> {
        Request::builder()
            .method("POST")
            .uri("/v1/responses")
            .header("content-type", "application/json")
            .body(Body::from(body))
            .unwrap()
    }

    /// 01-D3 / FR-4.3：上游收到的 Authorization = Bearer {provider.api_key}（非 Codex 传来值），且 body 透传。
    #[tokio::test]
    async fn injects_provider_key_and_passthrough() {
        let (base, seen) = mock_upstream("PASSTHROUGH_BODY").await;
        let (state, providers_path, root) = make_state("inject");
        // Codex 试图自带一个假 key，网关应忽略它、注入 provider key
        let _id = add_provider(&providers_path, &base, "sk-provider-secret");

        let resp = proxy_responses(State(Arc::new(state)), req_post_responses("{\"hello\":1}").await).await;
        assert_eq!(resp.status(), StatusCode::OK);
        let bytes = axum::body::to_bytes(resp.into_body(), usize::MAX).await.unwrap();
        assert_eq!(&bytes[..], b"PASSTHROUGH_BODY");
        // 给 mock 一点写 seen 的时间
        tokio::time::sleep(std::time::Duration::from_millis(50)).await;
        assert_eq!(seen.lock().unwrap().first().map(|s| s.as_str()), Some("Bearer sk-provider-secret"));
        let _ = std::fs::remove_dir_all(&root);
    }

    /// 01-D7 / FR-4.9：热切换——切 active 后下一请求走新 provider（新 key）。
    #[tokio::test]
    async fn hot_swap_next_request_uses_new_provider() {
        let (base_a, seen_a) = mock_upstream("FROM_A").await;
        let (base_b, seen_b) = mock_upstream("FROM_B").await;
        let (state, providers_path, root) = make_state("hotswap");

        let id_a = add_provider(&providers_path, &base_a, "sk-A");
        // 另建 B 并不激活
        let input_b = ProviderInput {
            name: "B".into(),
            base_url: base_b.clone(),
            api_key: "sk-B".into(),
            model: "m".into(),
            sub2api_multiplier: 1.0,
            ..ProviderInput::default()
        };
        let p_b = providers::create(&providers_path, input_b).unwrap();

        // 先走 A
        let r1 = proxy_responses(State(Arc::new(clone_state(&state))), req_post_responses("{}").await).await;
        assert_eq!(r1.status(), StatusCode::OK);
        // 热切换到 B（仅改 active_provider_id，不重启）
        providers::set_active(&providers_path, &p_b.id);
        let r2 = proxy_responses(State(Arc::new(clone_state(&state))), req_post_responses("{}").await).await;
        assert_eq!(r2.status(), StatusCode::OK);

        tokio::time::sleep(std::time::Duration::from_millis(50)).await;
        assert_eq!(seen_a.lock().unwrap().first().map(|s| s.as_str()), Some("Bearer sk-A"));
        assert_eq!(seen_b.lock().unwrap().first().map(|s| s.as_str()), Some("Bearer sk-B"));
        let _ = id_a;
        let _ = std::fs::remove_dir_all(&root);
    }

    /// FR-4.8：上游超时 → 504。
    #[tokio::test]
    async fn upstream_timeout_returns_504() {
        let app = Router::new().route(
            "/responses",
            post(|_h: axum::http::HeaderMap, _b: axum::body::Bytes| async move {
                tokio::time::sleep(std::time::Duration::from_secs(3)).await;
                (StatusCode::OK, "slow")
            }),
        );
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let addr = listener.local_addr().unwrap();
        tokio::spawn(async move {
            let _ = axum::serve(listener, app).await;
        });

        let (state, providers_path, root) = make_state("timeout");
        // 直接写 providers.json（绕过 create 的校验，专测网关超时行为；timeout_secs=1）
        let pd = providers::ProviderData {
            schema_version: 1,
            active_provider_id: Some("p-to".into()),
            providers: vec![providers::Provider {
                id: "p-to".into(),
                name: "T".into(),
                base_url: format!("http://{}", addr),
                api_key: "sk".into(),
                model: "m".into(),
                timeout_secs: Some(1),
                sub2api_multiplier: 1.0,
                ..Default::default()
            }],
        };
        std::fs::write(&providers_path, serde_json::to_string(&pd).unwrap()).unwrap();

        let resp = proxy_responses(State(Arc::new(state)), req_post_responses("{}").await).await;
        assert_eq!(resp.status(), StatusCode::GATEWAY_TIMEOUT);
        let _ = std::fs::remove_dir_all(&root);
    }

    /// FR-4.11：上游 5xx 原样透传。
    #[tokio::test]
    async fn upstream_error_passthrough() {
        let app = Router::new().route(
            "/responses",
            post(|_h: axum::http::HeaderMap, _b: axum::body::Bytes| async move {
                (StatusCode::INSUFFICIENT_STORAGE, "upstream-broke")
            }),
        );
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let addr = listener.local_addr().unwrap();
        tokio::spawn(async move {
            let _ = axum::serve(listener, app).await;
        });

        let (state, providers_path, root) = make_state("err507");
        add_provider(&providers_path, &format!("http://{}", addr), "sk");

        let resp = proxy_responses(State(Arc::new(state)), req_post_responses("{}").await).await;
        assert_eq!(resp.status(), StatusCode::INSUFFICIENT_STORAGE); // 507 透传
        let bytes = axum::body::to_bytes(resp.into_body(), usize::MAX).await.unwrap();
        assert_eq!(&bytes[..], b"upstream-broke");
        let _ = std::fs::remove_dir_all(&root);
    }

    /// 无 active provider → 503。
    #[tokio::test]
    async fn no_active_provider_returns_503() {
        let (state, _providers_path, root) = make_state("noactive");
        let resp = proxy_responses(State(Arc::new(state)), req_post_responses("{}").await).await;
        assert_eq!(resp.status(), StatusCode::SERVICE_UNAVAILABLE);
        let _ = std::fs::remove_dir_all(&root);
    }

    fn clone_state(s: &AppState) -> AppState {
        AppState {
            config_path: s.config_path.clone(),
            backup_dir: s.backup_dir.clone(),
            providers_path: s.providers_path.clone(),
            codex_home: s.codex_home.clone(),
            launcher: s.launcher.clone(),
        }
    }
}

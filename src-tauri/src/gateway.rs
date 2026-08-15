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

use crate::acclines::{AccLine, Cred};
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

    // ── 阶段 4 加速装配:决定走哪条线路,并备好直连兜底 ──
    let line = accel_plan(state, &provider.base_url);
    let direct_client = match build_client(&provider) {
        Ok(c) => c,
        Err(e) => return err_resp(StatusCode::INTERNAL_SERVER_ERROR, &format!("build client: {e}")),
    };
    let line_client = match &line {
        Some(l) => match build_line_client(l, Duration::from_secs(provider.timeout_secs.unwrap_or(DEFAULT_TIMEOUT_SECS))) {
            Ok(c) => Some(c),
            Err(e) => return err_resp(StatusCode::INTERNAL_SERVER_ERROR, &format!("build line client: {e}")),
        },
        None => None,
    };
    if let Some(l) = &line {
        eprintln!("[GW] accel line={} endpoint={} (直连兜底开启)", l.id, l.endpoint);
    }

    let method = reqwest::Method::from_bytes(req.method().as_str().as_bytes()).unwrap_or(reqwest::Method::POST);
    let (parts, body) = req.into_parts();
    let body_bytes = match axum::body::to_bytes(body, usize::MAX).await {
        Ok(b) => b,
        Err(e) => return err_resp(StatusCode::BAD_REQUEST, &format!("read body: {e}")),
    };

    // ★ 请求日志(排查用)
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

    // 01-D3：注入 provider.api_key（覆盖任何来源的凭证）；请求构建抽为闭包以支持换线重试
    let build_rb = |client: &reqwest::Client| -> reqwest::RequestBuilder {
        let mut rb = client
            .request(method.clone(), url.clone())
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
            rb = rb.body(send_body.clone());
        }
        rb
    };

    // ── 换线重试:首发带 line 的 client;连接层失败且 line 存在 → 用直连 client 重试一次。
    // send() 返回 Ok 前未向客户端写任何字节,故「已开始写响应(中途断流)」绝不重试是天然成立的。
    let used_line = line_client.is_some();
    let first = line_client.as_ref().unwrap_or(&direct_client);
    let upstream = match build_rb(first).send().await {
        Ok(r) => {
            // 代理 407 → 线路凭证无效,人话化;不换直连(避免绕过用户指定的线路)
            if used_line && r.status() == reqwest::StatusCode::PROXY_AUTHENTICATION_REQUIRED {
                eprintln!("[GW] line 凭证无效(407)");
                return err_resp(StatusCode::BAD_GATEWAY, "节点凭证无效");
            }
            r
        }
        Err(e) => {
            if used_line && proxy_auth_error(&e) {
                eprintln!("[GW] line 代理认证失败: {e}");
                return err_resp(StatusCode::BAD_GATEWAY, "节点凭证无效");
            }
            if used_line {
                eprintln!("[GW] line 失败({e}),换直连重试一次");
                match build_rb(&direct_client).send().await {
                    Ok(r) => r,
                    Err(e2) if e2.is_timeout() => return err_resp(StatusCode::GATEWAY_TIMEOUT, "upstream timeout"),
                    Err(e2) => {
                        eprintln!("[GW] ✗ upstream ERR: {e2}");
                        return err_resp(StatusCode::BAD_GATEWAY, "upstream unreachable");
                    }
                }
            } else if e.is_timeout() {
                return err_resp(StatusCode::GATEWAY_TIMEOUT, "upstream timeout");
            } else {
                eprintln!("[GW] ✗ upstream ERR: {e}");
                return err_resp(StatusCode::BAD_GATEWAY, "upstream unreachable");
            }
        }
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

// ── 阶段 4 加速装配(任务书 §五)────────────────────────────

/// 判断当前请求应走哪条加速线路:
/// - mode=custom → 自定义节点(全量走代理,凭证从 accel-credentials.json 注入);
/// - mode=official → 按供应商 base_url 命中的官方线路;
/// - mode=off / 未命中 → 直连(None)。
fn accel_plan(state: &AppState, base_url: &str) -> Option<AccLine> {
    let cfg = state.accel.lock().unwrap();
    match cfg.mode.as_str() {
        "custom" => {
            let endpoint = cfg.custom_node.trim();
            if endpoint.is_empty() {
                None
            } else {
                Some(AccLine {
                    id: "custom".into(),
                    name: "自定义节点".into(),
                    endpoint: endpoint.to_string(),
                    scope: Vec::new(),
                    priority: 0,
                    enabled: true,
                    credential: crate::acclines::load_credentials(&state.codex_home),
                })
            }
        }
        "official" => {
            let lines = state.health.lines.lock().unwrap();
            crate::acclines::match_line(base_url, &lines).cloned()
        }
        _ => None,
    }
}

/// 走线路的 HTTP 客户端:Proxy::all(line.endpoint) + basic auth(凭证来自线路)。
fn build_line_client(line: &AccLine, timeout: Duration) -> Result<reqwest::Client, String> {
    let proxy = reqwest::Proxy::all(&line.endpoint).map_err(|e| format!("proxy: {e}"))?;
    let proxy = if let Some(cred) = &line.credential {
        proxy.basic_auth(&cred.user, &cred.pass)
    } else {
        proxy
    };
    reqwest::Client::builder()
        .timeout(timeout)
        .proxy(proxy)
        .build()
        .map_err(|e| format!("client: {e}"))
}

/// 请求错误是否指向代理认证失败(407/401):CONNECT 模式下代理拒绝会以 Err 形式出现
/// (hyper 的 ProxyAuthRequired),需据此区分「凭证错误(不重试直连)」与「线路不可达(可换直连)」。
fn proxy_auth_error(e: &reqwest::Error) -> bool {
    if let Some(st) = e.status() {
        return st == reqwest::StatusCode::UNAUTHORIZED || st == reqwest::StatusCode::PROXY_AUTHENTICATION_REQUIRED;
    }
    // 拼接错误及整条 source 链的 Display(hyper 的 ProxyAuthRequired 在链内,顶层 to_string 不含)
    let mut chain = String::new();
    let mut cur: Option<&dyn std::error::Error> = Some(e);
    while let Some(err) = cur {
        chain.push(' ');
        chain.push_str(&err.to_string());
        cur = err.source();
    }
    let msg = chain.to_ascii_lowercase();
    let has_code = |c: &str| msg.split(|ch: char| !ch.is_ascii_alphanumeric()).any(|t| t == c);
    has_code("407") || has_code("401") || msg.contains("proxy auth") || msg.contains("proxyauthenticationrequired")
}

/// test-node 探测结果(供 POST /api/accel/test-node 映射人话)。
#[derive(Debug)]
pub enum NodeTestOutcome {
    Ok { latency_ms: u64 },
    Timeout,
    Auth,
    Unavailable,
}

/// 经代理测试目标节点连通性:basic auth 来自凭证(可空);成功计时返回。
/// target 由装配方决定(契约固定为 https://api.2xa.cc.cd/models)。
pub async fn test_node_via(endpoint: &str, target: &str, cred: Option<&Cred>, timeout: Duration) -> NodeTestOutcome {
    let proxy = match reqwest::Proxy::all(endpoint) {
        Ok(p) => p,
        Err(_) => return NodeTestOutcome::Unavailable,
    };
    let proxy = if let Some(c) = cred {
        proxy.basic_auth(&c.user, &c.pass)
    } else {
        proxy
    };
    let client = match reqwest::Client::builder().timeout(timeout).proxy(proxy).build() {
        Ok(c) => c,
        Err(_) => return NodeTestOutcome::Unavailable,
    };
    let started = std::time::Instant::now();
    match client.get(target).send().await {
        Ok(r) => {
            let latency_ms = started.elapsed().as_millis() as u64;
            let st = r.status();
            if st == reqwest::StatusCode::UNAUTHORIZED || st == reqwest::StatusCode::PROXY_AUTHENTICATION_REQUIRED {
                NodeTestOutcome::Auth
            } else if st.is_success() {
                NodeTestOutcome::Ok { latency_ms }
            } else {
                NodeTestOutcome::Unavailable
            }
        }
        Err(e) => {
            if e.is_timeout() {
                NodeTestOutcome::Timeout
            } else if proxy_auth_error(&e) {
                NodeTestOutcome::Auth
            } else {
                NodeTestOutcome::Unavailable
            }
        }
    }
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
    use axum::{routing::{get, post}, Router};
    use std::path::PathBuf;
    use std::sync::atomic::{AtomicU64, Ordering};
    use std::sync::Mutex;
    use tokio::io::{AsyncReadExt, AsyncWriteExt};

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
            health: std::sync::Arc::new(crate::acclines::HealthState::new(vec![])),
            accel: std::sync::Arc::new(std::sync::Mutex::new(crate::server::AccelCfg::default())),
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
    /// 另加 GET /(供 test-node 探测 200 用),不记录 seen。
    async fn mock_upstream(resp_body: &'static str) -> (String, Arc<Mutex<Vec<String>>>) {
        let seen = Arc::new(Mutex::new(Vec::new()));
        let seen_clone = seen.clone();
        let app = Router::new()
            .route(
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
            )
            .route("/", get(|| async { (StatusCode::OK, "UP_OK") }));
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
            health: s.health.clone(),
            accel: s.accel.clone(),
        }
    }

    // ── 阶段 4 加速装配:mock 代理集成测试(任务书 §五 必测)──

    fn test_line(id: &str, endpoint: &str, scope: &[&str], cred: Option<Cred>) -> AccLine {
        AccLine {
            id: id.into(),
            name: id.into(),
            endpoint: endpoint.into(),
            scope: scope.iter().map(|s| s.to_string()).collect(),
            priority: 1,
            enabled: true,
            credential: cred,
        }
    }

    fn set_accel(state: &AppState, mode: &str, lines: Vec<AccLine>, custom_node: &str) {
        *state.accel.lock().unwrap() = crate::server::AccelCfg {
            mode: mode.into(),
            custom_node: custom_node.into(),
        };
        state.health.set_lines(lines);
    }

    fn b64(data: &str) -> String {
        const T: &[u8] = b"ABCDEFGHIJKLMNOPQRSTUVWXYZabcdefghijklmnopqrstuvwxyz0123456789+/";
        let bytes = data.as_bytes();
        let mut out = String::new();
        for chunk in bytes.chunks(3) {
            let n = chunk.len();
            let mut v = [0u8; 3];
            v[..n].copy_from_slice(chunk);
            let x = ((v[0] as u32) << 16) | ((v[1] as u32) << 8) | (v[2] as u32);
            out.push(T[((x >> 18) & 63) as usize] as char);
            out.push(T[((x >> 12) & 63) as usize] as char);
            out.push(if n > 1 { T[((x >> 6) & 63) as usize] as char } else { '=' });
            out.push(if n > 2 { T[(x & 63) as usize] as char } else { '=' });
        }
        out
    }

    fn auth_ok(head: &str, required: &Option<(String, String)>) -> bool {
        let Some((u, p)) = required else { return true };
        let expected = format!("Basic {}", b64(&format!("{}:{}", u, p)));
        head.lines().any(|l| {
            let low = l.to_ascii_lowercase();
            if !(low.starts_with("proxy-authorization:") || low.starts_with("authorization:")) {
                return false;
            }
            l.splitn(2, ':').nth(1).map(str::trim) == Some(expected.as_str())
        })
    }

    fn content_length(head: &str) -> Option<usize> {
        head.lines().find_map(|l| {
            if l.to_ascii_lowercase().starts_with("content-length:") {
                l.splitn(2, ':').nth(1).and_then(|s| s.trim().parse().ok())
            } else {
                None
            }
        })
    }

    fn split_host_port(s: &str, def: u16) -> (String, u16) {
        if let Some(i) = s.rfind(':') {
            if let Ok(port) = s[i + 1..].parse::<u16>() {
                return (s[..i].to_string(), port);
            }
        }
        (s.to_string(), def)
    }

    fn find_head_end(buf: &[u8]) -> Option<usize> {
        buf.windows(4).position(|w| w == b"\r\n\r\n").map(|p| p + 4)
    }

    /// 读取 HTTP 头(到 \r\n\r\n 止),返回 (head 字符串, 该次读取中头之后的剩余字节)。
    /// 剩余字节必须回传——否则头读取会吞掉紧跟其后的 body 字节。
    async fn read_http_head<R: tokio::io::AsyncBufRead + Unpin>(r: &mut R) -> std::io::Result<(String, Vec<u8>)> {
        let mut buf = Vec::new();
        let mut tmp = [0u8; 2048];
        let end;
        loop {
            let n = r.read(&mut tmp).await?;
            if n == 0 {
                return Err(std::io::Error::new(std::io::ErrorKind::UnexpectedEof, "eof in head"));
            }
            buf.extend_from_slice(&tmp[..n]);
            if let Some(pos) = find_head_end(&buf) {
                end = pos;
                break;
            }
        }
        let head = String::from_utf8_lossy(&buf[..end]).to_string();
        let rest = buf[end..].to_vec();
        Ok((head, rest))
    }

    /// 按 Content-Length 补齐请求/响应体;rest 为头读取时已缓冲的剩余字节。
    async fn read_body<R: tokio::io::AsyncBufRead + Unpin>(r: &mut R, head: &str, mut rest: Vec<u8>) -> Vec<u8> {
        let clen = content_length(head).unwrap_or(0);
        if clen > rest.len() {
            let mut extra = vec![0u8; clen - rest.len()];
            let _ = r.read_exact(&mut extra).await;
            rest.extend_from_slice(&extra);
        }
        rest.truncate(clen);
        rest
    }

    /// mock 代理:支持 CONNECT 隧道 + HTTP 绝对式转发;需 basic-auth(auth=Some 时校验
    /// Proxy-Authorization/Authorization 头,不符返回 407)。seen 记录收到的方法与目标。
    async fn handle_proxy_conn(
        sock: tokio::net::TcpStream,
        auth: Option<(String, String)>,
        seen: Arc<Mutex<Vec<String>>>,
    ) {
        let mut br = tokio::io::BufReader::new(sock);
        let (head, rest) = match read_http_head(&mut br).await {
            Ok(h) => h,
            Err(_) => return,
        };
        let lines: Vec<&str> = head.split("\r\n").collect();
        let first_line = lines.first().map(|s| s.to_string()).unwrap_or_default();
        let mut it = first_line.split_whitespace();
        let method = it.next().unwrap_or("").to_string();
        let target = it.next().unwrap_or("").to_string();
        if method.is_empty() {
            return;
        }

        if method == "CONNECT" {
            seen.lock().unwrap().push(format!("CONNECT {target}"));
            if !auth_ok(&head, &auth) {
                let _ = br.write_all(b"HTTP/1.1 407 Proxy Authentication Required\r\nContent-Length: 0\r\nConnection: close\r\n\r\n").await;
                return;
            }
            let _ = br.write_all(b"HTTP/1.1 200 Connection Established\r\n\r\n").await;
            let (host, port) = split_host_port(&target, 443);
            if let Ok(up) = tokio::net::TcpStream::connect((host.as_str(), port)).await {
                let mut sock = br.into_inner();
                let (mut cr, mut cw) = sock.into_split();
                let (mut ur, mut uw) = up.into_split();
                let _ = tokio::join!(tokio::io::copy(&mut cr, &mut uw), tokio::io::copy(&mut ur, &mut cw));
            }
            return;
        }

        // HTTP 绝对式转发
        seen.lock().unwrap().push(format!("{method} {target}"));
        if !auth_ok(&head, &auth) {
            let body = b"proxy auth required";
            let resp = format!(
                "HTTP/1.1 407 Proxy Authentication Required\r\nContent-Type: text/plain\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{}",
                body.len(),
                String::from_utf8_lossy(body)
            );
            let _ = br.write_all(resp.as_bytes()).await;
            return;
        }
        let body = read_body(&mut br, &head, rest).await;

        let after = target.find("://").map(|i| &target[i + 3..]).unwrap_or(&target);
        let host_port = after.split('/').next().unwrap_or("").to_string();
        let (host, port) = split_host_port(&host_port, 80);
        let mut up = match tokio::net::TcpStream::connect((host.as_str(), port)).await {
            Ok(u) => u,
            Err(_) => {
                let _ = br.write_all(b"HTTP/1.1 502 Bad Gateway\r\nContent-Length: 0\r\nConnection: close\r\n\r\n").await;
                return;
            }
        };
        // 重建请求行为 origin-form,去掉 Proxy-Authorization(代理自身的鉴权头不下发上游)
        let path = if after.len() >= host_port.len() { &after[host_port.len()..] } else { "" };
        let path = if path.is_empty() { "/" } else { path };
        let mut out = String::new();
        out.push_str(&format!("{method} {path} HTTP/1.1\r\n"));
        for l in lines.iter().skip(1) {
            if l.is_empty() {
                break;
            }
            if l.to_ascii_lowercase().starts_with("proxy-authorization") {
                continue;
            }
            out.push_str(l);
            out.push_str("\r\n");
        }
        out.push_str("\r\n");
        let _ = up.write_all(out.as_bytes()).await;
        if !body.is_empty() {
            let _ = up.write_all(&body).await;
        }
        // 回传上游响应
        let mut up_br = tokio::io::BufReader::new(up);
        let (resp_head, rest) = match read_http_head(&mut up_br).await {
            Ok(h) => h,
            Err(_) => return,
        };
        let rbody = read_body(&mut up_br, &resp_head, rest).await;
        let _ = br.write_all(resp_head.as_bytes()).await;
        let _ = br.write_all(&rbody).await;
    }

    async fn mock_proxy(auth: Option<(&str, &str)>) -> (String, Arc<Mutex<Vec<String>>>) {
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let addr = listener.local_addr().unwrap();
        let seen = Arc::new(Mutex::new(Vec::new()));
        let required = auth.map(|(u, p)| (u.to_string(), p.to_string()));
        let seen2 = seen.clone();
        tokio::spawn(async move {
            loop {
                let Ok((sock, _)) = listener.accept().await else { break };
                let seen2 = seen2.clone();
                let req = required.clone();
                tokio::spawn(async move {
                    handle_proxy_conn(sock, req, seen2).await;
                });
            }
        });
        (format!("http://{}", addr), seen)
    }

    /// 坏代理:接受连接后立即关闭(连接层失败 → 应触发换直连重试;确定性,无端口竞争)。
    async fn broken_proxy() -> String {
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let addr = listener.local_addr().unwrap();
        tokio::spawn(async move {
            loop {
                let Ok((sock, _)) = listener.accept().await else { break };
                drop(sock);
            }
        });
        format!("http://{}", addr)
    }

    /// 挂起代理:读掉请求后保持连接但不回应(触发超时)。
    async fn hang_proxy() -> String {
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let addr = listener.local_addr().unwrap();
        tokio::spawn(async move {
            loop {
                let Ok((mut sock, _)) = listener.accept().await else { break };
                tokio::spawn(async move {
                    let mut buf = [0u8; 4096];
                    let _ = sock.read(&mut buf).await; // 消费请求,确保写完成
                    tokio::time::sleep(Duration::from_secs(60)).await;
                    let _ = sock;
                });
            }
        });
        format!("http://{}", addr)
    }

    // ① 命中走代理:上游应经代理转发收到请求(直连场景由 ② 对照)。
    #[tokio::test]
    async fn accel_hit_routes_through_proxy() {
        let (up_base, up_seen) = mock_upstream("PROXIED_BODY").await;
        let (px_url, px_seen) = mock_proxy(Some(("u", "p"))).await;
        let (state, providers_path, root) = make_state("accel-hit");
        add_provider(&providers_path, &up_base, "sk-line");
        set_accel(
            &state,
            "official",
            vec![test_line("l1", &px_url, &["127.0.0.1"], Some(Cred { user: "u".into(), pass: "p".into() }))],
            "",
        );

        let resp = proxy_responses(State(Arc::new(state)), req_post_responses("{\"hello\":1}").await).await;
        assert_eq!(resp.status(), StatusCode::OK);
        let bytes = axum::body::to_bytes(resp.into_body(), usize::MAX).await.unwrap();
        assert_eq!(&bytes[..], b"PROXIED_BODY");
        tokio::time::sleep(std::time::Duration::from_millis(150)).await;
        assert!(!px_seen.lock().unwrap().is_empty(), "代理应看到经其转发的请求");
        assert_eq!(up_seen.lock().unwrap().first().map(|s| s.as_str()), Some("Bearer sk-line"));
        let _ = std::fs::remove_dir_all(&root);
    }

    // ② 不命中 → 回落直连:代理不应看到任何请求。
    #[tokio::test]
    async fn accel_no_match_falls_back_direct() {
        let (up_base, up_seen) = mock_upstream("DIRECT_BODY").await;
        let (px_url, px_seen) = mock_proxy(Some(("u", "p"))).await;
        let (state, providers_path, root) = make_state("accel-nomatch");
        add_provider(&providers_path, &up_base, "sk-direct");
        set_accel(
            &state,
            "official",
            vec![test_line("l1", &px_url, &["not-this-host.com"], Some(Cred { user: "u".into(), pass: "p".into() }))],
            "",
        );

        let resp = proxy_responses(State(Arc::new(state)), req_post_responses("{}").await).await;
        assert_eq!(resp.status(), StatusCode::OK);
        let bytes = axum::body::to_bytes(resp.into_body(), usize::MAX).await.unwrap();
        assert_eq!(&bytes[..], b"DIRECT_BODY");
        tokio::time::sleep(std::time::Duration::from_millis(150)).await;
        assert!(px_seen.lock().unwrap().is_empty(), "未命中不应经代理");
        assert_eq!(up_seen.lock().unwrap().first().map(|s| s.as_str()), Some("Bearer sk-direct"));
        let _ = std::fs::remove_dir_all(&root);
    }

    // ③ 坏线(代理连不上)→ 自动换直连重试,响应完整不断流。
    #[tokio::test]
    async fn accel_bad_line_retries_direct_and_stream_complete() {
        let (up_base, up_seen) = mock_upstream("FULL_STREAM_BODY_1234567890").await;
        let bad = broken_proxy().await;
        let (state, providers_path, root) = make_state("accel-badline");
        add_provider(&providers_path, &up_base, "sk-retry");
        set_accel(&state, "official", vec![test_line("l1", &bad, &["127.0.0.1"], None)], "");

        let resp = proxy_responses(State(Arc::new(state)), req_post_responses("{}").await).await;
        assert_eq!(resp.status(), StatusCode::OK);
        let bytes = axum::body::to_bytes(resp.into_body(), usize::MAX).await.unwrap();
        assert_eq!(&bytes[..], b"FULL_STREAM_BODY_1234567890", "坏线换直连后响应应完整");
        tokio::time::sleep(std::time::Duration::from_millis(150)).await;
        assert_eq!(up_seen.lock().unwrap().len(), 1, "直连重试应恰好命中上游一次");
        let _ = std::fs::remove_dir_all(&root);
    }

    // ④ 凭证错误 → 代理 407 → 错误人话化(且不换直连绕过线路)。
    #[tokio::test]
    async fn accel_wrong_cred_proxy_407_humanized() {
        let (up_base, up_seen) = mock_upstream("SHOULD_NOT_REACH").await;
        let (px_url, px_seen) = mock_proxy(Some(("u", "right"))).await;
        let (state, providers_path, root) = make_state("accel-407");
        add_provider(&providers_path, &up_base, "sk-wrong");
        set_accel(
            &state,
            "official",
            vec![test_line("l1", &px_url, &["127.0.0.1"], Some(Cred { user: "u".into(), pass: "wrong".into() }))],
            "",
        );

        let resp = proxy_responses(State(Arc::new(state)), req_post_responses("{}").await).await;
        assert_eq!(resp.status(), StatusCode::BAD_GATEWAY);
        let bytes = axum::body::to_bytes(resp.into_body(), usize::MAX).await.unwrap();
        let s = String::from_utf8_lossy(&bytes);
        assert!(s.contains("节点凭证无效"), "407 应人话化为节点凭证无效, got {s}");
        tokio::time::sleep(std::time::Duration::from_millis(150)).await;
        assert!(up_seen.lock().unwrap().is_empty(), "凭证错误不应换直连命中上游");
        assert!(!px_seen.lock().unwrap().is_empty(), "代理应看到请求");
        let _ = std::fs::remove_dir_all(&root);
    }

    // ── test-node 探测(核心函数,路由 /api/accel/test-node 复用)──

    #[tokio::test]
    async fn test_node_via_ok_through_proxy() {
        let (up_base, _) = mock_upstream("OK").await;
        let (px_url, _) = mock_proxy(Some(("u", "p"))).await;
        let cred = Cred { user: "u".into(), pass: "p".into() };
        let out = test_node_via(&px_url, &up_base, Some(&cred), Duration::from_secs(5)).await;
        match out {
            NodeTestOutcome::Ok { .. } => {} // 本地 mock 可能 0ms,不苛求 latency 具体值
            other => panic!("应成功, got {other:?}"),
        }
    }

    #[tokio::test]
    async fn test_node_via_wrong_cred_407_auth() {
        let (px_url, _) = mock_proxy(Some(("u", "right"))).await;
        let cred = Cred { user: "u".into(), pass: "wrong".into() };
        let out = test_node_via(&px_url, "https://api.2xa.cc.cd/models", Some(&cred), Duration::from_secs(5)).await;
        assert!(matches!(out, NodeTestOutcome::Auth), "凭证错误应判 Auth, got {out:?}");
    }

    #[tokio::test]
    async fn test_node_via_timeout() {
        let hang = hang_proxy().await;
        let (up_base, _) = mock_upstream("OK").await;
        let out = test_node_via(&hang, &up_base, None, Duration::from_millis(400)).await;
        assert!(matches!(out, NodeTestOutcome::Timeout), "代理挂起应超时, got {out:?}");
    }
}

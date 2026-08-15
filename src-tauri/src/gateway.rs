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
    // 星图 任务 B1/B2:per-Key 凭证覆盖/降级 + 凭证确保(缺失或超 12h → 同步签发)
    let line = accel_plan(state, &provider.base_url, &provider.api_key);
    let line = ensure_line_cred(state, line, &provider.base_url, &provider.api_key).await;
    let direct_client = match build_client(&provider) {
        Ok(c) => c,
        Err(e) => return err_resp(StatusCode::INTERNAL_SERVER_ERROR, &format!("build client: {e}")),
    };
    let timeout = Duration::from_secs(provider.timeout_secs.unwrap_or(DEFAULT_TIMEOUT_SECS));
    let line_client = match &line {
        Some((l, _)) => match build_line_client(l, timeout) {
            Ok(c) => Some(c),
            Err(e) => return err_resp(StatusCode::INTERNAL_SERVER_ERROR, &format!("build line client: {e}")),
        },
        None => None,
    };
    if let Some((l, pk)) = &line {
        eprintln!("[GW] accel line={} endpoint={} per_key={} (直连兜底开启)", l.id, l.endpoint, pk);
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
    let per_key = line.as_ref().map(|(_, pk)| *pk).unwrap_or(false);
    let first = line_client.as_ref().unwrap_or(&direct_client);
    let upstream = match build_rb(first).send().await {
        Ok(r) => {
            // 代理 407 → 线路凭证无效。per-Key 凭证走重签判别(星图 resolve_407:重签/降级/直连);
            // legacy/custom 凭证人话化 502,不换直连(避免绕过用户指定的线路)。
            if used_line && r.status() == reqwest::StatusCode::PROXY_AUTHENTICATION_REQUIRED {
                if !per_key {
                    eprintln!("[GW] line 凭证无效(407)");
                    return err_resp(StatusCode::BAD_GATEWAY, "节点凭证无效");
                }
                let line_ref = &line.as_ref().expect("used_line ⇒ line").0;
                let retry_line = match resolve_407_perkey(state, &provider.api_key, line_ref, timeout).await {
                    Resolve407::Invalid => return err_resp(StatusCode::BAD_GATEWAY, "节点凭证无效"),
                    Resolve407::NewClient(c) => Some(c),
                    Resolve407::Direct => None,
                };
                let client = retry_line.as_ref().unwrap_or(&direct_client);
                match build_rb(client).send().await {
                    // 重签凭证重试仍 407 → 人话化收束(不无限重试)
                    Ok(r2) if retry_line.is_some() && r2.status() == reqwest::StatusCode::PROXY_AUTHENTICATION_REQUIRED => {
                        eprintln!("[GW] 重签后仍 407");
                        return err_resp(StatusCode::BAD_GATEWAY, "节点凭证无效");
                    }
                    Ok(r2) => r2,
                    Err(e2) if e2.is_timeout() => return err_resp(StatusCode::GATEWAY_TIMEOUT, "upstream timeout"),
                    Err(e2) => {
                        eprintln!("[GW] ✗ upstream ERR: {e2}");
                        return err_resp(StatusCode::BAD_GATEWAY, "upstream unreachable");
                    }
                }
            } else {
                r
            }
        }
        Err(e) => {
            if used_line && proxy_auth_error(&e) {
                // CONNECT 阶段的 407 以 Err(hyper ProxyAuthRequired) 形态出现:per-Key 同判别
                if !per_key {
                    eprintln!("[GW] line 代理认证失败: {e}");
                    return err_resp(StatusCode::BAD_GATEWAY, "节点凭证无效");
                }
                eprintln!("[GW] per-Key 代理认证失败,重签判别: {e}");
                let line_ref = &line.as_ref().expect("used_line ⇒ line").0;
                let retry_line = match resolve_407_perkey(state, &provider.api_key, line_ref, timeout).await {
                    Resolve407::Invalid => return err_resp(StatusCode::BAD_GATEWAY, "节点凭证无效"),
                    Resolve407::NewClient(c) => Some(c),
                    Resolve407::Direct => None,
                };
                let client = retry_line.as_ref().unwrap_or(&direct_client);
                match build_rb(client).send().await {
                    Ok(r2) if retry_line.is_some() && r2.status() == reqwest::StatusCode::PROXY_AUTHENTICATION_REQUIRED => {
                        eprintln!("[GW] 重签后仍 407");
                        return err_resp(StatusCode::BAD_GATEWAY, "节点凭证无效");
                    }
                    Ok(r2) => r2,
                    Err(e2) if e2.is_timeout() => return err_resp(StatusCode::GATEWAY_TIMEOUT, "upstream timeout"),
                    Err(e2) => {
                        eprintln!("[GW] ✗ upstream ERR: {e2}");
                        return err_resp(StatusCode::BAD_GATEWAY, "upstream unreachable");
                    }
                }
            } else if used_line {
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

// ── 阶段 4 加速装配(任务书 §五)+ 星图 任务 B:per-Key 凭证────────

/// 每账号节点凭证签发超过该时长视为过期,凭证确保段将重签(12h)。
const CRED_STALE_SECS: i64 = 12 * 3600;

/// 判断当前请求应走哪条加速线路(返回线路 + 凭证是否为 per-Key,供 407 判别):
/// - mode=custom → 自定义节点(全量走代理,凭证从 accel-credentials.json 注入;恒非 per-Key);
/// - mode=official → 按供应商 base_url 命中的官方线路:
///   有 per-Key 项且未降级 → 覆盖为该账号凭证;已降级 → None(直连,不再打节点);
///   无项但有 legacy → 保留共享凭证(老用户平滑);无项无 legacy → None(由凭证确保段尝试签发);
/// - mode=off / 未命中 → 直连(None)。
fn accel_plan(state: &AppState, base_url: &str, api_key: &str) -> Option<(AccLine, bool)> {
    let cfg = state.accel.lock().unwrap();
    match cfg.mode.as_str() {
        "custom" => {
            let endpoint = cfg.custom_node.trim();
            if endpoint.is_empty() {
                None
            } else {
                Some((
                    AccLine {
                        id: "custom".into(),
                        name: "自定义节点".into(),
                        endpoint: endpoint.to_string(),
                        scope: Vec::new(),
                        priority: 0,
                        enabled: true,
                        credential: crate::acclines::load_credentials(&state.codex_home),
                    },
                    false,
                ))
            }
        }
        "official" => {
            let line = {
                let lines = state.health.lines.lock().unwrap();
                crate::acclines::match_line(base_url, &lines).cloned()
            };
            let mut line = line?;
            let st = state.nodecreds.read().unwrap();
            match st.get_for_key(api_key) {
                Some(entry) if !entry.degraded_to_direct => {
                    // per-Key 覆盖:替换 acclines 注入的共享凭证
                    line.credential = Some(Cred { user: entry.user.clone(), pass: entry.pass.clone() });
                    Some((line, true))
                }
                Some(_) => None, // 已降级:本请求直接走直连
                None => {
                    if st.legacy_cred().is_some() {
                        Some((line, false)) // 老用户平滑:保留共享凭证兜底
                    } else {
                        None // 无凭证可用 → 直连(凭证确保段会尝试签发)
                    }
                }
            }
        }
        _ => None,
    }
}

/// 签发外呼统一限 5s(nodecreds 内建 10s,这里收紧为网关内联预算;超时视作不可达)。
async fn issue_timed(base: &str, api_key: &str) -> Result<crate::nodecreds::NodeCred, crate::nodecreds::IssueErr> {
    match tokio::time::timeout(Duration::from_secs(5), crate::nodecreds::issue_node_cred(base, api_key)).await {
        Ok(r) => r,
        Err(_) => Err(crate::nodecreds::IssueErr::Unreachable("签发超时(5s)".into())),
    }
}

/// 记降级:store 该 key 项 degraded_to_direct=true(快照若带配额数字一并回写)+ 落盘。
/// 无该项(如 legacy 用户)则无项可记,no-op。pass 永不进日志。
fn mark_degraded(state: &AppState, api_key: &str, snap: Option<&crate::nodecreds::QuotaSnapshot>) {
    let mut st = state.nodecreds.write().unwrap();
    if let Some(e) = st.creds.get_mut(&crate::nodecreds::hash_key(api_key)) {
        e.degraded_to_direct = true;
        if let Some(s) = snap {
            if let Some(u) = s.quota_used_bytes {
                e.quota_used_bytes = u;
            }
            if let Some(t) = s.quota_total_bytes {
                e.quota_total_bytes = t;
            }
        }
        let _ = crate::nodecreds::save_store(&state.codex_home, &st);
    }
}

/// 凭证确保段(星图 任务 B2):official 命中线但 store 无该 key 项(或签发超 12h)→
/// 同步签发(5s 超时,no_proxy):
/// - Ok → set_for_key + save_store + 覆盖凭证(per-Key);
/// - Err(Unreachable) → 本请求跳线直连(不报错,日志);legacy 凭证线保留(老用户平滑);
/// - Err(QuotaFull/KeyInvalid) → 跳线直连 + 记 degraded。
async fn ensure_line_cred(
    state: &AppState,
    line: Option<(AccLine, bool)>,
    base_url: &str,
    api_key: &str,
) -> Option<(AccLine, bool)> {
    let mode = {
        let cfg = state.accel.lock().unwrap();
        cfg.mode.clone()
    };
    if mode != "official" || api_key.trim().is_empty() {
        return line;
    }
    // 快照判定:该 key 项是否降级 / 是否需要(重)签发
    let (degraded, needs_issue) = {
        let st = state.nodecreds.read().unwrap();
        match st.get_for_key(api_key) {
            Some(e) if e.degraded_to_direct => (true, false),
            Some(e) => (false, chrono::Utc::now().timestamp() - e.issued_at > CRED_STALE_SECS),
            None => (false, true),
        }
    };
    if degraded {
        return None; // 已降级:直连,不再签发
    }
    if !needs_issue {
        return line; // 新鲜项:accel_plan 已完成 per-Key 覆盖
    }
    // 无线路时再取一次命中线(accel_plan 的 None 含「无项无 legacy」可签发场景)
    let base_line = match &line {
        Some((l, _)) => Some(l.clone()),
        None => {
            let lines = state.health.lines.lock().unwrap();
            crate::acclines::match_line(base_url, &lines).cloned()
        }
    };
    let Some(mut l) = base_line else {
        return None; // 未命中官方线路:直连,不签发
    };
    match issue_timed(&crate::server::issue_base(), api_key).await {
        Ok(cred) => {
            {
                let mut st = state.nodecreds.write().unwrap();
                st.set_for_key(api_key, cred.clone());
                let _ = crate::nodecreds::save_store(&state.codex_home, &st);
            }
            eprintln!("[GW] 每账号节点凭证已签发并落盘");
            l.credential = Some(Cred { user: cred.user, pass: cred.pass });
            Some((l, true))
        }
        Err(crate::nodecreds::IssueErr::Unreachable(e)) => {
            eprintln!("[GW] 节点凭证签发不可达({e}),本请求跳线直连");
            match line {
                Some((l, pk)) if !pk => Some((l, pk)), // legacy 共享凭证线保留
                _ => None,
            }
        }
        Err(crate::nodecreds::IssueErr::QuotaFull(snap)) => {
            eprintln!("[GW] 节点凭证签发:配额满,该 Key 记降级并本请求直连");
            mark_degraded(state, api_key, snap.as_ref());
            None
        }
        Err(crate::nodecreds::IssueErr::KeyInvalid) => {
            eprintln!("[GW] 节点凭证签发:Key 无效,该 Key 记降级并本请求直连");
            mark_degraded(state, api_key, None);
            None
        }
    }
}

/// 407 判别的结果:重签成功(新凭证 line client)/本请求直连/凭证无效(维持 502)。
enum Resolve407 {
    NewClient(reqwest::Client),
    Direct,
    Invalid,
}

/// per-Key 凭证的 407 判别(星图 任务 B3;安全前提同换线重试:407 在隧道握手阶段,
/// 上游未收到任何字节,故重试/换直连都不会重复副作用):
/// - 重签 Ok → 新凭证重建 line_client,由调用方重试原请求一次;
/// - Err(QuotaFull) → store 该 key degraded_to_direct=true + 落盘,本请求直连;
/// - Err(KeyInvalid) → 维持 502「节点凭证无效」(不绕过用户指定线路);
/// - Err(Unreachable) → 本请求直连。
/// legacy/custom 凭证的 407 不进本函数(调用方维持原 502 行为)。
async fn resolve_407_perkey(
    state: &AppState,
    api_key: &str,
    line: &AccLine,
    timeout: Duration,
) -> Resolve407 {
    eprintln!("[GW] 407 判别:重签每账号凭证");
    match issue_timed(&crate::server::issue_base(), api_key).await {
        Ok(cred) => {
            {
                let mut st = state.nodecreds.write().unwrap();
                st.set_for_key(api_key, cred.clone());
                let _ = crate::nodecreds::save_store(&state.codex_home, &st);
            }
            let l = AccLine {
                credential: Some(Cred { user: cred.user, pass: cred.pass }),
                ..line.clone()
            };
            match build_line_client(&l, timeout) {
                Ok(c) => Resolve407::NewClient(c),
                Err(e) => {
                    eprintln!("[GW] 重签后建线失败({e}),本请求直连");
                    Resolve407::Direct
                }
            }
        }
        Err(crate::nodecreds::IssueErr::QuotaFull(snap)) => {
            mark_degraded(state, api_key, snap.as_ref());
            eprintln!("[GW] 407 判别:配额满,该 Key 降级直连并落盘");
            Resolve407::Direct
        }
        Err(crate::nodecreds::IssueErr::KeyInvalid) => {
            eprintln!("[GW] 407 判别:Key 无效,维持 502(不绕过用户指定线路)");
            Resolve407::Invalid
        }
        Err(crate::nodecreds::IssueErr::Unreachable(e)) => {
            eprintln!("[GW] 407 判别:节点不可达({e}),本请求直连");
            Resolve407::Direct
        }
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
            nodecreds: std::sync::Arc::new(std::sync::RwLock::new(crate::nodecreds::Store::empty())),
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
            nodecreds: s.nodecreds.clone(),
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

    // ① 命中走代理(legacy 共享凭证场景):上游应经代理转发收到请求。
    // 星图后:无 per-Key 项但有 legacy → 保留共享凭证;签发外呼指向死端口(Unreachable)不干扰。
    #[tokio::test]
    async fn accel_hit_routes_through_proxy() {
        let _g = crate::server::set_issue_base_for_tests(crate::server::DEAD_ISSUE_BASE);
        let (up_base, up_seen) = mock_upstream("PROXIED_BODY").await;
        let (px_url, px_seen) = mock_proxy(Some(("u", "p"))).await;
        let (state, providers_path, root) = make_state("accel-hit");
        add_provider(&providers_path, &up_base, "sk-line");
        state.nodecreds.write().unwrap().legacy =
            Some(Cred { user: "u".into(), pass: "p".into() });
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
    // 星图后:无凭证项 → 确保段向死端口签发(Unreachable)跳线,坏线本就连不上 → 直连兜底不变。
    #[tokio::test]
    async fn accel_bad_line_retries_direct_and_stream_complete() {
        let _g = crate::server::set_issue_base_for_tests(crate::server::DEAD_ISSUE_BASE);
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

    // ④ legacy 凭证错误 → 代理 407 → 错误人话化(且不换直连绕过线路)。星图后 legacy 407 行为不变。
    #[tokio::test]
    async fn accel_wrong_cred_proxy_407_humanized() {
        let _g = crate::server::set_issue_base_for_tests(crate::server::DEAD_ISSUE_BASE);
        let (up_base, up_seen) = mock_upstream("SHOULD_NOT_REACH").await;
        let (px_url, px_seen) = mock_proxy(Some(("u", "right"))).await;
        let (state, providers_path, root) = make_state("accel-407");
        add_provider(&providers_path, &up_base, "sk-wrong");
        // legacy 在册(老用户)→ 线路保留共享凭证;该凭证在代理侧为错
        state.nodecreds.write().unwrap().legacy =
            Some(Cred { user: "u".into(), pass: "wrong".into() });
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

    // ── 星图 任务 B:per-Key 凭证覆盖 + 407 判别 + 降级 ──

    /// 往 store 放该 key 的 per-Key 项(issued_at=now,即「新鲜」)。
    fn put_cred(state: &AppState, api_key: &str, user: &str, pass: &str, degraded: bool) {
        let mut c = crate::server::test_node_cred(user, pass);
        c.degraded_to_direct = degraded;
        state.nodecreds.write().unwrap().set_for_key(api_key, c);
    }

    // ⑤ per-Key 覆盖:store 有新鲜项 → 代理请求带该凭证(而非线路共享凭证)。
    // 共享凭证在代理侧为错;若覆盖失效会 407→判别→mock 签发 401→502,断言 200 即证明覆盖生效。
    #[tokio::test]
    async fn per_key_cred_overrides_shared_line_cred() {
        let issue = crate::server::spawn_issue_mock("401 Unauthorized", r#"{"error":"x"}"#).await;
        let _g = crate::server::set_issue_base_for_tests(&issue);
        let (up_base, up_seen) = mock_upstream("PK_BODY").await;
        let (px_url, px_seen) = mock_proxy(Some(("pk-user", "pk-pass"))).await;
        let (state, providers_path, root) = make_state("pk-override");
        add_provider(&providers_path, &up_base, "sk-pk-override-0001");
        put_cred(&state, "sk-pk-override-0001", "pk-user", "pk-pass", false);
        set_accel(
            &state,
            "official",
            vec![test_line("l1", &px_url, &["127.0.0.1"], Some(Cred { user: "shared".into(), pass: "shared-wrong".into() }))],
            "",
        );

        let resp = proxy_responses(State(Arc::new(state)), req_post_responses("{}").await).await;
        assert_eq!(resp.status(), StatusCode::OK, "per-Key 凭证应被代理接受");
        let bytes = axum::body::to_bytes(resp.into_body(), usize::MAX).await.unwrap();
        assert_eq!(&bytes[..], b"PK_BODY");
        tokio::time::sleep(std::time::Duration::from_millis(150)).await;
        assert!(!px_seen.lock().unwrap().is_empty(), "应经代理转发");
        assert_eq!(up_seen.lock().unwrap().len(), 1);
        let _ = std::fs::remove_dir_all(&root);
    }

    // ⑥ per-Key 407 → 重签判得配额满 → store 记降级,本请求改直连且响应完整。
    #[tokio::test]
    async fn per_key_407_quota_full_degrades_direct() {
        let issue = crate::server::spawn_issue_mock(
            "403 Forbidden",
            r#"{"error":"该账号本月已用满 10G","quotaUsedBytes":777,"quotaTotalBytes":888}"#,
        )
        .await;
        let _g = crate::server::set_issue_base_for_tests(&issue);
        let (up_base, up_seen) = mock_upstream("DIRECT_FULL_BODY_9876543210").await;
        let (px_url, px_seen) = mock_proxy(Some(("right", "right"))).await;
        let (state, providers_path, root) = make_state("pk-403");
        add_provider(&providers_path, &up_base, "sk-pk-full-0002");
        put_cred(&state, "sk-pk-full-0002", "stale-user", "stale-pass", false); // 代理侧为错 → 407
        set_accel(
            &state,
            "official",
            vec![test_line("l1", &px_url, &["127.0.0.1"], Some(Cred { user: "x".into(), pass: "y".into() }))],
            "",
        );

        let resp = proxy_responses(State(Arc::new(state.clone())), req_post_responses("{}").await).await;
        assert_eq!(resp.status(), StatusCode::OK);
        let bytes = axum::body::to_bytes(resp.into_body(), usize::MAX).await.unwrap();
        assert_eq!(&bytes[..], b"DIRECT_FULL_BODY_9876543210", "配额满应降级直连且响应完整");
        tokio::time::sleep(std::time::Duration::from_millis(150)).await;
        assert_eq!(up_seen.lock().unwrap().len(), 1, "直连重试恰好命中上游一次");
        assert!(!px_seen.lock().unwrap().is_empty(), "首发应打到代理(收到 407)");
        let entry = state.nodecreds.read().unwrap().get_for_key("sk-pk-full-0002").cloned().unwrap();
        assert!(entry.degraded_to_direct, "QuotaFull 应记 degraded_to_direct");
        assert_eq!(entry.quota_used_bytes, 777, "快照 used 应回写");
        let _ = std::fs::remove_dir_all(&root);
    }

    // ⑦ per-Key 407 → 重签 Ok → 新凭证重建线客户端,原请求重试一次成功。
    #[tokio::test]
    async fn per_key_407_reissue_retries_with_new_cred() {
        let issue = crate::server::spawn_issue_mock(
            "200 OK",
            r#"{"user":"fresh-user","pass":"fresh-pass","quotaTotalBytes":50,"quotaUsedBytes":10,"proxyEndpoint":"http://n"}"#,
        )
        .await;
        let _g = crate::server::set_issue_base_for_tests(&issue);
        let (up_base, up_seen) = mock_upstream("REISSUE_OK").await;
        let (px_url, px_seen) = mock_proxy(Some(("fresh-user", "fresh-pass"))).await;
        let (state, providers_path, root) = make_state("pk-reissue");
        add_provider(&providers_path, &up_base, "sk-pk-reissue-0003");
        put_cred(&state, "sk-pk-reissue-0003", "old-user", "old-pass", false); // 代理侧为错 → 407
        set_accel(
            &state,
            "official",
            vec![test_line("l1", &px_url, &["127.0.0.1"], Some(Cred { user: "x".into(), pass: "y".into() }))],
            "",
        );

        let resp = proxy_responses(State(Arc::new(state.clone())), req_post_responses("{}").await).await;
        assert_eq!(resp.status(), StatusCode::OK, "重签新凭证重试应成功");
        let bytes = axum::body::to_bytes(resp.into_body(), usize::MAX).await.unwrap();
        assert_eq!(&bytes[..], b"REISSUE_OK");
        tokio::time::sleep(std::time::Duration::from_millis(150)).await;
        assert_eq!(up_seen.lock().unwrap().len(), 1);
        assert!(px_seen.lock().unwrap().len() >= 2, "首发 407 + 新凭证重试,代理至少见两次");
        let entry = state.nodecreds.read().unwrap().get_for_key("sk-pk-reissue-0003").cloned().unwrap();
        assert_eq!(entry.quota_total_bytes, 50, "重签后 store 应更新为新凭证配额");
        assert!(!entry.degraded_to_direct);
        let _ = std::fs::remove_dir_all(&root);
    }

    // ⑧ per-Key 407 → 重签判得 Key 无效 → 维持 502 人话化(不绕过线路)。
    #[tokio::test]
    async fn per_key_407_key_invalid_keeps_502() {
        let issue = crate::server::spawn_issue_mock("401 Unauthorized", r#"{"error":"Key 无效或未充值"}"#).await;
        let _g = crate::server::set_issue_base_for_tests(&issue);
        let (up_base, up_seen) = mock_upstream("SHOULD_NOT_REACH").await;
        let (px_url, px_seen) = mock_proxy(Some(("right", "right"))).await;
        let (state, providers_path, root) = make_state("pk-401");
        add_provider(&providers_path, &up_base, "sk-pk-invalid-0004");
        put_cred(&state, "sk-pk-invalid-0004", "stale-user", "stale-pass", false);
        set_accel(
            &state,
            "official",
            vec![test_line("l1", &px_url, &["127.0.0.1"], Some(Cred { user: "x".into(), pass: "y".into() }))],
            "",
        );

        let resp = proxy_responses(State(Arc::new(state)), req_post_responses("{}").await).await;
        assert_eq!(resp.status(), StatusCode::BAD_GATEWAY);
        let bytes = axum::body::to_bytes(resp.into_body(), usize::MAX).await.unwrap();
        assert!(String::from_utf8_lossy(&bytes).contains("节点凭证无效"));
        tokio::time::sleep(std::time::Duration::from_millis(150)).await;
        assert!(up_seen.lock().unwrap().is_empty(), "KeyInvalid 不应绕线直连命中上游");
        assert!(!px_seen.lock().unwrap().is_empty());
        let _ = std::fs::remove_dir_all(&root);
    }

    // ⑨ degraded_to_direct → 该请求直接走直连,代理零请求。
    #[tokio::test]
    async fn degraded_entry_goes_direct_zero_proxy_hits() {
        let _g = crate::server::set_issue_base_for_tests(crate::server::DEAD_ISSUE_BASE);
        let (up_base, up_seen) = mock_upstream("DEGRADED_DIRECT_OK").await;
        let (px_url, px_seen) = mock_proxy(Some(("u", "p"))).await;
        let (state, providers_path, root) = make_state("pk-degraded");
        add_provider(&providers_path, &up_base, "sk-pk-degraded-0005");
        put_cred(&state, "sk-pk-degraded-0005", "u", "p", true); // 已降级
        set_accel(
            &state,
            "official",
            vec![test_line("l1", &px_url, &["127.0.0.1"], Some(Cred { user: "u".into(), pass: "p".into() }))],
            "",
        );

        let resp = proxy_responses(State(Arc::new(state)), req_post_responses("{}").await).await;
        assert_eq!(resp.status(), StatusCode::OK);
        let bytes = axum::body::to_bytes(resp.into_body(), usize::MAX).await.unwrap();
        assert_eq!(&bytes[..], b"DEGRADED_DIRECT_OK");
        tokio::time::sleep(std::time::Duration::from_millis(150)).await;
        assert!(px_seen.lock().unwrap().is_empty(), "已降级:代理应零请求");
        assert_eq!(up_seen.lock().unwrap().len(), 1, "直连恰好命中上游一次");
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

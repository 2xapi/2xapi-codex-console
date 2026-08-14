use serde::Deserialize;
use std::time::Duration;

/// 已知模型的上下文窗口（token）。上游不返回时用此兜底（命中即填）。
fn known_context(name: &str) -> Option<u64> {
    let n = name.to_ascii_lowercase();
    let has = |sub: &str| n.contains(sub);
    // OpenAI
    if has("gpt-4o") || has("gpt-4-turbo") || has("gpt-4-1106") || has("gpt-4-0125") || has("gpt-4-vision") {
        return Some(128_000);
    }
    if has("o1-mini") || has("o3-mini") { return Some(128_000); }
    if has("o1") || has("o3") { return Some(200_000); }
    if has("gpt-4") { return Some(8_192); }
    if has("gpt-3.5") { return Some(16_385); }
    // Anthropic
    if has("claude") { return Some(200_000); }
    // DeepSeek（含中转的自定义名如 deepseek-v4-flash）
    if has("deepseek") { return Some(64_000); }
    // Google
    if has("gemini-1.5") { return Some(1_000_000); }
    if has("gemini-2") { return Some(2_000_000); }
    if has("gemini") { return Some(32_000); }
    // MiniMax
    if has("minimax") { return Some(1_000_000); }
    // 通义
    if has("qwen") || has("qwq") { return Some(128_000); }
    // 智谱 / 月之暗面 / xAI
    if has("glm") { return Some(128_000); }
    if has("kimi") { return Some(256_000); }
    if has("grok") { return Some(131_072); }
    None
}

fn client() -> reqwest::Client {
    reqwest::Client::builder()
        .timeout(Duration::from_secs(10))
        .no_proxy() // 绕过系统代理
        .build()
        .expect("failed to build HTTP client")
}

/// 探测模型列表。返回 (模型名, 上下文窗口)；上下文优先取上游字段，其次已知表，都没有则 None。
/// 兼容 base_url 是否已含 /v1：先试 /models，再试 /v1/models。
pub async fn probe_endpoint(base_url: &str, api_key: &str) -> Vec<(String, Option<u64>)> {
    let base = base_url.trim_end_matches('/');
    for suffix in ["/models", "/v1/models"] {
        let got = try_probe(&format!("{}{}", base, suffix), api_key).await;
        if !got.is_empty() {
            return got;
        }
    }
    Vec::new()
}

async fn try_probe(url: &str, api_key: &str) -> Vec<(String, Option<u64>)> {
    let resp = match client()
        .get(url)
        .header("Accept", "application/json")
        .header("Authorization", format!("Bearer {}", api_key))
        .send()
        .await
    {
        Ok(r) => r,
        Err(_) => return Vec::new(),
    };

    #[derive(Deserialize)]
    struct ModelsResp {
        #[serde(default)]
        data: Vec<ModelEntry>,
    }
    #[derive(Deserialize)]
    struct ModelEntry {
        #[serde(default)]
        id: String,
        #[serde(default)]
        name: String,
        // 常见上下文字段（不同上游命名不同）；max_tokens 语义歧义（常为输出上限），不采用
        #[serde(default)]
        context_length: Option<u64>,
        #[serde(default)]
        context_window: Option<u64>,
        #[serde(default)]
        max_context_tokens: Option<u64>,
        #[serde(default)]
        max_context_window: Option<u64>,
    }

    if let Ok(parsed) = resp.json::<ModelsResp>().await {
        return parsed
            .data
            .into_iter()
            .filter_map(|m| {
                let name = if !m.id.is_empty() { m.id } else { m.name };
                if name.is_empty() {
                    return None;
                }
                let ctx = m
                    .context_length
                    .or(m.context_window)
                    .or(m.max_context_tokens)
                    .or(m.max_context_window)
                    .or_else(|| known_context(&name));
                Some((name, ctx))
            })
            .collect();
    }
    Vec::new()
}

/// 探测上游支持的 reasoning effort 等级（逐个试 /responses，记录 200 的）。
pub async fn probe_reasoning_levels(base_url: &str, api_key: &str, model: &str) -> Vec<String> {
    let base = base_url.trim_end_matches('/');
    let client = reqwest::Client::builder()
        .timeout(std::time::Duration::from_secs(15))
        .no_proxy()
        .build()
        .unwrap_or_default();
    let candidates = ["low", "medium", "high", "xhigh", "max", "ultra"];
    let mut supported = Vec::new();
    for effort in &candidates {
        for suffix in ["/responses", "/v1/responses"] {
            let url = format!("{base}{suffix}");
            let body = serde_json::json!({"model": model, "input": "1", "stream": false, "reasoning": {"effort": effort}, "max_output_tokens": 1});
            match client.post(&url).header("Authorization", format!("Bearer {api_key}")).json(&body).send().await {
                Ok(r) if r.status().is_success() => { supported.push(effort.to_string()); break; }
                _ => {}
            }
        }
    }
    supported
}

#[cfg(test)]
mod tests {
    use super::*;

    /// 启动一个内存 mock 上游，返回 base_url。同时响应 /models 与 /v1/models。
    async fn spawn_mock() -> String {
        use tokio::net::TcpListener;
        use tokio::io::{AsyncReadExt, AsyncWriteExt};

        let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
        let addr = listener.local_addr().unwrap();
        tokio::spawn(async move {
            loop {
                let Ok((mut sock, _)) = listener.accept().await else { break };
                tokio::spawn(async move {
                    let mut buf = [0u8; 4096];
                    if sock.read(&mut buf).await.is_err() { return; }
                    let body = r#"{"object":"list","data":[
                        {"id":"gpt-4o","object":"model","owned_by":"openai","context_length":128000},
                        {"id":"deepseek-chat","object":"model","owned_by":"deepseek","context_window":64000},
                        {"id":"plain","object":"model","owned_by":"x"}
                    ]}"#;
                    let resp = format!(
                        "HTTP/1.1 200 OK\r\nContent-Type: application/json\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{}",
                        body.len(), body
                    );
                    let _ = sock.write_all(resp.as_bytes()).await;
                });
            }
        });
        format!("http://{}", addr)
    }

    #[tokio::test]
    async fn probe_local_mock_parses_models_and_ctx() {
        let base = spawn_mock().await;
        let r = probe_endpoint(&base, "sk-tmp").await;
        assert_eq!(r.len(), 3, "should find 3 models, got {r:?}");
        assert_eq!(r[0], ("gpt-4o".to_string(), Some(128_000)));
        assert_eq!(r[1], ("deepseek-chat".to_string(), Some(64_000)));
        // 无上下文字段的上游字段 → 已知表兜底；plain 不在表 → None
        assert_eq!(r[2], ("plain".to_string(), None));
    }

    #[tokio::test]
    async fn probe_handles_base_url_with_v1_suffix() {
        let base = spawn_mock().await;
        // base 自带 /v1（opencode 场景）：probe 应先试 {base}/models 成功，不再拼 /v1/v1/models
        let r = probe_endpoint(&format!("{base}/v1"), "sk-tmp").await;
        assert_eq!(r.len(), 3, "base with /v1 should still find models, got {r:?}");
    }

    #[tokio::test]
    async fn probe_returns_empty_for_unreachable() {
        let r = probe_endpoint("http://127.0.0.1:9", "sk-tmp").await;
        assert!(r.is_empty());
    }
}

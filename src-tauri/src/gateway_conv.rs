//! Responses ↔ ChatCompletions 协议转换（M3b，FR-5）。
//!
//! 背景（01-D5）：Codex 恒发 **Responses** 格式。当 `provider.wire_api = chat_completions` 时，
//! 网关在 `/responses` 入口做：
//! - 请求：Responses → ChatCompletions（`input`/`instructions` → `messages`）。
//! - 非流式响应：Chat `choices[0].message` → Responses `output`。
//! - 流式响应：Chat SSE `delta` → Responses SSE（`response.created` / `response.output_text.delta` / `.done` / `response.completed`）。
//!
//! 实现策略：上游响应整体缓冲后转换（保证事件序列正确；增量逐 token 投递为后续优化）。

use serde_json::{json, Value};

/// 转换后的 Chat 请求体 + 是否流式。
pub struct ConvertedRequest {
    pub body: Vec<u8>,
    pub stream: bool,
}

/// Responses 请求体 → ChatCompletions 请求体（FR-5.1）。
pub fn responses_to_chat_request(body: &[u8]) -> Result<ConvertedRequest, String> {
    let v: Value = serde_json::from_slice(body).map_err(|e| format!("非法 responses body: {e}"))?;
    let obj = v.as_object().ok_or("responses body 不是 object")?;

    let mut messages: Vec<Value> = Vec::new();
    // instructions → system 消息
    if let Some(ins) = obj.get("instructions").and_then(|x| x.as_str()) {
        if !ins.is_empty() {
            messages.push(json!({ "role": "system", "content": ins }));
        }
    }
    // input → messages
    match obj.get("input") {
        Some(Value::String(s)) => {
            messages.push(json!({ "role": "user", "content": s }));
        }
        Some(Value::Array(arr)) => {
            for item in arr {
                if let Some(role) = item.get("role").and_then(|x| x.as_str()) {
                    let content = extract_text(item.get("content"));
                    messages.push(json!({ "role": role, "content": content }));
                } else if let Some(s) = item.as_str() {
                    messages.push(json!({ "role": "user", "content": s }));
                }
                // 无法映射的 input 条目类型（如 reasoning）丢弃
            }
        }
        _ => {}
    }

    let mut chat = serde_json::Map::new();
    chat.insert("model".into(), obj.get("model").cloned().unwrap_or(json!("")));
    chat.insert("messages".into(), Value::Array(messages));
    if let Some(s) = obj.get("stream") {
        chat.insert("stream".into(), s.clone());
    }
    for src in ["temperature", "top_p"] {
        if let Some(x) = obj.get(src) {
            chat.insert(src.into(), x.clone());
        }
    }
    if let Some(x) = obj.get("max_output_tokens") {
        chat.insert("max_tokens".into(), x.clone());
    }

    let stream = obj.get("stream").and_then(|x| x.as_bool()).unwrap_or(false);
    let body = serde_json::to_vec(&Value::Object(chat)).map_err(|e| format!("编码 chat body: {e}"))?;
    Ok(ConvertedRequest { body, stream })
}

/// 非流式：ChatCompletions 响应 → Responses 响应（FR-5.2）。
pub fn chat_json_to_responses_json(chat: &[u8]) -> Result<Vec<u8>, String> {
    let v: Value = serde_json::from_slice(chat).map_err(|e| format!("非法 chat body: {e}"))?;
    let text = v
        .get("choices")
        .and_then(|c| c.get(0))
        .and_then(|c| c.get("message"))
        .and_then(|m| m.get("content"))
        .and_then(|c| c.as_str())
        .unwrap_or("");
    let id = v.get("id").and_then(|x| x.as_str()).unwrap_or("").to_string();
    let model = v.get("model").and_then(|x| x.as_str()).unwrap_or("").to_string();
    let finish = v
        .get("choices")
        .and_then(|c| c.get(0))
        .and_then(|c| c.get("finish_reason"))
        .and_then(|x| x.as_str())
        .unwrap_or("stop");
    let status = if finish == "length" { "incomplete" } else { "completed" };
    let created_at = chrono::Utc::now().timestamp();

    let mut resp = serde_json::Map::new();
    // Responses 标准字段（Codex 客户端依赖）：object / created_at / model / error / incomplete_details
    resp.insert("id".into(), json!(id));
    resp.insert("object".into(), json!("response"));
    resp.insert("created_at".into(), json!(created_at));
    resp.insert("status".into(), json!(status));
    resp.insert("model".into(), json!(model));
    resp.insert("error".into(), Value::Null);
    resp.insert("incomplete_details".into(), Value::Null);
    resp.insert(
        "output".into(),
        json!([{ "type": "message", "id": format!("msg_{}", &id), "role": "assistant", "content": [{ "type": "output_text", "text": text }] }]),
    );
    if let Some(u) = v.get("usage") {
        resp.insert("usage".into(), convert_usage(u));
    }
    serde_json::to_vec(&Value::Object(resp)).map_err(|e| format!("编码 responses body: {e}"))
}

/// 流式：ChatCompletions SSE → Responses SSE（FR-5.3）。
pub fn chat_sse_to_responses_sse(chat_sse: &[u8]) -> Vec<u8> {
    let s = String::from_utf8_lossy(chat_sse);
    let mut out = String::new();
    let mut id = String::from("resp-converted");
    let mut text = String::new();
    let mut created = false;
    let mut usage: Option<Value> = None;

    for line in s.lines() {
        let payload = match line.strip_prefix("data:") {
            Some(p) => p.trim().to_string(),
            None => continue,
        };
        if payload == "[DONE]" {
            continue;
        }
        let v: Value = match serde_json::from_str(&payload) {
            Ok(v) => v,
            Err(_) => continue,
        };
        if !created {
            if let Some(i) = v.get("id").and_then(|x| x.as_str()) {
                id = i.to_string();
            }
            emit_event(
                &mut out,
                "response.created",
                &json!({ "type": "response.created", "response": { "id": id.clone(), "object": "response", "status": "in_progress", "output": [] } }),
            );
            created = true;
        }
        if let Some(choice) = v.get("choices").and_then(|c| c.get(0)) {
            if let Some(delta) = choice.get("delta") {
                if let Some(c) = delta.get("content").and_then(|x| x.as_str()) {
                    text.push_str(c);
                    emit_event(
                        &mut out,
                        "response.output_text.delta",
                        &json!({ "type": "response.output_text.delta", "delta": c }),
                    );
                }
            }
            if let Some(u) = choice.get("usage") {
                usage = Some(u.clone());
            }
        }
        if let Some(u) = v.get("usage") {
            usage = Some(u.clone());
        }
    }

    emit_event(
        &mut out,
        "response.output_text.done",
        &json!({ "type": "response.output_text.done", "text": text.clone() }),
    );
    let final_usage = usage.as_ref().map(convert_usage).unwrap_or(json!({}));
    emit_event(
        &mut out,
        "response.completed",
        &json!({ "type": "response.completed", "response": {
            "id": id.clone(), "object": "response", "status": "completed",
            "output": [{ "type": "message", "id": format!("msg_{}", id), "role": "assistant", "content": [{ "type": "output_text", "text": text }] }],
            "usage": final_usage
        } }),
    );
    out.into_bytes()
}

fn convert_usage(u: &Value) -> Value {
    json!({
        "input_tokens": u.get("prompt_tokens").cloned().unwrap_or(json!(0)),
        "output_tokens": u.get("completion_tokens").cloned().unwrap_or(json!(0)),
        "total_tokens": u.get("total_tokens").cloned().unwrap_or(json!(0)),
    })
}

/// 从 Responses 的 content（字符串 或 [{type:..,text:..}]）抽出文本。
fn extract_text(content: Option<&Value>) -> String {
    match content {
        Some(Value::String(s)) => s.clone(),
        Some(Value::Array(arr)) => arr
            .iter()
            .filter_map(|p| p.get("text").and_then(|t| t.as_str()).map(String::from).or_else(|| p.as_str().map(String::from)))
            .collect::<Vec<_>>()
            .join(""),
        _ => String::new(),
    }
}

fn emit_event(out: &mut String, event: &str, data: &Value) {
    out.push_str(&format!("event: {event}\ndata: {}\n\n", data));
}

// ── 流式增量转换（解决 Codex 超时：逐块转换而非缓冲全部）──

pub struct SseConvState {
    buffer: String,
    created: bool,
    text: String,
    id: String,
}

impl SseConvState {
    pub fn new() -> Self {
        Self { buffer: String::new(), created: false, text: String::new(), id: "resp-conv".into() }
    }
    pub fn feed(&mut self, chunk: &[u8]) -> Vec<String> {
        self.buffer.push_str(&String::from_utf8_lossy(chunk));
        let mut out = Vec::new();
        while let Some(pos) = self.buffer.find('\n') {
            let line = self.buffer[..pos].trim().to_string();
            self.buffer = self.buffer[pos + 1..].to_string();
            self.proc(&line, &mut out);
        }
        out
    }
    pub fn finish(&mut self) -> Vec<String> {
        let mut out = self.feed(b"\n");
        out.push(fmt("response.output_text.done", &json!({"type":"response.output_text.done","text":self.text.clone()})));
        out.push(fmt("response.completed", &json!({"type":"response.completed","response":{"id":self.id.clone(),"status":"completed","output":[{"type":"message","role":"assistant","content":[{"type":"output_text","text":self.text.clone()}]}],"usage":{}}})));
        out
    }
    fn proc(&mut self, line: &str, out: &mut Vec<String>) {
        let p = match line.strip_prefix("data:") { Some(p) => p.trim(), None => return };
        if p == "[DONE]" { return; }
        let v: Value = match serde_json::from_str(p) { Ok(v) => v, Err(_) => return };
        if !self.created {
            if let Some(i) = v.get("id").and_then(|x| x.as_str()) { self.id = i.to_string(); }
            out.push(fmt("response.created", &json!({"type":"response.created","response":{"id":self.id.clone(),"status":"in_progress","output":[]}})));
            self.created = true;
        }
        if let Some(ch) = v.get("choices").and_then(|c| c.get(0)).and_then(|c| c.get("delta")).and_then(|d| d.get("content")).and_then(|c| c.as_str()) {
            self.text.push_str(ch);
            out.push(fmt("response.output_text.delta", &json!({"type":"response.output_text.delta","delta":ch})));
        }
    }
}

fn fmt(event: &str, data: &Value) -> String { format!("event: {}\ndata: {}\n\n", event, data) }

// ── 单测（FR-5.1~5.4）─────────────────────────────────────────

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn converts_request_messages() {
        // Responses：instructions + input(数组消息)
        let body = br#"{"model":"gpt-x","instructions":"be brief","input":[{"type":"message","role":"user","content":[{"type":"input_text","text":"Hello"}]}],"stream":false,"temperature":0.5,"max_output_tokens":100}"#;
        let conv = responses_to_chat_request(body).unwrap();
        assert!(!conv.stream);
        let v: Value = serde_json::from_slice(&conv.body).unwrap();
        let msgs = v.get("messages").unwrap().as_array().unwrap();
        assert_eq!(msgs.len(), 2);
        assert_eq!(msgs[0]["role"], "system");
        assert_eq!(msgs[0]["content"], "be brief");
        assert_eq!(msgs[1]["role"], "user");
        assert_eq!(msgs[1]["content"], "Hello");
        assert_eq!(v["model"], "gpt-x");
        assert_eq!(v["max_tokens"], 100);
        assert_eq!(v["temperature"], 0.5);
        // 未知 Responses 字段（max_output_tokens 之外）不应泄漏为 chat 字段
        assert!(v.get("input").is_none());
    }

    #[test]
    fn converts_request_string_input() {
        let body = br#"{"model":"m","input":"Hi there","stream":true}"#;
        let conv = responses_to_chat_request(body).unwrap();
        assert!(conv.stream);
        let v: Value = serde_json::from_slice(&conv.body).unwrap();
        assert_eq!(v["messages"][0]["role"], "user");
        assert_eq!(v["messages"][0]["content"], "Hi there");
        assert_eq!(v["stream"], true);
    }

    #[test]
    fn converts_nonstream_response() {
        let chat = br#"{"id":"chat-1","choices":[{"index":0,"message":{"role":"assistant","content":"Hi back"},"finish_reason":"stop"}],"usage":{"prompt_tokens":3,"completion_tokens":2,"total_tokens":5}}"#;
        let out = chat_json_to_responses_json(chat).unwrap();
        let v: Value = serde_json::from_slice(&out).unwrap();
        assert_eq!(v["id"], "chat-1");
        assert_eq!(v["object"], "response"); // 标准字段：Codex 客户端依赖
        assert!(v["created_at"].is_i64());
        assert_eq!(v["status"], "completed");
        assert_eq!(v["output"][0]["content"][0]["text"], "Hi back");
        assert_eq!(v["usage"]["input_tokens"], 3);
        assert_eq!(v["usage"]["output_tokens"], 2);
    }

    /// FR-5.4：round-trip——用户文本经请求转换后能被 chat 侧读到，chat 响应文本能转回 responses。
    #[test]
    fn round_trip_text_preserved() {
        let req = br#"{"model":"m","input":[{"type":"message","role":"user","content":[{"type":"input_text","text":"ping"}]}]}"#;
        let conv = responses_to_chat_request(req).unwrap();
        let chat_req: Value = serde_json::from_slice(&conv.body).unwrap();
        assert_eq!(chat_req["messages"][0]["content"], "ping"); // 请求侧文本一致
        // 模拟 chat 上游回复
        let chat_resp = br#"{"id":"c","choices":[{"index":0,"message":{"role":"assistant","content":"pong"},"finish_reason":"stop"}]}"#;
        let resp = chat_json_to_responses_json(chat_resp).unwrap();
        let v: Value = serde_json::from_slice(&resp).unwrap();
        assert_eq!(v["output"][0]["content"][0]["text"], "pong"); // 响应侧文本一致
    }

    /// FR-5.3：流式 SSE 转换——事件类型与文本累积正确。
    #[test]
    fn converts_stream_sse() {
        let sse = b"data: {\"id\":\"chatcmpl-1\",\"choices\":[{\"index\":0,\"delta\":{\"role\":\"assistant\",\"content\":\"Hel\"}}]}\n\ndata: {\"id\":\"chatcmpl-1\",\"choices\":[{\"index\":0,\"delta\":{\"content\":\"lo\"}}]}\n\ndata: {\"id\":\"chatcmpl-1\",\"choices\":[{\"index\":0,\"finish_reason\":\"stop\"}],\"usage\":{\"prompt_tokens\":2,\"completion_tokens\":2,\"total_tokens\":4}}\n\ndata: [DONE]\n\n";
        let out = chat_sse_to_responses_sse(sse);
        let s = String::from_utf8(out).unwrap();
        assert!(s.contains("event: response.created"), "缺 created:\n{s}");
        assert!(s.contains(r#""object":"response""#), "created 事件缺 object 字段:\n{s}");
        assert!(s.contains("event: response.output_text.delta"));
        assert!(s.contains(r#""delta":"Hel""#));
        assert!(s.contains(r#""delta":"lo""#));
        assert!(s.contains("event: response.output_text.done"));
        assert!(s.contains(r#""text":"Hello""#), "累积文本应为 Hello:\n{s}");
        assert!(s.contains("event: response.completed"));
        assert!(s.contains(r#""status":"completed""#));
        assert!(s.contains(r#""input_tokens":2"#));
    }
}

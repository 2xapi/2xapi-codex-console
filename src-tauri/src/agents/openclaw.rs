//! OpenClaw adapter(叠加托管,D1 拍板形态;调研报告 §四)。
//!
//! 载体:`~/.openclaw/openclaw.json` 的 `models.providers["2xapi-gateway"]` 条目(upsert);
//! 派生注册表 `agents/<id>/agent/models.json` 为 OpenClaw 自管理,**不碰**。
//! 默认指针 `agents.defaults.model` 按 D1——仅原值缺失才切,否则不动并响应 suggested。
//!
//! 已知首版边界(交接日志备案):OpenClaw 配置为 JSON5(注释/尾逗号),首版仅支持「文件
//! 不存在(直写新文件)」或「合法标准 JSON(合并)」两种;含注释时拒绝写入并提示整理
//! (不做保格式合并,后续批次参照 cc-switch RtJSON 手法增强)。协议统一走
//! openai-completions(网关 `/openclaw/*` 专属入口,Anthropic 直连形态后续批次)。

use serde_json::{json, Map, Value};
use std::path::{Path, PathBuf};

pub type OpError = (u16, String, String);

const GATEWAY_BASE: &str = "http://127.0.0.1:8787";
const PLACEHOLDER_KEY: &str = "2xapi-gateway-managed";
pub const PROVIDER_ID: &str = "2xapi-gateway";

/// 配置文件路径:`<oclaw_home>/openclaw.json`(oclaw_home=~/.openclaw 根,测试传 tempdir)。
pub fn config_path(oclaw_home: &Path) -> PathBuf {
    oclaw_home.join("openclaw.json")
}

fn slug(name: &str) -> String {
    let s: String = name
        .trim()
        .chars()
        .map(|c| {
            if c.is_ascii_alphanumeric() || c == '.' || c == '_' || c == '-' {
                c
            } else {
                '-'
            }
        })
        .collect();
    let s = s.trim_matches('-').to_string();
    if s.is_empty() {
        "model".into()
    } else {
        s
    }
}

fn read_root(oclaw_home: &Path) -> Result<Map<String, Value>, OpError> {
    let path = config_path(oclaw_home);
    if !path.exists() {
        return Ok(Map::new());
    }
    let raw = std::fs::read_to_string(&path).map_err(|e| (500, "E_IO".into(), e.to_string()))?;
    let v: Value = serde_json::from_str(&raw)
        .map_err(|_| (422, "E_CONFIG_JSON5".into(), "openclaw.json 含注释或 JSON5 语法,暂不支持安全合并,请先整理为标准 JSON(或移走由本软件重写)".into()))?;
    Ok(v.as_object().cloned().unwrap_or_default())
}

fn write_root(
    oclaw_home: &Path,
    backup_dir: &Path,
    root: &Map<String, Value>,
    purpose: &str,
) -> Result<bool, OpError> {
    let path = config_path(oclaw_home);
    let new_text = serde_json::to_string_pretty(&Value::Object(root.clone()))
        .map_err(|e| (500, "E_IO".into(), e.to_string()))?;
    if path.exists() {
        let cur = std::fs::read_to_string(&path).unwrap_or_default();
        if cur == new_text {
            return Ok(false);
        }
        crate::config::backup_file(&path, backup_dir, "config-apply", purpose)
            .map_err(|e| (500, "E_IO".into(), e.to_string()))?;
    } else if let Some(p) = path.parent() {
        std::fs::create_dir_all(p).map_err(|e| (500, "E_IO".into(), e.to_string()))?;
    }
    std::fs::write(&path, new_text).map_err(|e| (500, "E_IO".into(), e.to_string()))?;
    Ok(true)
}

fn find_provider(
    providers_path: &Path,
    provider_id: &str,
) -> Result<crate::providers::Provider, OpError> {
    crate::providers::load(providers_path)
        .providers
        .into_iter()
        .find(|p| p.id == provider_id)
        .ok_or_else(|| {
            (
                404,
                "E_NO_PROVIDER".into(),
                format!("供应商不存在: {provider_id}"),
            )
        })
}

/// 托管态:条目存在性 + 默认指针归属。
pub fn state(oclaw_home: &Path) -> Value {
    let root = match read_root(oclaw_home) {
        Ok(r) => r,
        Err((_, code, msg)) => return json!({ "hosting": null, "warn": format!("[{code}] {msg}") }),
    };
    let entry = root
        .get("models")
        .and_then(|m| m.get("providers"))
        .and_then(|p| p.get(PROVIDER_ID));
    let hosting = entry.map(|e| {
        json!({
            "providerId": PROVIDER_ID,
            "baseUrl": e.get("baseUrl"),
            "api": e.get("api"),
            "models": e.get("models"),
        })
    });
    let default_model = root
        .get("agents")
        .and_then(|a| a.get("defaults"))
        .and_then(|d| d.get("model"))
        .and_then(|v| v.as_str())
        .unwrap_or("");
    json!({
        "hosting": hosting,
        "defaultModel": default_model,
        "defaultModelIsOurs": default_model.starts_with(&format!("{PROVIDER_ID}/")),
    })
}

pub fn host(
    oclaw_home: &Path,
    backup_dir: &Path,
    providers_path: &Path,
    provider_id: &str,
    way: &str,
) -> Result<Value, OpError> {
    if way != "gateway" && way != "direct" {
        return Err((
            400,
            "E_BAD_WAY".into(),
            "未知托管方式,仅支持 gateway / direct".into(),
        ));
    }
    let provider = find_provider(providers_path, provider_id)?;
    if provider.model.trim().is_empty() {
        return Err((
            422,
            "E_NO_MODEL".into(),
            "该供应商未配置默认模型,请先在编辑里拉取模型或手填".into(),
        ));
    }
    if way == "direct" && provider.base_url.trim().is_empty() {
        return Err((
            422,
            "E_NO_BASE_URL".into(),
            "该供应商未配置 API 地址".into(),
        ));
    }
    // Anthropic 协议供应商:网关通路(chat→anthropic 转换未做)明确拒绝不静默;直连已原生支持
    if way == "gateway" && provider.wire_api == crate::providers::WireApi::Anthropic {
        return Err((
            400,
            "E_ANTHROPIC_DIRECT_ONLY".into(),
            "该供应商为 Anthropic 协议,OpenClaw 网关通路暂不支持,请改用直连方式(已支持 Anthropic)"
                .into(),
        ));
    }

    let mut root = read_root(oclaw_home)?;
    let (base_url, api_key, api_kind, key_note) = if way == "gateway" {
        (
            format!("{GATEWAY_BASE}/openclaw/v1"),
            PLACEHOLDER_KEY.to_string(),
            "openai-completions",
            "占位(真实 Key 只在网关)",
        )
    } else if provider.wire_api == crate::providers::WireApi::Anthropic {
        // 真机实证:anthropic-messages 自动拼 /v1/messages + x-api-key 头 → 带 /v1 尾的上游去尾防双拼
        (
            provider
                .base_url
                .trim()
                .trim_end_matches('/')
                .trim_end_matches("/v1")
                .to_string(),
            provider.api_key.clone(),
            "anthropic-messages",
            "直连:真实 Key 落盘于 openclaw.json(Anthropic 协议)",
        )
    } else {
        (
            provider.base_url.trim().trim_end_matches('/').to_string(),
            provider.api_key.clone(),
            "openai-completions",
            "直连:真实 Key 落盘于 openclaw.json",
        )
    };

    let model_ids: Vec<(String, String, Option<u64>)> = if provider.models.is_empty() {
        let cw = provider
            .context_window
            .as_deref()
            .and_then(|s| s.parse().ok());
        vec![(slug(&provider.model), provider.model.clone(), cw)]
    } else {
        provider
            .models
            .iter()
            .map(|m| (slug(&m.name), m.name.clone(), m.context_window))
            .collect()
    };
    let models: Vec<Value> = model_ids
        .iter()
        .map(|(id, name, cw)| {
            let mut o = json!({ "id": id, "name": name });
            if let Some(c) = cw {
                o["contextWindow"] = json!(c);
            }
            o
        })
        .collect();
    let entry = json!({
        "baseUrl": base_url,
        "apiKey": api_key,
        "api": api_kind,
        "models": models,
    });
    root.entry("models".to_string())
        .or_insert_with(|| json!({}))
        .as_object_mut()
        .ok_or_else(|| {
            (
                422,
                "E_CONFIG_JSON5".into(),
                "models 段存在但不是对象,拒绝写入".into(),
            )
        })?
        .entry("providers".to_string())
        .or_insert_with(|| json!({}))
        .as_object_mut()
        .ok_or_else(|| {
            (
                422,
                "E_CONFIG_JSON5".into(),
                "models.providers 段存在但不是对象,拒绝写入".into(),
            )
        })?
        .insert(PROVIDER_ID.into(), entry);

    // D1:默认指针仅缺失才切(OpenClaw 语义:指针缺省=官方/引导态);已有第三方值不动
    let existing = root
        .get("agents")
        .and_then(|a| a.get("defaults"))
        .and_then(|d| d.get("model"))
        .and_then(|v| v.as_str())
        .map(|s| s.trim().to_string())
        .unwrap_or_default();
    let mut switched = false;
    if existing.is_empty() {
        root.entry("agents".to_string())
            .or_insert_with(|| json!({}))
            .as_object_mut()
            .unwrap()
            .entry("defaults".to_string())
            .or_insert_with(|| json!({}))
            .as_object_mut()
            .unwrap()
            .insert(
                "model".to_string(),
                json!(format!("{PROVIDER_ID}/{}", model_ids[0].0)),
            );
        switched = true;
    }

    let written = write_root(
        oclaw_home,
        backup_dir,
        &root,
        if switched { "pre-host" } else { "pre-switch" },
    )?;
    Ok(json!({
        "hosted": true, "way": way, "api": api_kind, "switched": !existing.is_empty(),
        "defaultModelSwitched": switched,
        "suggested": !switched,
        "changed": { "config": written },
        "keyNote": key_note,
    }))
}

pub fn unhost(oclaw_home: &Path, backup_dir: &Path) -> Result<Value, OpError> {
    let mut root = read_root(oclaw_home)?;
    let ours_prefix = format!("{PROVIDER_ID}/");
    let removed = root
        .get_mut("models")
        .and_then(|m| m.get_mut("providers"))
        .and_then(|p| p.as_object_mut())
        .map(|m| m.remove(PROVIDER_ID).is_some())
        .unwrap_or(false);
    if !removed {
        return Ok(json!({ "restored": false, "alreadyClean": true }));
    }
    let pointer_removed = root
        .get_mut("agents")
        .and_then(|a| a.get_mut("defaults"))
        .and_then(|d| d.as_object_mut())
        .map(|d| {
            let is_ours = d
                .get("model")
                .and_then(|v| v.as_str())
                .map(|m| m.starts_with(&ours_prefix))
                .unwrap_or(false);
            if is_ours {
                d.remove("model");
            }
            is_ours
        })
        .unwrap_or(false);
    let written = write_root(oclaw_home, backup_dir, &root, "pre-unhost")?;
    Ok(
        json!({ "restored": true, "changed": { "config": written }, "defaultModelRemoved": pointer_removed }),
    )
}

#[cfg(test)]
mod tests {
    use super::*;

    /// 直连协议分流(增强批):Anthropic→anthropic-messages(真机实证:自动拼 /v1/messages+x-api-key);
    /// 其余→openai-completions(原行为)。gateway+Anthropic 早拒。
    fn setup(tag: &str) -> (std::path::PathBuf, std::path::PathBuf, std::path::PathBuf) {
        let root = std::env::temp_dir().join(format!("oclaw-enh-{tag}"));
        let _ = std::fs::remove_dir_all(&root);
        let home = root.join("home");
        let backup = root.join("backup");
        std::fs::create_dir_all(&backup).unwrap();
        (home, backup, root)
    }

    fn fixture(dir: &std::path::Path, wire: &str, base_url: &str) -> std::path::PathBuf {
        let path = dir.join("providers.json");
        std::fs::write(
            &path,
            json!({
                "providers": [{
                    "id": "p1", "name": "t", "agent": "openclaw",
                    "base_url": base_url, "api_key": "sk-real",
                    "model": "m1", "wire_api": wire, "sort_index": 0, "created_at": 1
                }],
                "active_provider_id": null
            })
            .to_string(),
        )
        .unwrap();
        path
    }

    fn read_entry(home: &Path) -> Value {
        let v: Value =
            serde_json::from_str(&std::fs::read_to_string(config_path(home)).unwrap()).unwrap();
        v["models"]["providers"][PROVIDER_ID].clone()
    }

    #[test]
    fn direct_anthropic_writes_messages_api() {
        let (home, backup, root) = setup("direct-anth");
        let prov = fixture(&root, "anthropic", "https://opencode.ai/zen/go/v1/");
        let r = host(&home, &backup, &prov, "p1", "direct").unwrap();
        assert_eq!(r["api"], json!("anthropic-messages"));
        let e = read_entry(&home);
        assert_eq!(e["api"], json!("anthropic-messages"));
        assert_eq!(
            e["baseUrl"],
            json!("https://opencode.ai/zen/go"),
            "带 /v1 尾须去尾(OpenClaw 自动拼 /v1/messages)"
        );
        assert_eq!(
            e["apiKey"],
            json!("sk-real"),
            "direct 真实 Key 落盘(既定语义)"
        );
        assert_eq!(e["models"][0]["id"], json!("m1"));
        assert!(r["keyNote"].as_str().unwrap().contains("Anthropic"));
        let _ = std::fs::remove_dir_all(&root);
    }

    #[test]
    fn direct_anthropic_bare_base_kept() {
        let (home, backup, root) = setup("direct-anth-bare");
        let prov = fixture(&root, "messages", "https://2xa.cc.cd");
        let _ = host(&home, &backup, &prov, "p1", "direct").unwrap();
        let e = read_entry(&home);
        assert_eq!(
            e["baseUrl"],
            json!("https://2xa.cc.cd"),
            "裸域原样(自动拼出 /v1/messages)"
        );
        assert_eq!(e["api"], json!("anthropic-messages"));
        let _ = std::fs::remove_dir_all(&root);
    }

    #[test]
    fn direct_chat_unchanged() {
        let (home, backup, root) = setup("direct-chat");
        let prov = fixture(&root, "chat_completions", "https://go.example/v1");
        let r = host(&home, &backup, &prov, "p1", "direct").unwrap();
        assert_eq!(
            r["api"],
            json!("openai-completions"),
            "非 Anthropic 供应商行为不变"
        );
        let e = read_entry(&home);
        assert_eq!(e["api"], json!("openai-completions"));
        assert_eq!(
            e["baseUrl"],
            json!("https://go.example/v1"),
            "openai-completions 不去 /v1 尾(原行为)"
        );
        let _ = std::fs::remove_dir_all(&root);
    }

    #[test]
    fn gateway_rejects_anthropic_provider() {
        let (home, backup, root) = setup("gw-reject");
        let prov = fixture(&root, "anthropic", "https://2xa.cc.cd");
        let err = host(&home, &backup, &prov, "p1", "gateway").unwrap_err();
        assert_eq!(err.0, 400);
        assert_eq!(err.1, "E_ANTHROPIC_DIRECT_ONLY");
        assert!(!config_path(&home).exists(), "拒绝时零写入");
        let _ = std::fs::remove_dir_all(&root);
    }

    #[test]
    fn unhost_after_anthropic_direct_restores() {
        let (home, backup, root) = setup("unhost-anth");
        let prov = fixture(&root, "anthropic", "https://x.example");
        host(&home, &backup, &prov, "p1", "direct").unwrap();
        let s = state(&home);
        assert!(s["hosting"].is_object());
        assert_eq!(
            s["hosting"]["api"],
            json!("anthropic-messages"),
            "state 暴露 api 形态"
        );
        let u = unhost(&home, &backup).unwrap();
        assert_eq!(u["restored"], json!(true));
        assert_eq!(u["defaultModelRemoved"], json!(true));
        let s2 = state(&home);
        assert!(s2["hosting"].is_null());
        let _ = std::fs::remove_dir_all(&root);
    }
}

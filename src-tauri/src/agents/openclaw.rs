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
        .map(|c| if c.is_ascii_alphanumeric() || c == '.' || c == '_' || c == '-' { c } else { '-' })
        .collect();
    let s = s.trim_matches('-').to_string();
    if s.is_empty() { "model".into() } else { s }
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

fn write_root(oclaw_home: &Path, backup_dir: &Path, root: &Map<String, Value>, purpose: &str) -> Result<bool, OpError> {
    let path = config_path(oclaw_home);
    let new_text = serde_json::to_string_pretty(&Value::Object(root.clone())).map_err(|e| (500, "E_IO".into(), e.to_string()))?;
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

fn find_provider(providers_path: &Path, provider_id: &str) -> Result<crate::providers::Provider, OpError> {
    crate::providers::load(providers_path)
        .providers
        .into_iter()
        .find(|p| p.id == provider_id)
        .ok_or_else(|| (404, "E_NO_PROVIDER".into(), format!("供应商不存在: {provider_id}")))
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
        return Err((400, "E_BAD_WAY".into(), "未知托管方式,仅支持 gateway / direct".into()));
    }
    let provider = find_provider(providers_path, provider_id)?;
    if provider.model.trim().is_empty() {
        return Err((422, "E_NO_MODEL".into(), "该供应商未配置默认模型,请先在编辑里拉取模型或手填".into()));
    }
    if way == "direct" && provider.base_url.trim().is_empty() {
        return Err((422, "E_NO_BASE_URL".into(), "该供应商未配置 API 地址".into()));
    }

    let mut root = read_root(oclaw_home)?;
    let (base_url, api_key, key_note) = if way == "gateway" {
        (format!("{GATEWAY_BASE}/openclaw/v1"), PLACEHOLDER_KEY.to_string(), "占位(真实 Key 只在网关)")
    } else {
        (provider.base_url.trim().trim_end_matches('/').to_string(), provider.api_key.clone(), "直连:真实 Key 落盘于 openclaw.json")
    };

    let model_ids: Vec<(String, String, Option<u64>)> = if provider.models.is_empty() {
        let cw = provider.context_window.as_deref().and_then(|s| s.parse().ok());
        vec![(slug(&provider.model), provider.model.clone(), cw)]
    } else {
        provider.models.iter().map(|m| (slug(&m.name), m.name.clone(), m.context_window)).collect()
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
        "api": "openai-completions",
        "models": models,
    });
    root.entry("models".to_string())
        .or_insert_with(|| json!({}))
        .as_object_mut()
        .ok_or_else(|| (422, "E_CONFIG_JSON5".into(), "models 段存在但不是对象,拒绝写入".into()))?
        .entry("providers".to_string())
        .or_insert_with(|| json!({}))
        .as_object_mut()
        .ok_or_else(|| (422, "E_CONFIG_JSON5".into(), "models.providers 段存在但不是对象,拒绝写入".into()))?
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
            .insert("model".to_string(), json!(format!("{PROVIDER_ID}/{}", model_ids[0].0)));
        switched = true;
    }

    let written = write_root(oclaw_home, backup_dir, &root, if switched { "pre-host" } else { "pre-switch" })?;
    Ok(json!({
        "hosted": true, "way": way, "switched": !existing.is_empty(),
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
        .and_then(|d| {
            let is_ours = d.get("model").and_then(|v| v.as_str()).map(|m| m.starts_with(&ours_prefix)).unwrap_or(false);
            if is_ours {
                d.remove("model");
            }
            Some(is_ours)
        })
        .unwrap_or(false);
    let written = write_root(oclaw_home, backup_dir, &root, "pre-unhost")?;
    Ok(json!({ "restored": true, "changed": { "config": written }, "defaultModelRemoved": pointer_removed }))
}

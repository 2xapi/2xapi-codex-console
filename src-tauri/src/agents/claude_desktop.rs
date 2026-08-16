//! Claude Desktop adapter(阶段 D;调研报告 §七定案:第三方推理为官方原生功能)。
//!
//! 写入手法(cc-switch claude_desktop_config.rs 实证 + 本机 v1.30096.1 调研):
//! ① `…/Claude/claude_desktop_config.json` 顶层 deploymentMode="3p"(保留 mcpServers 等其余字段)
//! ② `…/Claude-3p/claude_desktop_config.json` 同
//! ③ `…/Claude-3p/configLibrary/<PROFILE_ID>.json` profile(bearer/gateway/base URL/Key)
//! ④ `…/Claude-3p/configLibrary/_meta.json` 登记 entries[].id + appliedId
//! 簿记:③④ 旁写私有 `2xapi-state.json` 记 host 前两处 deploymentMode 原值,unhost 按它
//! 恢复(原值本就是 3p 的用户——调研实证本机现状——保持不动)。
//!
//! 协议:Anthropic messages,走网关专属入口 `/{claude-desktop 的 anthropic 路径}`,per-agent
//! 取供应商,与 Claude Code 的 /anthropic/* 不串台。改配置后需重启 Claude Desktop 生效
//! (host 响应带 note)。Key 语义同先例:gateway=占位(真 Key 只在网关),direct=落盘 profile。

use serde_json::{json, Map, Value};
use std::path::{Path, PathBuf};

pub type OpError = (u16, String, String);

const GATEWAY_BASE: &str = "http://127.0.0.1:8787";
const PLACEHOLDER_KEY: &str = "2xapi-gateway-managed";
/// 本产品固定 profile id(合法 hex UUID;与 cc-switch 的不同,避免互踩)。
pub const PROFILE_ID: &str = "2a0f1e5d-0000-4000-8000-0000000c0d3a";
const STATE_FILE: &str = "2xapi-state.json";

/// Claude 主目录(`…/Claude`)与 3p 目录(`…/Claude-3p`)的公共父(Application Support 根;
/// 测试传 tempdir)。注:Windows 的 Claude Desktop 路径(APPDATA)未实证,首版 macOS 为主。
pub fn main_dir(cd_home: &Path) -> PathBuf {
    cd_home.join("Claude")
}
pub fn p3_dir(cd_home: &Path) -> PathBuf {
    cd_home.join("Claude-3p")
}
fn config_json(dir: &Path) -> PathBuf {
    dir.join("claude_desktop_config.json")
}
fn profile_path(cd_home: &Path) -> PathBuf {
    p3_dir(cd_home)
        .join("configLibrary")
        .join(format!("{PROFILE_ID}.json"))
}
fn meta_path(cd_home: &Path) -> PathBuf {
    p3_dir(cd_home).join("configLibrary").join("_meta.json")
}
fn state_path(cd_home: &Path) -> PathBuf {
    p3_dir(cd_home).join("configLibrary").join(STATE_FILE)
}

fn read_json_obj(path: &Path) -> Map<String, Value> {
    std::fs::read_to_string(path)
        .ok()
        .and_then(|raw| serde_json::from_str::<Value>(&raw).ok())
        .and_then(|v| v.as_object().cloned())
        .unwrap_or_default()
}

fn write_json(path: &Path, obj: &Map<String, Value>) -> Result<(), OpError> {
    if let Some(p) = path.parent() {
        std::fs::create_dir_all(p).map_err(|e| (500, "E_IO".into(), e.to_string()))?;
    }
    let text = serde_json::to_string_pretty(&Value::Object(obj.clone()))
        .map_err(|e| (500, "E_IO".into(), e.to_string()))?;
    std::fs::write(path, text).map_err(|e| (500, "E_IO".into(), e.to_string()))
}

fn set_deployment_mode(path: &Path, mode: &str) -> Result<(), OpError> {
    let mut obj = read_json_obj(path);
    obj.insert("deploymentMode".into(), json!(mode));
    write_json(path, &obj)
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

/// 托管态:profile 文件存在且 _meta.appliedId 指向我们。
pub fn state(cd_home: &Path) -> Value {
    let applied = read_json_obj(&meta_path(cd_home))
        .get("appliedId")
        .and_then(|v| v.as_str())
        .unwrap_or("")
        .to_string();
    let hosted = profile_path(cd_home).exists() && applied == PROFILE_ID;
    json!({
        "hosting": if hosted { json!({ "providerId": PROFILE_ID, "mode": "3p" }) } else { Value::Null },
        "deploymentMode": read_json_obj(&config_json(&main_dir(cd_home)))
            .get("deploymentMode")
            .cloned()
            .unwrap_or(Value::Null),
    })
}

pub fn host(
    cd_home: &Path,
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

    let main_cfg = config_json(&main_dir(cd_home));
    let p3_cfg = config_json(&p3_dir(cd_home));
    let prev_main = read_json_obj(&main_cfg)
        .get("deploymentMode")
        .and_then(|v| v.as_str())
        .map(String::from);
    let prev_p3 = read_json_obj(&p3_cfg)
        .get("deploymentMode")
        .and_then(|v| v.as_str())
        .map(String::from);

    let (base_url, api_key, key_note) = if way == "gateway" {
        (
            format!("{GATEWAY_BASE}/claude-desktop"),
            PLACEHOLDER_KEY.to_string(),
            "占位(真实 Key 只在网关)",
        )
    } else {
        (
            provider.base_url.trim().trim_end_matches('/').to_string(),
            provider.api_key.clone(),
            "直连:真实 Key 写入 profile",
        )
    };
    let profile = json!({
        "coworkEgressAllowedHosts": ["*"],
        "disableDeploymentModeChooser": true,
        "inferenceGatewayApiKey": api_key,
        "inferenceGatewayAuthScheme": "bearer",
        "inferenceGatewayBaseUrl": base_url,
        "inferenceProvider": "gateway",
    });

    set_deployment_mode(&main_cfg, "3p")?;
    set_deployment_mode(&p3_cfg, "3p")?;

    let mut prof_obj = profile.as_object().cloned().unwrap_or_default();
    prof_obj.insert(
        "inferenceModels".into(),
        json!([
            { "id": provider.model.trim(), "name": provider.model.trim() }
        ]),
    );
    write_json(&profile_path(cd_home), &prof_obj)?;

    let mut meta = read_json_obj(&meta_path(cd_home));
    let entries = meta
        .entry("entries".to_string())
        .or_insert_with(|| json!([]));
    if let Some(arr) = entries.as_array_mut() {
        if !arr
            .iter()
            .any(|e| e.get("id").and_then(|v| v.as_str()) == Some(PROFILE_ID))
        {
            arr.push(json!({ "id": PROFILE_ID }));
        }
    }
    meta.insert("appliedId".into(), json!(PROFILE_ID));
    write_json(&meta_path(cd_home), &meta)?;

    // 私有簿记:host 前两处 deploymentMode 原值(unhost 按此恢复;原即 3p 则保持)
    write_json(&state_path(cd_home), &{
        let mut m = Map::new();
        if let Some(p) = &prev_main {
            m.insert("prevMain".into(), json!(p));
        }
        if let Some(p) = &prev_p3 {
            m.insert("prevP3".into(), json!(p));
        }
        m
    })?;

    Ok(json!({
        "hosted": true, "way": way,
        "profileId": PROFILE_ID,
        "keyNote": key_note,
        "note": "配置已写入;重启 Claude Desktop 后生效",
        "changed": { "mainConfig": true, "p3Config": true, "profile": true, "meta": true },
    }))
}

pub fn unhost(cd_home: &Path) -> Result<Value, OpError> {
    if !profile_path(cd_home).exists() {
        return Ok(json!({ "restored": false, "alreadyClean": true }));
    }
    // 簿记恢复 deploymentMode(无簿记/原值缺失 → "1p",官方模式)
    let bk = read_json_obj(&state_path(cd_home));
    let restore = |cfg: &Path, key: &str| {
        let mode = bk
            .get(key)
            .and_then(|v| v.as_str())
            .unwrap_or("1p")
            .to_string();
        let _ = set_deployment_mode(cfg, &mode);
    };
    restore(&config_json(&main_dir(cd_home)), "prevMain");
    restore(&config_json(&p3_dir(cd_home)), "prevP3");

    std::fs::remove_file(profile_path(cd_home)).map_err(|e| (500, "E_IO".into(), e.to_string()))?;
    let mut meta = read_json_obj(&meta_path(cd_home));
    if let Some(arr) = meta.get_mut("entries").and_then(|v| v.as_array_mut()) {
        arr.retain(|e| e.get("id").and_then(|v| v.as_str()) != Some(PROFILE_ID));
    }
    if meta.get("appliedId").and_then(|v| v.as_str()) == Some(PROFILE_ID) {
        meta.remove("appliedId");
    }
    write_json(&meta_path(cd_home), &meta)?;
    let _ = std::fs::remove_file(state_path(cd_home));

    Ok(json!({ "restored": true, "note": "已还原;重启 Claude Desktop 后生效" }))
}

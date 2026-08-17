//! Config 写入引擎（M2，契约 01 行为矩阵 + D1~D4 + 02 §5）。
//!
//! 三模式 config.toml / auth.json 生成（02 §5）：
//! - **Official**：`model_provider="openai"`，删除 `[model_providers.custom]` 与 `model_catalog_json`；auth.json 不动。
//! - **Mixed/PureApi**：`model_provider="custom"`，`custom.base_url=http://127.0.0.1:8787`（指向网关，非上游！），
//!   `wire_api="responses"`（Codex 恒发 Responses，01-D5），`requires_openai_auth=true`，**不设** `experimental_bearer_token`（01-D2）。
//! - **PureApi**：auth.json 设 `OPENAI_API_KEY=provider.api_key`；首次切换前备份 `auth.json.official.bak`（01-D4）。
//!
//! 字段级合并写（FR-3.6）：只改受控字段，保留用户其他配置。
//!
//! 注（CCR-001，待文档同步）：02 §5.5 写的 model_catalog 格式为简化占位 `{name,provider,context_window}`；
//! 真实 Codex 的 model_catalog_json 需要富格式（slug/base_instructions/...），本引擎沿用已验证可用的富格式。

use serde_json::{json, Value};
use sha2::{Digest, Sha256};
#[allow(unused_imports)] // PathBuf 仅测试用
use std::path::{Path, PathBuf};

use crate::providers::{AccessMode, ModelConfig, Provider, ProviderData};

/// 网关地址：config 里 `custom.base_url` 指向它（02 §5.2）。
pub const GATEWAY_BASE_URL: &str = "http://127.0.0.1:8787";
pub(crate) const MODEL_CATALOG_FILENAME: &str = "2xapi-model-catalog.json";
pub(crate) const AUTH_OFFICIAL_BAK: &str = "auth.json.official.bak";

// ── TOML 读写（JSON Value ↔ TOML，原子写）────────────────────

/// 读 TOML → JSON Value；失败返回空对象。
pub fn read_toml(path: &Path) -> Value {
    match std::fs::read_to_string(path) {
        Ok(content) => match toml::from_str::<toml::Value>(&content) {
            Ok(v) => toml_to_json(&v),
            Err(_) => json!({}),
        },
        Err(_) => json!({}),
    }
}

/// JSON Value → 原子写 TOML（临时文件→rename）。
pub fn write_toml(path: &Path, cfg: &Value) -> Result<(), String> {
    let toml_val = json_to_toml(cfg);
    let s = toml::to_string_pretty(&toml_val).map_err(|e| format!("TOML 编码失败: {e}"))?;
    let tmp = path.with_extension("toml.tmp");
    std::fs::write(&tmp, s).map_err(|e| format!("写入临时文件失败: {e}"))?;
    std::fs::rename(&tmp, path).map_err(|e| format!("重命名失败: {e}"))?;
    Ok(())
}

pub(crate) fn config_to_toml_string(cfg: &Value) -> Result<String, String> {
    let t = json_to_toml(cfg);
    toml::to_string_pretty(&t).map_err(|e| format!("TOML 编码失败: {e}"))
}

fn toml_to_json(v: &toml::Value) -> Value {
    match v {
        toml::Value::String(s) => json!(s),
        toml::Value::Integer(i) => json!(i),
        toml::Value::Float(f) => json!(f),
        toml::Value::Boolean(b) => json!(b),
        toml::Value::Array(arr) => json!(arr.iter().map(toml_to_json).collect::<Vec<_>>()),
        toml::Value::Table(t) => {
            let mut m = serde_json::Map::new();
            for (k, v) in t {
                m.insert(k.clone(), toml_to_json(v));
            }
            Value::Object(m)
        }
        _ => Value::Null,
    }
}

fn json_to_toml(v: &Value) -> toml::Value {
    match v {
        Value::String(s) => toml::Value::String(s.clone()),
        Value::Number(n) => {
            if let Some(i) = n.as_i64() {
                toml::Value::Integer(i)
            } else {
                toml::Value::Float(n.as_f64().unwrap_or(0.0))
            }
        }
        Value::Bool(b) => toml::Value::Boolean(*b),
        Value::Array(arr) => toml::Value::Array(arr.iter().map(json_to_toml).collect()),
        Value::Object(m) => {
            let mut t = toml::map::Map::new();
            for (k, v) in m {
                t.insert(k.clone(), json_to_toml(v));
            }
            toml::Value::Table(t)
        }
        Value::Null => toml::Value::String(String::new()),
    }
}

// ── auth.json 读写 ───────────────────────────────────────────

pub(crate) fn read_auth_json(path: &Path) -> Value {
    match std::fs::read_to_string(path) {
        Ok(raw) => serde_json::from_str(&raw).unwrap_or(json!({})),
        Err(_) => json!({}),
    }
}

pub(crate) fn write_auth_json(path: &Path, v: &Value) -> Result<(), String> {
    let raw = serde_json::to_string_pretty(v).map_err(|e| e.to_string())?;
    let tmp = path.with_extension("json.tmp");
    std::fs::write(&tmp, &raw).map_err(|e| e.to_string())?;
    std::fs::rename(&tmp, path).map_err(|e| e.to_string())?;
    Ok(())
}

// ── 受控字段合并（FR-3.6：保留用户未知字段）──────────────────

/// 在 `current` 之上合并出目标配置（只动受控字段/段，其余保留）。
pub fn build_config_value(current: &Value, provider: &Provider, catalog_path: &str) -> Value {
    let mut cfg = current.clone();
    let obj = cfg.as_object_mut().expect("config 不是 object");

    obj.remove("codex_plus_chat_base_url"); // 清理遗留字段

    match provider.access_mode {
        AccessMode::Official => {
            obj.insert("model_provider".into(), json!("openai"));
            if !provider.model.is_empty() {
                obj.insert("model".into(), json!(provider.model));
            }
            obj.remove("model_catalog_json");
            // 删除 [model_providers.custom]，保留其他 provider 条目
            if let Some(mp) = obj
                .get_mut("model_providers")
                .and_then(|v| v.as_object_mut())
            {
                mp.remove("custom");
            }
        }
        AccessMode::Mixed => {
            obj.insert("model_provider".into(), json!("custom"));
            if !provider.model.is_empty() {
                obj.insert("model".into(), json!(provider.model));
            }
            obj.insert("model_catalog_json".into(), json!(catalog_path));
            let mut custom = serde_json::Map::new();
            custom.insert("name".into(), json!("custom"));
            custom.insert("base_url".into(), json!(GATEWAY_BASE_URL));
            custom.insert("wire_api".into(), json!("responses"));
            custom.insert("requires_openai_auth".into(), json!(true));
            // v2 文档：Mixed 注入 experimental_bearer_token（Codex 自带 key 发给网关，无需依赖 OAuth）
            custom.insert("experimental_bearer_token".into(), json!(provider.api_key));
            let mp = obj.entry("model_providers").or_insert(json!({}));
            if let Some(m) = mp.as_object_mut() {
                m.insert("custom".into(), Value::Object(custom));
            }
        }
        AccessMode::PureApi => {
            obj.insert("model_provider".into(), json!("custom"));
            if !provider.model.is_empty() {
                obj.insert("model".into(), json!(provider.model));
            }
            obj.insert("model_catalog_json".into(), json!(catalog_path));
            let mut custom = serde_json::Map::new();
            custom.insert("name".into(), json!("custom"));
            custom.insert("base_url".into(), json!(GATEWAY_BASE_URL));
            custom.insert("wire_api".into(), json!("responses"));
            custom.insert("requires_openai_auth".into(), json!(true));
            // PureApi：不设 experimental_bearer_token，key 写 auth.json
            let mp = obj.entry("model_providers").or_insert(json!({}));
            if let Some(m) = mp.as_object_mut() {
                m.insert("custom".into(), Value::Object(custom));
            }
        }
    }
    cfg
}

// ── 结果结构 ─────────────────────────────────────────────────

#[derive(Debug, Clone, serde::Serialize)]
pub struct ApplyOutcome {
    pub config_written: bool,
    pub auth_changed: bool,
    pub backup_created: bool,
    pub config_toml_snapshot: String,
    pub auth_json_snapshot: String,
}

#[derive(Debug, Clone, serde::Serialize)]
pub struct PreviewOutcome {
    pub config_toml: String,
    pub auth_action: String, // "noop" | "set_key"
    pub auth_diff: Option<Vec<String>>,
    pub backup_will_create: bool,
}

#[derive(Debug, Clone, serde::Serialize)]
pub struct ActivateResult {
    pub active_provider_id: String,
    pub config_written: bool,
    pub auth_changed: bool,
    pub backup_created: bool,
}

#[derive(Debug, Clone, serde::Serialize)]
pub struct OfficialOutcome {
    pub config_written: bool,
    pub auth_restored: bool,
}

// ── apply（核心：合并写 config + auth + 备份）─────────────────

/// 应用一个 provider（按其 access_mode 写 config/auth）。幂等（FR-2.5）。
/// `backup_dir` 用于 apply 前对 config.toml 的安全快照（/api/backups 用）；auth 备份固定落 `codex_home`。
pub fn apply_provider(
    config_path: &Path,
    backup_dir: &Path,
    codex_home: &Path,
    provider: &Provider,
) -> Result<ApplyOutcome, String> {
    let catalog_path = codex_home.join(MODEL_CATALOG_FILENAME);
    let current_cfg = read_toml(config_path);
    let current_toml = config_to_toml_string(&current_cfg).unwrap_or_default();

    let merged = build_config_value(&current_cfg, provider, &catalog_path.to_string_lossy());
    let new_toml = config_to_toml_string(&merged)?;

    // config 幂等写
    let config_written = if new_toml != current_toml {
        let _ = backup_file(config_path, backup_dir, "config-apply", "pre-apply");
        write_toml(config_path, &merged)?;
        true
    } else {
        false
    };

    // model catalog 文件（Mixed/PureApi 且有模型才写；Official 删除）
    if provider.access_mode != AccessMode::Official && !provider.models.is_empty() {
        let catalog = build_model_catalog(
            &provider.models,
            provider.reasoning_levels.as_deref().unwrap_or(&[]),
        );
        let raw = serde_json::to_string_pretty(&catalog).unwrap_or_default();
        std::fs::write(&catalog_path, format!("{raw}\n")).map_err(|e| e.to_string())?;
    } else if provider.access_mode == AccessMode::Official {
        let _ = std::fs::remove_file(&catalog_path);
    }

    // auth.json（仅 PureApi 动）
    let (auth_changed, backup_created) = match provider.access_mode {
        AccessMode::PureApi => {
            let auth_p = codex_home.join("auth.json");
            let bak_p = codex_home.join(AUTH_OFFICIAL_BAK);
            // 01-D4：首次切换前备份（.bak 不存在才备份，保护最早的官方态）
            let mut created = false;
            if !bak_p.exists() {
                if let Ok(data) = std::fs::read(&auth_p) {
                    std::fs::write(&bak_p, &data).map_err(|e| format!("备份 auth 失败: {e}"))?;
                    created = true;
                }
            }
            // 设 OPENAI_API_KEY（幂等）
            let mut existing = read_auth_json(&auth_p);
            let key = provider.api_key.clone();
            let changed = existing
                .get("OPENAI_API_KEY")
                .and_then(|v| v.as_str())
                .map(|s| s.to_string())
                != Some(key.clone());
            if changed {
                if let Some(o) = existing.as_object_mut() {
                    o.insert("OPENAI_API_KEY".into(), json!(key));
                }
                write_auth_json(&auth_p, &existing)?;
            }
            (changed, created)
        }
        _ => (false, false),
    };

    let auth_snap = match provider.access_mode {
        AccessMode::PureApi => "set OPENAI_API_KEY".to_string(),
        _ => "noop".to_string(),
    };

    Ok(ApplyOutcome {
        config_written,
        auth_changed,
        backup_created,
        config_toml_snapshot: new_toml,
        auth_json_snapshot: auth_snap,
    })
}

// ── preview（FR-3.3/8.2：与 apply 结果一致）──────────────────

pub fn preview_provider(
    config_path: &Path,
    codex_home: &Path,
    provider: &Provider,
) -> Result<PreviewOutcome, String> {
    let catalog_path = codex_home.join(MODEL_CATALOG_FILENAME);
    let current_cfg = read_toml(config_path);
    let merged = build_config_value(&current_cfg, provider, &catalog_path.to_string_lossy());
    let new_toml = config_to_toml_string(&merged)?;

    let (auth_action, auth_diff) = match provider.access_mode {
        AccessMode::PureApi => (
            "set_key".to_string(),
            Some(vec!["OPENAI_API_KEY".to_string()]),
        ),
        _ => ("noop".to_string(), None),
    };
    let backup_will_create =
        provider.access_mode == AccessMode::PureApi && !codex_home.join(AUTH_OFFICIAL_BAK).exists();

    Ok(PreviewOutcome {
        config_toml: new_toml,
        auth_action,
        auth_diff,
        backup_will_create,
    })
}

// ── activate / activate-official（FR-2.1~2.5）───────────────

/// 激活某 provider：apply + 设置 active + 写 snapshot。返回三布尔。
pub fn activate(
    config_path: &Path,
    backup_dir: &Path,
    providers_path: &Path,
    codex_home: &Path,
    provider_id: &str,
) -> Result<ActivateResult, String> {
    let mut data: ProviderData = crate::providers::load(providers_path);
    let provider = data
        .providers
        .iter()
        .find(|p| p.id == provider_id)
        .ok_or("供应商不存在")?
        .clone();

    let outcome = apply_provider(config_path, backup_dir, codex_home, &provider)?;

    data.active_provider_id = Some(provider_id.to_string());
    if let Some(p) = data.providers.iter_mut().find(|p| p.id == provider_id) {
        p.config_toml_snapshot = Some(outcome.config_toml_snapshot.clone());
        p.auth_json_snapshot = Some(outcome.auth_json_snapshot.clone());
    }
    crate::providers::store(providers_path, &data)?;

    Ok(ActivateResult {
        active_provider_id: provider_id.to_string(),
        config_written: outcome.config_written,
        auth_changed: outcome.auth_changed,
        backup_created: outcome.backup_created,
    })
}

/// 切回官方：恢复 auth.json（从 .bak）+ config 改 official + 清 active。
pub fn activate_official(
    config_path: &Path,
    backup_dir: &Path,
    providers_path: &Path,
    codex_home: &Path,
) -> Result<OfficialOutcome, String> {
    // 01-D4：恢复 auth.json
    let bak_p = codex_home.join(AUTH_OFFICIAL_BAK);
    let auth_p = codex_home.join("auth.json");
    let auth_restored = if bak_p.exists() {
        let data = std::fs::read(&bak_p).map_err(|e| format!("读 .bak 失败: {e}"))?;
        std::fs::write(&auth_p, &data).map_err(|e| format!("恢复 auth 失败: {e}"))?;
        true
    } else {
        false
    };

    // config → official（用空 official provider；不动 model 字段）
    let official = Provider {
        access_mode: AccessMode::Official,
        ..Provider::default()
    };
    let current_cfg = read_toml(config_path);
    let current_toml = config_to_toml_string(&current_cfg).unwrap_or_default();
    let merged = build_config_value(&current_cfg, &official, "");
    let new_toml = config_to_toml_string(&merged)?;
    let config_written = if new_toml != current_toml {
        let _ = backup_file(
            config_path,
            backup_dir,
            "config-apply",
            "pre-activate-official",
        );
        write_toml(config_path, &merged)?;
        true
    } else {
        false
    };
    let _ = std::fs::remove_file(codex_home.join(MODEL_CATALOG_FILENAME));

    // 清 active
    let mut data = crate::providers::load(providers_path);
    data.active_provider_id = None;
    crate::providers::store(providers_path, &data)?;

    Ok(OfficialOutcome {
        config_written,
        auth_restored,
    })
}

// ── model catalog（富格式；CCR-001 待 02 §5.5 同步）─────────

pub(crate) fn build_model_catalog(models: &[ModelConfig], reasoning_levels: &[String]) -> Value {
    // 若上游探测到 levels 用它；否则用默认 5 级
    let levels: Vec<String> = if reasoning_levels.is_empty() {
        ["low", "medium", "high", "xhigh", "max"]
            .iter()
            .map(|s| s.to_string())
            .collect()
    } else {
        reasoning_levels.to_vec()
    };
    let default_level = levels.first().cloned().unwrap_or_else(|| "medium".into());
    let rl_json: Vec<Value> = levels
        .iter()
        .map(|e| {
            let desc = match e.as_str() {
                "low" => "Fast responses with lighter reasoning",
                "medium" => "Balances speed and reasoning depth for everyday tasks",
                "high" => "Greater reasoning depth for complex problems",
                "xhigh" => "Extra high reasoning depth for complex problems",
                "max" => "Maximum reasoning depth for the hardest problems",
                "ultra" => "Maximum reasoning with automatic task delegation",
                _ => "Custom",
            };
            json!({"effort": e, "description": desc})
        })
        .collect();
    let arr: Vec<Value> = models
        .iter()
        .enumerate()
        .map(|(i, m)| {
            let cw = m.context_window.unwrap_or(200000);
            let disp = m.display_name.clone().unwrap_or_else(|| m.name.clone());
            json!({
                "additional_speed_tiers": [],
                "availability_nux": Value::Null,
                "base_instructions": "You are Codex, a coding agent. You and the user share the same workspace and collaborate to achieve the user's goals.",
                "context_window": cw,
                "default_reasoning_level": default_level,
                "default_reasoning_summary": "none",
                "description": disp,
                "display_name": disp,
                "effective_context_window_percent": 95,
                "experimental_supported_tools": [],
                "input_modalities": ["text"],
                "max_context_window": cw,
                "priority": 1000 + i,
                "service_tiers": [],
                "shell_type": "shell_command",
                "slug": m.name,
                "support_verbosity": false,
                "supported_in_api": true,
                "supported_reasoning_levels": rl_json,
                "supports_image_detail_original": false,
                "supports_parallel_tool_calls": true,
                "supports_reasoning_summaries": true,
                "supports_search_tool": false,
                "truncation_policy": { "limit": 10000, "mode": "bytes" },
                "upgrade": Value::Null,
                "visibility": "list"
            })
        })
        .collect();
    json!({ "models": arr })
}

// ── 备份 / 快照 / 恢复（/api/backups、/api/config/* 用）──────

/// 备份一个文件到 backup_dir（带 sha256 manifest）。
pub fn backup_file(
    src: &Path,
    backup_dir: &Path,
    prefix: &str,
    purpose: &str,
) -> Result<(), String> {
    let timestamp = chrono::Local::now().format("%Y%m%d-%H%M%S").to_string();
    let backup_name = format!("{prefix}-{timestamp}.toml");
    let backup_path = backup_dir.join(&backup_name);

    match std::fs::read(src) {
        Ok(data) => {
            let mut hasher = Sha256::new();
            hasher.update(&data);
            let hash = hex::encode(hasher.finalize());
            std::fs::write(&backup_path, &data).map_err(|e| e.to_string())?;
            let manifest = json!({
                "version": 1,
                "kind": "config",
                "purpose": purpose,
                "createdAt": chrono::Local::now().to_rfc3339(),
                "configPath": src.to_string_lossy(),
                "originalExists": true,
                "sha256": hash,
            });
            let manifest_path = format!("{}.manifest.json", backup_path.display());
            std::fs::write(
                &manifest_path,
                serde_json::to_string_pretty(&manifest).unwrap_or_default(),
            )
            .ok();
        }
        Err(_) => {
            std::fs::write(&backup_path, "# original config.toml did not exist\n").ok();
        }
    }
    Ok(())
}

/// 从备份恢复 config。
pub fn restore(config_path: &Path, backup_path: &str) -> Result<(), String> {
    let data = std::fs::read(backup_path).map_err(|e| format!("读取备份失败: {e}"))?;
    std::fs::write(config_path, data).map_err(|e| format!("写入失败: {e}"))?;
    Ok(())
}

/// 手动快照当前 config。
pub fn create_snapshot(config_path: &Path, backup_dir: &Path) -> Result<Value, String> {
    backup_file(config_path, backup_dir, "config-snapshot", "manual")?;
    let entries = crate::backups::list(backup_dir);
    Ok(entries
        .first()
        .cloned()
        .unwrap_or(json!({ "created": true })))
}

// ── 单测（M2 Gate）────────────────────────────────────────────

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::atomic::{AtomicU64, Ordering};

    static COUNTER: AtomicU64 = AtomicU64::new(0);

    /// 临时 sandbox：返回 (root, config_path, backup_dir, codex_home)，自动清理旧的同名。
    fn sandbox(label: &str) -> (PathBuf, PathBuf, PathBuf, PathBuf) {
        let n = COUNTER.fetch_add(1, Ordering::SeqCst);
        let root =
            std::env::temp_dir().join(format!("2xapi-m2-{label}-{}-{n}", std::process::id()));
        let _ = std::fs::remove_dir_all(&root);
        let codex_home = root.join("codex");
        let backup_dir = root.join("backups");
        let config_path = codex_home.join("config.toml");
        std::fs::create_dir_all(&codex_home).unwrap();
        std::fs::create_dir_all(&backup_dir).unwrap();
        (root, config_path, backup_dir, codex_home)
    }

    fn pureapi_provider() -> Provider {
        Provider {
            id: "p-pure".into(),
            name: "Pure".into(),
            base_url: "https://up.example.com".into(),
            api_key: "sk-pure-secret".into(),
            keys: vec![],
            access_mode: AccessMode::PureApi,
            model: "gpt-pure".into(),
            ..Default::default()
        }
    }

    fn provider_with(mode: AccessMode) -> Provider {
        Provider {
            id: format!("p-{mode:?}").to_lowercase(),
            name: format!("{mode:?}"),
            base_url: "https://up.example.com".into(),
            api_key: "sk-secret".into(),
            keys: vec![],
            access_mode: mode,
            model: "gpt-x".into(),
            ..Default::default()
        }
    }

    /// Official/Mixed/PureApi：config 的 custom.base_url 必须是网关而非上游（核心架构修正）。
    #[test]
    fn mixed_points_base_url_to_gateway() {
        let (_root, cfg, bk, home) = sandbox("mixed-gw");
        let p = provider_with(AccessMode::Mixed);
        apply_provider(&cfg, &bk, &home, &p).unwrap();
        let written = std::fs::read_to_string(&cfg).unwrap();
        assert!(
            written.contains("base_url = \"http://127.0.0.1:8787\""),
            "base_url 必须指向网关:\n{written}"
        );
        assert!(
            !written.contains("https://up.example.com"),
            "config 里不应出现上游地址:\n{written}"
        );
        let _ = std::fs::remove_dir_all(&_root);
    }

    /// v2 文档：Mixed 含 experimental_bearer_token；PureApi 不含。
    #[test]
    fn experimental_bearer_token_only_in_mixed() {
        let (_root, cfg, bk, home) = sandbox("ebt");
        // Mixed 应含
        let p = provider_with(AccessMode::Mixed);
        apply_provider(&cfg, &bk, &home, &p).unwrap();
        let written = std::fs::read_to_string(&cfg).unwrap();
        assert!(
            written.contains("experimental_bearer_token"),
            "Mixed 应含 experimental_bearer_token:\n{written}"
        );
        // PureApi 不应含
        std::fs::write(&cfg, "").ok();
        let p2 = provider_with(AccessMode::PureApi);
        apply_provider(&cfg, &bk, &home, &p2).unwrap();
        let written2 = std::fs::read_to_string(&cfg).unwrap();
        assert!(
            !written2.contains("experimental_bearer_token"),
            "PureApi 不应含 experimental_bearer_token:\n{written2}"
        );
        let _ = std::fs::remove_dir_all(&_root);
    }

    /// Official：删除 custom 段 + model_provider=openai + 无 model_catalog_json。
    #[test]
    fn official_removes_custom_section() {
        let (_root, cfg, bk, home) = sandbox("official");
        // 先写一份带 custom 的 config
        std::fs::write(&cfg, "model_provider = \"custom\"\n[model_providers.custom]\nbase_url = \"http://127.0.0.1:8787\"\n").unwrap();
        let p = provider_with(AccessMode::Official);
        apply_provider(&cfg, &bk, &home, &p).unwrap();
        let written = std::fs::read_to_string(&cfg).unwrap();
        assert!(written.contains("model_provider = \"openai\""));
        assert!(!written.contains("[model_providers.custom]"));
        assert!(!written.contains("model_catalog_json"));
        let _ = std::fs::remove_dir_all(&_root);
    }

    /// FR-3.6：用户自写未知字段 apply 后保留。
    #[test]
    fn user_fields_preserved() {
        let (_root, cfg, bk, home) = sandbox("preserve");
        std::fs::write(&cfg, "my_custom_setting = \"keep_me\"\nfoo = 123\n").unwrap();
        let p = provider_with(AccessMode::Mixed);
        apply_provider(&cfg, &bk, &home, &p).unwrap();
        let written = std::fs::read_to_string(&cfg).unwrap();
        assert!(
            written.contains("my_custom_setting = \"keep_me\""),
            "用户字段丢失:\n{written}"
        );
        assert!(written.contains("foo = 123"));
        let _ = std::fs::remove_dir_all(&_root);
    }

    /// FR-3.3/8.2：preview 的 config_toml == apply 写入的 config_toml。
    #[test]
    fn preview_equals_apply() {
        let (_root, cfg, bk, home) = sandbox("preview");
        std::fs::write(&cfg, "user_x = 1\n").unwrap();
        let p = provider_with(AccessMode::PureApi);
        let pv = preview_provider(&cfg, &home, &p).unwrap();
        apply_provider(&cfg, &bk, &home, &p).unwrap();
        let written = std::fs::read_to_string(&cfg).unwrap();
        assert_eq!(pv.config_toml, written, "preview 与 apply 不一致");
        assert_eq!(pv.auth_action, "set_key");
        assert_eq!(pv.auth_diff, Some(vec!["OPENAI_API_KEY".to_string()]));
        let _ = std::fs::remove_dir_all(&_root);
    }

    /// 01-D4：PureApi 首次切换备份 .official.bak；auth 设 key；再次 apply 不重复备份。
    #[test]
    fn pureapi_backup_and_idempotent_auth() {
        let (_root, cfg, bk, home) = sandbox("pureapi-bak");
        // 预置一个「官方」auth.json
        std::fs::write(
            home.join("auth.json"),
            "{\"tokens\":{\"id_token\":\"official\"}}",
        )
        .unwrap();

        let p = pureapi_provider();
        let o1 = apply_provider(&cfg, &bk, &home, &p).unwrap();
        assert!(o1.backup_created, "首次切换应创建 .bak");
        assert!(o1.auth_changed, "首次应改 auth");
        assert!(home.join(AUTH_OFFICIAL_BAK).exists(), ".bak 应存在");
        // auth.json 被设了 key，但仍保留原字段
        let auth_after = std::fs::read_to_string(home.join("auth.json")).unwrap();
        assert!(auth_after.contains("OPENAI_API_KEY"));
        assert!(auth_after.contains("sk-pure-secret"));
        assert!(auth_after.contains("official"), "原 OAuth 字段应保留");

        // 再次 apply 同一 provider：幂等
        let o2 = apply_provider(&cfg, &bk, &home, &p).unwrap();
        assert!(!o2.backup_created, "不应重复备份");
        assert!(!o2.auth_changed, "key 未变不应改 auth");
        let _ = std::fs::remove_dir_all(&_root);
    }

    /// FR-2.4：activate-official 用 .bak 恢复 auth.json。
    #[test]
    fn activate_official_restores_auth() {
        let (_root, cfg, bk, home) = sandbox("restore");
        // 官方态 auth
        let official_auth = "{\"tokens\":{\"id_token\":\"OFFICIAL\"}}";
        std::fs::write(home.join("auth.json"), official_auth).unwrap();
        // providers.json
        let providers_path = home.join("providers.json");
        std::fs::write(
            &providers_path,
            "{\"schema_version\":1,\"active_provider_id\":null,\"providers\":[]}",
        )
        .unwrap();

        // 先切 PureApi（产生 .bak，auth 被改）
        let p = pureapi_provider();
        apply_provider(&cfg, &bk, &home, &p).unwrap();
        assert_ne!(
            std::fs::read_to_string(home.join("auth.json")).unwrap(),
            official_auth
        );

        // activate-official 恢复
        let out = activate_official(&cfg, &bk, &providers_path, &home).unwrap();
        assert!(out.auth_restored, "应从 .bak 恢复");
        assert_eq!(
            std::fs::read_to_string(home.join("auth.json")).unwrap(),
            official_auth,
            "恢复后应等于官方态"
        );
        let _ = std::fs::remove_dir_all(&_root);
    }

    /// FR-2.1/2.5：activate 返回三布尔，且首次全 true、二次幂等。
    #[test]
    fn activate_three_booleans_and_idempotent() {
        let (_root, cfg, bk, home) = sandbox("activate");
        std::fs::write(home.join("auth.json"), "{\"tokens\":{\"id_token\":\"O\"}}").unwrap();
        let providers_path = home.join("providers.json");
        let p = pureapi_provider();
        std::fs::write(
            &providers_path,
            serde_json::to_string(&ProviderData {
                schema_version: 1,
                active_provider_id: None,
                providers: vec![p.clone()],
            })
            .unwrap(),
        )
        .unwrap();

        let r1 = activate(&cfg, &bk, &providers_path, &home, &p.id).unwrap();
        assert_eq!(r1.active_provider_id, p.id);
        assert!(r1.config_written);
        assert!(r1.auth_changed);
        assert!(r1.backup_created);

        // 二次：内容相同 → 幂等
        let r2 = activate(&cfg, &bk, &providers_path, &home, &p.id).unwrap();
        assert!(!r2.config_written, "config 未变不应重写");
        assert!(!r2.auth_changed, "key 未变不应改 auth");
        assert!(!r2.backup_created, ".bak 已存在不应重复");

        // active 已写入 providers.json
        let data = crate::providers::load(&providers_path);
        assert_eq!(data.active_provider_id, Some(p.id));
        let _ = std::fs::remove_dir_all(&_root);
    }
}

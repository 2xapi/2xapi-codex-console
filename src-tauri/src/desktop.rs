//! 桌面版托管开关(阶段 1,开发任务书 §1.1)。
//!
//! 桌面版 ChatGPT.app 无法注入 env/参数,唯一配置入口是 `~/.codex/config.toml`。
//! 「托管」= 字段级合并写一处 `[model_providers.custom]` 指向本机网关 8787:
//! - 有官方登录 → `requires_openai_auth=true`(混入:官方 token 发网关,网关丢弃并注入中转 Key)
//! - 无官方登录 → `requires_openai_auth=false` + auth.json 写 OPENAI_API_KEY(纯 API 形态,先备份)
//! 配置文件零 Key(Key 由网关注入);「还原」= unhost。
//!
//! 与 config.rs(M2)的关系:复用其 toml 读写/备份/catalog 原语,但合并逻辑独立——
//! M2 的 Mixed 会写 experimental_bearer_token 且 requires_openai_auth 恒 true,
//! 与任务书 §1.1(b) 契约(零 bearer、按账号态取值)不同,M2 行为保持不动。

use serde_json::{json, Value};
use std::path::Path;

use crate::config::{
    backup_file, build_model_catalog, config_to_toml_string, read_auth_json, read_toml, write_auth_json,
    write_toml, AUTH_OFFICIAL_BAK, GATEWAY_BASE_URL, MODEL_CATALOG_FILENAME,
};
use crate::providers::Provider;

pub const GATEWAY_ADDR: &str = "127.0.0.1:8787";

/// host/unhost 的错误:(HTTP 状态码, 错误码, 人话信息)。handler 层转 {"error": code, "message": msg}。
pub type OpError = (u16, String, String);

// ── hasOfficial 判定 ─────────────────────────────────────────

/// auth.json 是否含官方登录态(ChatGPT 账号 token)。
/// ⚠️探索点(任务书 §1.1(a)):官方登录的实际字段名以真机为准,本机当前无官方登录、无法观察,
/// 故写成通用检查——存在键名含 "token" 且值非空的字段(官方 OAuth 形态为 `tokens` 对象)即算;
/// 仅 OPENAI_API_KEY 或文件不存在/解析失败 → false。
pub fn has_official(codex_home: &Path) -> bool {
    let raw = match std::fs::read_to_string(codex_home.join("auth.json")) {
        Ok(r) => r,
        Err(_) => return false,
    };
    let obj = match serde_json::from_str::<Value>(&raw) {
        Ok(Value::Object(o)) => o,
        _ => return false,
    };
    for (k, v) in &obj {
        if k == "OPENAI_API_KEY" {
            continue;
        }
        if !k.to_lowercase().contains("token") {
            continue;
        }
        let nonempty = match v {
            Value::String(s) => !s.is_empty(),
            Value::Object(o) => !o.is_empty(),
            _ => false,
        };
        if nonempty {
            return true;
        }
    }
    false
}

// ── hosting 判定 ─────────────────────────────────────────────

/// 当前 config.toml 是否处于本软件托管态。
/// - custom.base_url 指向网关 → gateway(providerId 用 providers.json active 交叉印证)
/// - custom.base_url == 当前 active 供应商地址 → direct(本期 host 不产生此形态,判定保留完整性)
/// - 无 custom 段 / 第三方手写 custom(如 opencode) → null
pub fn detect_hosting(config_path: &Path, providers_path: &Path) -> Value {
    let cfg = read_toml(config_path);
    let base_url = cfg
        .get("model_providers")
        .and_then(|m| m.get("custom"))
        .and_then(|c| c.get("base_url"))
        .and_then(|v| v.as_str());
    let Some(base_url) = base_url else {
        return Value::Null;
    };
    if base_url.contains(GATEWAY_ADDR) {
        let data = crate::providers::load(providers_path);
        let (id, name) = match &data.active_provider_id {
            Some(id) => {
                let name = data
                    .providers
                    .iter()
                    .find(|p| &p.id == id)
                    .map(|p| json!(p.name))
                    .unwrap_or(Value::Null);
                (json!(id), name)
            }
            None => (Value::Null, Value::Null),
        };
        return json!({ "providerId": id, "providerName": name, "way": "gateway" });
    }
    if let Some(p) = crate::providers::get_active(providers_path) {
        if p.base_url == base_url {
            return json!({ "providerId": p.id, "providerName": p.name, "way": "direct" });
        }
    }
    Value::Null
}

pub fn gateway_alive() -> bool {
    let addr: std::net::SocketAddr = GATEWAY_ADDR.parse().unwrap();
    std::net::TcpStream::connect_timeout(&addr, std::time::Duration::from_millis(300)).is_ok()
}

/// GET /api/desktop/state
pub fn state(config_path: &Path, providers_path: &Path, codex_home: &Path) -> Value {
    json!({
        "hasOfficial": has_official(codex_home),
        "hosting": detect_hosting(config_path, providers_path),
        "gateway": { "addr": GATEWAY_ADDR, "alive": gateway_alive() },
        "codexHome": codex_home.to_string_lossy(),
    })
}

// ── host ─────────────────────────────────────────────────────

/// 合并出 gateway 托管态 config(零 Key:不写 experimental_bearer_token,任务书 §1.1(b) 契约)。
fn build_hosted_config(current: &Value, provider: &Provider, catalog_path: &str, requires_openai_auth: bool) -> Value {
    let mut cfg = current.clone();
    let obj = cfg.as_object_mut().expect("config 不是 object");
    obj.insert("model_provider".into(), json!("custom"));
    if !provider.model.is_empty() {
        obj.insert("model".into(), json!(provider.model));
    }
    // catalog 仅在供应商有模型时指向(文件也仅在此时写),避免指向不存在文件
    if !provider.models.is_empty() {
        obj.insert("model_catalog_json".into(), json!(catalog_path));
    }
    let mut custom = serde_json::Map::new();
    custom.insert("name".into(), json!("custom"));
    custom.insert("base_url".into(), json!(GATEWAY_BASE_URL));
    custom.insert("wire_api".into(), json!("responses"));
    custom.insert("requires_openai_auth".into(), json!(requires_openai_auth));
    let mp = obj.entry("model_providers").or_insert(json!({}));
    if let Some(m) = mp.as_object_mut() {
        m.insert("custom".into(), Value::Object(custom));
    }
    cfg
}

/// 无官方账号时:auth.json 设 OPENAI_API_KEY;首次(.bak 不存在)先备份 host 前状态(01-D4 同语义)。
/// 返回 (changed, backup_created)。
fn ensure_auth_key(codex_home: &Path, api_key: &str) -> Result<(bool, bool), String> {
    let auth_p = codex_home.join("auth.json");
    let bak_p = codex_home.join(AUTH_OFFICIAL_BAK);
    let mut created = false;
    if !bak_p.exists() {
        if let Ok(data) = std::fs::read(&auth_p) {
            std::fs::write(&bak_p, &data).map_err(|e| format!("备份 auth 失败: {e}"))?;
            created = true;
        }
    }
    let mut existing = read_auth_json(&auth_p);
    if existing.get("OPENAI_API_KEY").and_then(|v| v.as_str()) == Some(api_key) {
        return Ok((false, created));
    }
    if existing.is_object() {
        if let Some(o) = existing.as_object_mut() {
            o.insert("OPENAI_API_KEY".into(), json!(api_key));
        }
        write_auth_json(&auth_p, &existing)?;
    }
    Ok((true, created))
}

/// POST /api/desktop/host {providerId, way}
pub fn host(
    config_path: &Path,
    backup_dir: &Path,
    codex_home: &Path,
    providers_path: &Path,
    provider_id: &str,
    way: &str,
) -> Result<Value, OpError> {
    // direct 的 provider 段 Bearer 字段未实测(任务书 §1.4 探索),通过前一律拒绝
    if way != "gateway" {
        return Err((400, "E_DIRECT_UNAVAILABLE".into(), "直连方式即将支持,当前请使用网关方式".into()));
    }
    let data = crate::providers::load(providers_path);
    let provider = data
        .providers
        .iter()
        .find(|p| p.id == provider_id)
        .cloned()
        .ok_or_else(|| (404, "E_PROVIDER_NOT_FOUND".to_string(), "找不到该供应商".to_string()))?;

    let io = |e: String| -> OpError { (500, "E_IO".to_string(), e) };

    // 已处于 gateway 托管(含换供应商):仅 set_active,不重写 config(任务书 §1.1(b):custom 段不变)
    let already = detect_hosting(config_path, providers_path);
    if already.get("way").and_then(|v| v.as_str()) == Some("gateway") {
        crate::providers::set_active(providers_path, &provider.id);
        let mut auth_changed = false;
        if !has_official(codex_home) {
            auth_changed = ensure_auth_key(codex_home, &provider.api_key).map_err(io)?.0;
        }
        return Ok(json!({
            "hosted": true, "switched": true,
            "hasOfficial": has_official(codex_home),
            "hosting": detect_hosting(config_path, providers_path),
            "changed": { "config": false, "auth": auth_changed },
        }));
    }

    // 全量托管写(字段级合并 + 备份)
    let has_off = has_official(codex_home);
    let current = read_toml(config_path);
    let catalog_path = codex_home.join(MODEL_CATALOG_FILENAME);
    let merged = build_hosted_config(&current, &provider, &catalog_path.to_string_lossy(), has_off);
    let new_toml = config_to_toml_string(&merged).map_err(io)?;
    let current_toml = config_to_toml_string(&current).unwrap_or_default();
    let config_written = if new_toml != current_toml {
        backup_file(config_path, backup_dir, "config-apply", "pre-host").map_err(io)?;
        write_toml(config_path, &merged).map_err(io)?;
        true
    } else {
        false
    };

    if !provider.models.is_empty() {
        let catalog = build_model_catalog(&provider.models, provider.reasoning_levels.as_deref().unwrap_or(&[]));
        let raw = serde_json::to_string_pretty(&catalog).map_err(|e| e.to_string()).map_err(io)?;
        std::fs::write(&catalog_path, format!("{raw}\n")).map_err(|e| e.to_string()).map_err(io)?;
    }

    // 无官方账号:auth.json 写供应商 key(先备份)
    let (auth_changed, backup_created) = if has_off {
        (false, false)
    } else {
        ensure_auth_key(codex_home, &provider.api_key).map_err(io)?
    };

    crate::providers::set_active(providers_path, &provider.id);

    Ok(json!({
        "hosted": true, "switched": false,
        "hasOfficial": has_off,
        "hosting": detect_hosting(config_path, providers_path),
        "changed": { "config": config_written, "auth": auth_changed, "authBackup": backup_created },
    }))
}

// ── unhost ───────────────────────────────────────────────────

/// 移除 custom 段及关联字段(model_provider/model/model_catalog_json),其余保留。
fn build_unhosted_config(current: &Value) -> Value {
    let mut cfg = current.clone();
    let obj = cfg.as_object_mut().expect("config 不是 object");
    obj.remove("model_provider");
    obj.remove("model");
    obj.remove("model_catalog_json");
    if let Some(mp) = obj.get_mut("model_providers").and_then(|v| v.as_object_mut()) {
        mp.remove("custom");
        if mp.is_empty() {
            obj.remove("model_providers");
        }
    }
    cfg
}

/// auth.json 里移除我们写入的 OPENAI_API_KEY(仅当值匹配最后一次托管供应商的 key;无 .bak 时的兜底)。
fn remove_auth_key_if_ours(codex_home: &Path, api_key: &str) -> bool {
    let auth_p = codex_home.join("auth.json");
    let mut existing = read_auth_json(&auth_p);
    let matches = existing.get("OPENAI_API_KEY").and_then(|v| v.as_str()) == Some(api_key);
    if matches {
        if let Some(o) = existing.as_object_mut() {
            o.remove("OPENAI_API_KEY");
            if write_auth_json(&auth_p, &existing).is_ok() {
                return true;
            }
        }
    }
    false
}

/// POST /api/desktop/unhost
pub fn unhost(
    config_path: &Path,
    backup_dir: &Path,
    codex_home: &Path,
    providers_path: &Path,
) -> Result<Value, OpError> {
    let io = |e: String| -> OpError { (500, "E_IO".to_string(), e) };

    let hosting = detect_hosting(config_path, providers_path);
    if hosting.is_null() {
        // 未托管(或第三方手写 custom 段,不属于我们)→ 幂等 no-op
        return Ok(json!({ "restored": false, "alreadyClean": true }));
    }

    if has_official(codex_home) {
        // 有官方账号:还原官方登录态(复用 M2 activate-official:恢复 .bak + config→official + 清 active)
        crate::config::activate_official(config_path, backup_dir, providers_path, codex_home)
            .map_err(|e| (500, "E_IO".to_string(), e))?;
        return Ok(json!({ "restored": true, "way": "official" }));
    }

    // 无官方账号:移除托管痕迹
    let active = crate::providers::get_active(providers_path);
    let current = read_toml(config_path);
    let merged = build_unhosted_config(&current);
    let new_toml = config_to_toml_string(&merged).map_err(io)?;
    let current_toml = config_to_toml_string(&current).unwrap_or_default();
    let config_written = if new_toml != current_toml {
        backup_file(config_path, backup_dir, "config-apply", "pre-unhost").map_err(io)?;
        write_toml(config_path, &merged).map_err(io)?;
        true
    } else {
        false
    };
    let _ = std::fs::remove_file(codex_home.join(MODEL_CATALOG_FILENAME));

    // auth 恢复:有 .bak → 恢复 host 前状态;无 .bak → 仅移除我们写的 key(host 前本无 auth.json)
    let auth_restored = if codex_home.join(AUTH_OFFICIAL_BAK).exists() {
        let data = std::fs::read(codex_home.join(AUTH_OFFICIAL_BAK)).map_err(|e| io(e.to_string()))?;
        std::fs::write(codex_home.join("auth.json"), &data).map_err(|e| io(e.to_string()))?;
        true
    } else if let Some(p) = &active {
        remove_auth_key_if_ours(codex_home, &p.api_key)
    } else {
        false
    };

    crate::providers::clear_active(providers_path);

    Ok(json!({
        "restored": true, "way": "clean",
        "changed": { "config": config_written, "auth": auth_restored },
    }))
}

// ── 单测(任务书 §1.3)────────────────────────────────────────

#[cfg(test)]
mod tests {
    use super::*;
    use crate::providers::{AccessMode, ProviderData};
    use std::sync::atomic::{AtomicU64, Ordering};

    static COUNTER: AtomicU64 = AtomicU64::new(0);

    fn sandbox(label: &str) -> (std::path::PathBuf, std::path::PathBuf, std::path::PathBuf, std::path::PathBuf, std::path::PathBuf) {
        let n = COUNTER.fetch_add(1, Ordering::SeqCst);
        let root = std::env::temp_dir().join(format!("2xapi-stage1-{label}-{}-{n}", std::process::id()));
        let _ = std::fs::remove_dir_all(&root);
        let codex_home = root.join("codex");
        let backup_dir = root.join("backups");
        let config_path = codex_home.join("config.toml");
        let providers_path = root.join("providers.json");
        std::fs::create_dir_all(&codex_home).unwrap();
        std::fs::create_dir_all(&backup_dir).unwrap();
        (root, config_path, backup_dir, codex_home, providers_path)
    }

    fn provider(id: &str, name: &str) -> Provider {
        Provider {
            id: id.into(),
            name: name.into(),
            base_url: "https://up.example.com".into(),
            api_key: "sk-test-secret".into(),
            access_mode: AccessMode::PureApi,
            model: "gpt-demo".into(),
            ..Default::default()
        }
    }

    fn write_providers(path: &Path, providers: Vec<Provider>) {
        std::fs::write(
            path,
            serde_json::to_string(&ProviderData { schema_version: 1, active_provider_id: None, providers }).unwrap(),
        )
        .unwrap();
    }

    // ── hasOfficial 三态 ──

    #[test]
    fn has_official_three_states() {
        let (_r, _c, _b, home, _p) = sandbox("auth3");
        // 不存在 → false
        assert!(!has_official(&home));
        // 仅 OPENAI_API_KEY → false
        std::fs::write(home.join("auth.json"), r#"{"OPENAI_API_KEY":"sk-x"}"#).unwrap();
        assert!(!has_official(&home));
        // 官方 OAuth(tokens 对象)→ true
        std::fs::write(home.join("auth.json"), r#"{"tokens":{"id_token":"a","access_token":"b"}}"#).unwrap();
        assert!(has_official(&home));
        // 两者并存(混入后常见)→ true
        std::fs::write(
            home.join("auth.json"),
            r#"{"OPENAI_API_KEY":"sk-x","tokens":{"access_token":"b"}}"#,
        )
        .unwrap();
        assert!(has_official(&home));
        // 坏 JSON → false
        std::fs::write(home.join("auth.json"), "not json").unwrap();
        assert!(!has_official(&home));
    }

    // ── host(gateway)写入快照 ──

    #[test]
    fn host_gateway_writes_expected_config() {
        let (root, cfg, bk, home, prov) = sandbox("host-gw");
        std::fs::write(&cfg, "my_custom_setting = \"keep_me\"\n").unwrap();
        // host 前已有 auth.json(别家 key)→ 应被备份
        std::fs::write(home.join("auth.json"), r#"{"OPENAI_API_KEY":"sk-old"}"#).unwrap();
        let mut p = provider("p1", "2xapi");
        p.models = vec![crate::providers::ModelConfig {
            name: "gpt-demo".into(),
            display_name: None,
            context_window: Some(400000),
            is_multimodal: false,
            send_as_is: false,
        }];
        write_providers(&prov, vec![p]);

        host(&cfg, &bk, &home, &prov, "p1", "gateway").unwrap();
        let written = std::fs::read_to_string(&cfg).unwrap();
        assert!(written.contains("model_provider = \"custom\""));
        assert!(written.contains("base_url = \"http://127.0.0.1:8787\""), "custom 段应指向网关:\n{written}");
        assert!(written.contains("wire_api = \"responses\""));
        // 无官方账号 → requires_openai_auth=false
        assert!(written.contains("requires_openai_auth = false"), "无账号应为 false:\n{written}");
        // 零 Key 契约:不写 bearer token,上游地址与 key 都不进 config
        assert!(!written.contains("experimental_bearer_token"), "不应写 bearer:\n{written}");
        assert!(!written.contains("up.example.com"));
        assert!(!written.contains("sk-test-secret"));
        // 用户字段保留 + catalog 指向
        assert!(written.contains("my_custom_setting"));
        assert!(written.contains("model_catalog_json"));
        // catalog 文件与 active
        assert!(home.join(MODEL_CATALOG_FILENAME).exists());
        assert_eq!(crate::providers::load(&prov).active_provider_id, Some("p1".into()));
        // auth:无账号 → key 写入 + host 前状态备份
        let auth = std::fs::read_to_string(home.join("auth.json")).unwrap();
        assert!(auth.contains("sk-test-secret"));
        assert!(home.join(AUTH_OFFICIAL_BAK).exists(), "应备份 host 前的 auth.json");
        assert_eq!(
            std::fs::read_to_string(home.join(AUTH_OFFICIAL_BAK)).unwrap(),
            r#"{"OPENAI_API_KEY":"sk-old"}"#
        );
        let _ = std::fs::remove_dir_all(&root);
    }

    #[test]
    fn host_gateway_with_official_keeps_auth_untouched() {
        let (root, cfg, bk, home, prov) = sandbox("host-official");
        let official_auth = r#"{"tokens":{"id_token":"official-state"}}"#;
        std::fs::write(home.join("auth.json"), official_auth).unwrap();
        write_providers(&prov, vec![provider("p1", "2xapi")]);

        let out = host(&cfg, &bk, &home, &prov, "p1", "gateway").unwrap();
        assert!(out["hasOfficial"].as_bool().unwrap());
        let written = std::fs::read_to_string(&cfg).unwrap();
        assert!(written.contains("requires_openai_auth = true"), "有账号应混入:\n{written}");
        // auth.json 原样、无备份
        assert_eq!(std::fs::read_to_string(home.join("auth.json")).unwrap(), official_auth);
        assert!(!home.join(AUTH_OFFICIAL_BAK).exists());
        let _ = std::fs::remove_dir_all(&root);
    }

    #[test]
    fn host_rejects_direct_and_unknown_provider() {
        let (root, cfg, bk, home, prov) = sandbox("host-err");
        write_providers(&prov, vec![provider("p1", "2xapi")]);
        let err = host(&cfg, &bk, &home, &prov, "p1", "direct").unwrap_err();
        assert_eq!(err.1, "E_DIRECT_UNAVAILABLE");
        let err2 = host(&cfg, &bk, &home, &prov, "nope", "gateway").unwrap_err();
        assert_eq!(err2.1, "E_PROVIDER_NOT_FOUND");
        let _ = std::fs::remove_dir_all(&root);
    }

    #[test]
    fn host_idempotent_same_provider() {
        let (root, cfg, bk, home, prov) = sandbox("host-idem");
        write_providers(&prov, vec![provider("p1", "2xapi")]);
        let r1 = host(&cfg, &bk, &home, &prov, "p1", "gateway").unwrap();
        assert!(r1["changed"]["config"].as_bool().unwrap());
        let before = std::fs::read_to_string(&cfg).unwrap();
        let r2 = host(&cfg, &bk, &home, &prov, "p1", "gateway").unwrap();
        assert!(r2["switched"].as_bool().unwrap(), "重复 host 同供应商走切换分支");
        assert!(!r2["changed"]["config"].as_bool().unwrap());
        let after = std::fs::read_to_string(&cfg).unwrap();
        assert_eq!(before, after, "config 不应变化");
        let _ = std::fs::remove_dir_all(&root);
    }

    #[test]
    fn host_switch_provider_keeps_custom_section() {
        let (root, cfg, bk, home, prov) = sandbox("host-switch");
        write_providers(&prov, vec![provider("p1", "A"), provider("p2", "B")]);
        host(&cfg, &bk, &home, &prov, "p1", "gateway").unwrap();
        let before = std::fs::read_to_string(&cfg).unwrap();
        let r = host(&cfg, &bk, &home, &prov, "p2", "gateway").unwrap();
        assert!(r["switched"].as_bool().unwrap());
        assert_eq!(std::fs::read_to_string(&cfg).unwrap(), before, "换供应商仅 set_active,config 不变(任务书契约)");
        assert_eq!(crate::providers::load(&prov).active_provider_id, Some("p2".into()));
        let _ = std::fs::remove_dir_all(&root);
    }

    // ── hosting 判定变体 ──

    #[test]
    fn detect_hosting_variants() {
        let (root, cfg, _bk, home, prov) = sandbox("detect");
        // 无 custom → null
        std::fs::write(&cfg, "model_provider = \"openai\"\n").unwrap();
        assert!(detect_hosting(&cfg, &prov).is_null());
        // 第三方 custom(opencode 形态)→ null
        std::fs::write(&cfg, "[model_providers.custom]\nbase_url = \"https://opencode.ai/zen/go/v1\"\n").unwrap();
        assert!(detect_hosting(&cfg, &prov).is_null(), "第三方 custom 不应误判为托管");
        // gateway 托管 + active
        write_providers(&prov, vec![provider("p1", "A")]);
        std::fs::write(&cfg, "model_provider = \"custom\"\n[model_providers.custom]\nbase_url = \"http://127.0.0.1:8787\"\n").unwrap();
        crate::providers::set_active(&prov, "p1");
        let h = detect_hosting(&cfg, &prov);
        assert_eq!(h["way"], "gateway");
        assert_eq!(h["providerId"], "p1");
        assert_eq!(h["providerName"], "A");
        // gateway 但无 active(状态破坏)→ way=gateway, providerId=null
        crate::providers::clear_active(&prov);
        let h2 = detect_hosting(&cfg, &prov);
        assert_eq!(h2["way"], "gateway");
        assert!(h2["providerId"].is_null());
        let _ = std::fs::remove_dir_all(&root);
        let _ = home;
    }

    // ── unhost ──

    #[test]
    fn unhost_no_official_cleans_everything() {
        let (root, cfg, bk, home, prov) = sandbox("unhost-clean");
        // host 前用户已有别家 key(本机真实场景:opencode)
        std::fs::write(home.join("auth.json"), r#"{"OPENAI_API_KEY":"sk-other-vendor"}"#).unwrap();
        write_providers(&prov, vec![provider("p1", "A")]);
        host(&cfg, &bk, &home, &prov, "p1", "gateway").unwrap();

        let out = unhost(&cfg, &bk, &home, &prov).unwrap();
        assert!(out["restored"].as_bool().unwrap());
        assert_eq!(out["way"], "clean");
        let written = std::fs::read_to_string(&cfg).unwrap();
        assert!(!written.contains("[model_providers.custom]"));
        assert!(!written.contains("model_provider ="));
        assert!(!written.contains("model_catalog_json"));
        assert!(!written.contains("model ="));
        assert!(!home.join(MODEL_CATALOG_FILENAME).exists());
        // auth 回到 host 前的别家 key
        assert_eq!(
            std::fs::read_to_string(home.join("auth.json")).unwrap(),
            r#"{"OPENAI_API_KEY":"sk-other-vendor"}"#
        );
        assert!(crate::providers::load(&prov).active_provider_id.is_none());
        // 幂等
        let out2 = unhost(&cfg, &bk, &home, &prov).unwrap();
        assert!(out2["alreadyClean"].as_bool().unwrap());
        let _ = std::fs::remove_dir_all(&root);
    }

    #[test]
    fn unhost_with_official_restores_official() {
        let (root, cfg, bk, home, prov) = sandbox("unhost-official");
        std::fs::write(home.join("auth.json"), r#"{"tokens":{"id_token":"OFFICIAL"}}"#).unwrap();
        write_providers(&prov, vec![provider("p1", "A")]);
        host(&cfg, &bk, &home, &prov, "p1", "gateway").unwrap();

        let out = unhost(&cfg, &bk, &home, &prov).unwrap();
        assert_eq!(out["way"], "official");
        let written = std::fs::read_to_string(&cfg).unwrap();
        assert!(written.contains("model_provider = \"openai\""));
        assert!(!written.contains("[model_providers.custom]"));
        assert!(std::fs::read_to_string(home.join("auth.json")).unwrap().contains("OFFICIAL"));
        let _ = std::fs::remove_dir_all(&root);
    }

    #[test]
    fn unhost_without_bak_removes_only_our_key() {
        let (root, cfg, bk, home, prov) = sandbox("unhost-nobak");
        // host 前无 auth.json → 无 .bak;host 后 auth 只有我们写的 key
        write_providers(&prov, vec![provider("p1", "A")]);
        host(&cfg, &bk, &home, &prov, "p1", "gateway").unwrap();
        assert!(!home.join(AUTH_OFFICIAL_BAK).exists());

        unhost(&cfg, &bk, &home, &prov).unwrap();
        let auth = read_auth_json(&home.join("auth.json"));
        assert!(auth.get("OPENAI_API_KEY").is_none(), "我们写的 key 应被移除:\n{auth}");
        let _ = std::fs::remove_dir_all(&root);
    }

    #[test]
    fn unhost_ignores_third_party_custom() {
        let (root, cfg, bk, home, prov) = sandbox("unhost-third");
        std::fs::write(&cfg, "[model_providers.custom]\nbase_url = \"https://opencode.ai/zen/go/v1\"\n").unwrap();
        let out = unhost(&cfg, &bk, &home, &prov).unwrap();
        assert!(out["alreadyClean"].as_bool().unwrap());
        // 第三方段原样保留
        assert!(std::fs::read_to_string(&cfg).unwrap().contains("opencode.ai"));
        let _ = std::fs::remove_dir_all(&root);
    }
}

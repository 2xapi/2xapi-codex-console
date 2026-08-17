//! 能力注册表骨架(超融合 A 线一期 §3,方案 v1.0):
//! 「一切能力=注册表条目」,挂载点四个(媒体解析/工具执行/协议转换/调度策略)永久冻结。
//! 一期=骨架 + kind=model 条目(探测标签的注册表视角);二期媒体关卡/工具执行开始消费。

use serde_json::{json, Map, Value};
use std::path::{Path, PathBuf};

#[derive(Debug, Clone, PartialEq)]
pub enum Kind {
    Model,
    Plugin,
    Tool,
}

impl Kind {
    fn as_str(&self) -> &'static str {
        match self {
            Kind::Model => "model",
            Kind::Plugin => "plugin",
            Kind::Tool => "tool",
        }
    }
}

#[derive(Debug, Clone)]
pub struct Entry {
    pub id: String,
    pub kind: Kind,
    /// model 条目:归属供应商+模型(标签粒度对齐);plugin/tool 条目:二期契约
    pub provider_id: Option<String>,
    pub model: Option<String>,
    pub enabled: bool,
    pub meta: Map<String, Value>,
    /// v3 用户配置(配置页保存):{models:[{id,api,note}](优先级), failover:bool, values:{k:v}}
    pub config: Map<String, Value>,
    /// v3 来源:local|paste|remote|official(旧档读取按 builtin 推导)
    pub source: String,
    /// v3 最近变更(安装/配置/启停/更新),unix 秒
    pub updated_at: String,
}

fn registry_path(codex_home: &Path) -> PathBuf {
    codex_home.join("fusion-registry.json")
}

fn load(codex_home: &Path) -> Vec<Entry> {
    let raw = std::fs::read_to_string(registry_path(codex_home)).unwrap_or_default();
    let v: Value = serde_json::from_str(&raw).unwrap_or(json!({}));
    v.get("entries")
        .and_then(|e| e.as_array())
        .map(|arr| {
            arr.iter()
                .filter_map(|e| {
                    Some(Entry {
                        id: e.get("id")?.as_str()?.to_string(),
                        kind: match e.get("kind")?.as_str()? {
                            "plugin" => Kind::Plugin,
                            "tool" => Kind::Tool,
                            _ => Kind::Model,
                        },
                        provider_id: e
                            .get("provider_id")
                            .and_then(|x| x.as_str())
                            .map(String::from),
                        model: e.get("model").and_then(|x| x.as_str()).map(String::from),
                        enabled: e.get("enabled").and_then(|x| x.as_bool()).unwrap_or(true),
                        meta: e
                            .get("meta")
                            .and_then(|m| m.as_object())
                            .cloned()
                            .unwrap_or_default(),
                        config: e
                            .get("config")
                            .and_then(|c| c.as_object())
                            .cloned()
                            .unwrap_or_default(),
                        source: e
                            .get("source")
                            .and_then(|s| s.as_str())
                            .map(String::from)
                            .unwrap_or_else(|| {
                                let builtin = e["meta"]
                                    .get("builtin")
                                    .and_then(|b| b.as_bool())
                                    .unwrap_or(false);
                                if builtin {
                                    "official".into()
                                } else {
                                    "remote".into()
                                }
                            }),
                        updated_at: e
                            .get("updated_at")
                            .and_then(|u| u.as_str())
                            .map(String::from)
                            .unwrap_or_default(),
                    })
                })
                .collect()
        })
        .unwrap_or_default()
}

fn save(codex_home: &Path, entries: &[Entry]) {
    let path = registry_path(codex_home);
    if let Some(parent) = path.parent() {
        std::fs::create_dir_all(parent).ok();
    }
    let body = json!({
        "version": 1,
        "entries": entries.iter().map(|e| json!({
            "id": e.id, "kind": e.kind.as_str(),
            "provider_id": e.provider_id, "model": e.model,
            "enabled": e.enabled, "meta": e.meta,
            "config": e.config, "source": e.source, "updated_at": e.updated_at,
        })).collect::<Vec<_>>(),
    });
    let tmp = path.with_extension("json.tmp");
    if std::fs::write(
        &tmp,
        serde_json::to_string_pretty(&body).unwrap_or_default(),
    )
    .is_ok()
    {
        let _ = std::fs::rename(&tmp, &path);
    }
}

/// upsert(model 条目):探测落标签时同步登记,id=provider::model。
pub fn upsert_model(codex_home: &Path, provider_id: &str, model: &str, meta: Map<String, Value>) {
    let mut entries = load(codex_home);
    let id = format!("{provider_id}::{model}");
    if let Some(e) = entries.iter_mut().find(|e| e.id == id) {
        e.meta = meta;
    } else {
        entries.push(Entry {
            id,
            kind: Kind::Model,
            provider_id: Some(provider_id.to_string()),
            model: Some(model.to_string()),
            enabled: true,
            meta,
            config: Map::new(),
            source: String::new(),
            updated_at: now(),
        });
    }
    save(codex_home, &entries);
}

/// upsert(plugin 条目):meta=manifest 全量;同 id 覆盖(重装/更新,保留用户 config)。
/// 新条目带默认配置(config def 种子化 + manifest models + failover 默认开)与 source。
pub fn upsert_plugin(codex_home: &Path, manifest: &Map<String, Value>) {
    let id = manifest
        .get("id")
        .and_then(|v| v.as_str())
        .unwrap_or("")
        .to_string();
    if id.is_empty() {
        return;
    }
    let mut entries = load(codex_home);
    let global = manifest
        .get("source_id")
        .and_then(|v| v.as_str())
        .map(|s| format!("{s}.{id}"))
        .unwrap_or(id); // 前缀命名(OpenWrt 吸收,市场源用;直接登记=无前缀
                        // 内置能力=tool 条目(本机实现);http 型=plugin 条目
    let kind = if manifest
        .get("builtin")
        .and_then(|v| v.as_bool())
        .unwrap_or(false)
    {
        Kind::Tool
    } else {
        Kind::Plugin
    };
    let source = manifest
        .get("source")
        .and_then(|v| v.as_str())
        .filter(|s| matches!(*s, "local" | "paste" | "remote" | "official"))
        .map(String::from)
        .unwrap_or_else(|| {
            if manifest
                .get("builtin")
                .and_then(|v| v.as_bool())
                .unwrap_or(false)
            {
                "official".into()
            } else if manifest.get("source_id").and_then(|v| v.as_str()) == Some("local") {
                "local".into()
            } else {
                "remote".into()
            }
        });
    if let Some(e) = entries
        .iter_mut()
        .find(|e| e.id == global && e.kind == kind)
    {
        e.meta = manifest.clone();
        e.updated_at = now();
    } else {
        // 默认配置:manifest config 的 def 种子化 + models 声明 + failover 默认开
        let mut values = Map::new();
        if let Some(arr) = manifest.get("config").and_then(|c| c.as_array()) {
            for c in arr {
                if let (Some(k), Some(def)) = (c.get("k").and_then(|v| v.as_str()), c.get("def")) {
                    values.insert(k.to_string(), def.clone());
                }
            }
        }
        let mut config = Map::new();
        config.insert(
            "models".into(),
            manifest.get("models").cloned().unwrap_or_else(|| json!([])),
        );
        config.insert("failover".into(), json!(true));
        config.insert("values".into(), json!(values));
        entries.push(Entry {
            id: global,
            kind,
            provider_id: None,
            model: None,
            enabled: true,
            meta: manifest.clone(),
            config,
            source,
            updated_at: now(),
        });
    }
    save(codex_home, &entries);
}

pub fn get_plugin(codex_home: &Path, id: &str) -> Option<Entry> {
    load(codex_home)
        .into_iter()
        .find(|e| e.id == id && (e.kind == Kind::Plugin || e.kind == Kind::Tool))
}

pub fn remove(codex_home: &Path, id: &str) {
    let mut entries = load(codex_home);
    entries.retain(|e| e.id != id);
    save(codex_home, &entries);
}

pub fn set_enabled(codex_home: &Path, id: &str, enabled: bool) {
    let mut entries = load(codex_home);
    if let Some(e) = entries.iter_mut().find(|e| e.id == id) {
        e.enabled = enabled;
        e.updated_at = now();
    }
    save(codex_home, &entries);
}

/// v3:保存插件用户配置(models 优先级/故障转移开关/配置项值);id 不存在返回 false。
pub fn set_config(codex_home: &Path, id: &str, config: Map<String, Value>) -> bool {
    let mut entries = load(codex_home);
    let Some(e) = entries.iter_mut().find(|e| e.id == id) else {
        return false;
    };
    e.config = config;
    e.updated_at = now();
    save(codex_home, &entries);
    true
}

/// unix 秒时间戳(updated_at 用)。
pub fn now() -> String {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_secs().to_string())
        .unwrap_or_default()
}

pub fn list_json(codex_home: &Path) -> Value {
    let entries = load(codex_home);
    json!({
        "entries": entries.iter().map(|e| json!({
            "id": e.id, "kind": e.kind.as_str(),
            "provider_id": e.provider_id, "model": e.model,
            "enabled": e.enabled, "meta": e.meta,
            "config": e.config, "source": e.source, "updated_at": e.updated_at,
        })).collect::<Vec<_>>(),
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn upsert_and_list() {
        let r = std::env::temp_dir().join(format!("2xapi-reg-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&r);
        std::fs::create_dir_all(&r).unwrap();
        let mut meta = Map::new();
        meta.insert("image_in".into(), json!("yes"));
        upsert_model(&r, "p1", "m1", meta.clone());
        upsert_model(&r, "p1", "m1", meta.clone()); // 幂等
        let v = list_json(&r);
        let arr = v["entries"].as_array().unwrap();
        assert_eq!(arr.len(), 1, "同 id upsert 不重复");
        assert_eq!(arr[0]["kind"], "model");
        assert_eq!(arr[0]["meta"]["image_in"], "yes");
        upsert_model(&r, "p2", "m1", meta);
        assert_eq!(list_json(&r)["entries"].as_array().unwrap().len(), 2);
        let _ = std::fs::remove_dir_all(&r);
    }
}

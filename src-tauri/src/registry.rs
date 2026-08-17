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
                        provider_id: e.get("provider_id").and_then(|x| x.as_str()).map(String::from),
                        model: e.get("model").and_then(|x| x.as_str()).map(String::from),
                        enabled: e.get("enabled").and_then(|x| x.as_bool()).unwrap_or(true),
                        meta: e.get("meta").and_then(|m| m.as_object()).cloned().unwrap_or_default(),
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
        })).collect::<Vec<_>>(),
    });
    let tmp = path.with_extension("json.tmp");
    if std::fs::write(&tmp, serde_json::to_string_pretty(&body).unwrap_or_default()).is_ok() {
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
        });
    }
    save(codex_home, &entries);
}

pub fn list_json(codex_home: &Path) -> Value {
    let entries = load(codex_home);
    json!({
        "entries": entries.iter().map(|e| json!({
            "id": e.id, "kind": e.kind.as_str(),
            "provider_id": e.provider_id, "model": e.model,
            "enabled": e.enabled, "meta": e.meta,
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

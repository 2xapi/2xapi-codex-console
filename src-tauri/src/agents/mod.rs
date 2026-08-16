//! 多平台 agent 注册表(多平台接入方案 §2.1,A 阶段地基)
//!
//! A 阶段:元数据 + 白名单 + 泛化路由分发骨架;外部行为零变化(codex/claude 照旧)。
//! B 阶段起:每平台一个 adapter 模块挂载于此(registry 登记即可接入),
//! workbuddy 为第一个新平台 adapter(2026-08-16,叠加写双 models.json,见 workbuddy.rs)。
//!
//! 注册表即产品事实源:前端导航(D3 决策「A 后一次全亮,未实现标即将上线」)与
//! providers.rs 的 agent 白名单都从本表派生;pi 已裁撤(2026-08-16),不在表内。

pub mod gemini;
pub mod workbuddy;

use serde_json::{json, Value};

/// 单个 agent 平台的元数据。
#[derive(Debug, Clone)]
pub struct AgentMeta {
    /// 平台标识(codex / claude / gemini / grokbuild / opencode / openclaw / hermes / claude-desktop)
    pub id: &'static str,
    /// 显示名
    pub name: &'static str,
    /// 导航提示文案
    pub tip: &'static str,
    /// 是否已实现(可切换世界 / 可建供应商);false = 前端置灰标「即将上线」
    pub available: bool,
    /// 对网关的消费协议(responses|chat|anthropic|gemini);未实现平台为规划值
    pub egress: &'static str,
    /// 托管形态:"config"=写配置文件 / "inject"=注入式启动 / ""=未定
    pub hosting: &'static str,
}

/// 全平台注册表(顺序即前端导航顺序)。
static REGISTRY: &[AgentMeta] = &[
    AgentMeta {
        id: "codex",
        name: "Codex",
        tip: "Codex",
        available: true,
        egress: "responses",
        hosting: "config",
    },
    AgentMeta {
        id: "claude",
        name: "Claude Code",
        tip: "Claude Code",
        available: true,
        egress: "anthropic",
        hosting: "inject",
    },
    AgentMeta {
        id: "gemini",
        name: "Gemini CLI",
        tip: "Gemini CLI",
        available: true,
        egress: "gemini",
        hosting: "config",
    },
    AgentMeta {
        id: "grokbuild",
        name: "Grok Build",
        tip: "Grok Build(即将上线)",
        available: false,
        egress: "chat",
        hosting: "config",
    },
    AgentMeta {
        id: "opencode",
        name: "OpenCode",
        tip: "OpenCode(即将上线)",
        available: false,
        egress: "chat",
        hosting: "config",
    },
    AgentMeta {
        id: "openclaw",
        name: "OpenClaw",
        tip: "OpenClaw(即将上线)",
        available: false,
        egress: "anthropic",
        hosting: "config",
    },
    AgentMeta {
        id: "hermes",
        name: "Hermes",
        tip: "Hermes(即将上线)",
        available: false,
        egress: "chat",
        hosting: "config",
    },
    AgentMeta {
        id: "claude-desktop",
        name: "Claude 桌面版",
        tip: "Claude Desktop(即将上线)",
        available: false,
        egress: "anthropic",
        hosting: "config",
    },
    AgentMeta {
        id: "workbuddy",
        name: "WorkBuddy",
        tip: "WorkBuddy / CodeBuddy",
        available: true,
        egress: "chat",
        hosting: "config",
    },
];

pub fn registry() -> impl Iterator<Item = &'static AgentMeta> {
    REGISTRY.iter()
}

/// 按 id 查找(大小写不敏感,与 providers.rs 归一化口径一致)。
pub fn find(id: &str) -> Option<&'static AgentMeta> {
    let norm = id.trim().to_ascii_lowercase();
    REGISTRY.iter().find(|m| m.id == norm)
}

/// 已实现平台白名单(providers.rs normalize_agent 的事实源;A 阶段恒为 codex/claude)。
pub fn supported_ids() -> Vec<&'static str> {
    REGISTRY
        .iter()
        .filter(|m| m.available)
        .map(|m| m.id)
        .collect()
}

/// GET /api/desktop/agents 响应体。
pub fn registry_json() -> Value {
    json!({
        "agents": REGISTRY
            .iter()
            .map(|m| {
                json!({
                    "id": m.id,
                    "name": m.name,
                    "tip": m.tip,
                    "available": m.available,
                    "egress": m.egress,
                    "hosting": m.hosting,
                })
            })
            .collect::<Vec<_>>(),
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    /// 注册表完整性:9 平台(pi 已裁撤不在内)、id 唯一。
    #[test]
    fn registry_has_nine_unique_platforms() {
        let all: Vec<&str> = registry().map(|m| m.id).collect();
        assert_eq!(all.len(), 9, "平台数应为 9(pi 已裁撤): {all:?}");
        let mut uniq = all.clone();
        uniq.sort_unstable();
        uniq.dedup();
        assert_eq!(uniq.len(), all.len(), "id 不得重复");
        assert!(!all.contains(&"pi"), "pi 已裁撤,不得出现在注册表");
    }

    /// 可用平台 = codex/claude/gemini/workbuddy(gemini 阶段 C 接入;workbuddy 为 B 阶段第一个新平台)。
    #[test]
    fn supported_ids_includes_workbuddy() {
        assert_eq!(supported_ids(), vec!["codex", "claude", "gemini", "workbuddy"]);
    }

    /// find 大小写不敏感;未注册 id 返回 None(泛化路由据此 404)。
    #[test]
    fn find_is_case_insensitive() {
        assert_eq!(find("Claude").unwrap().id, "claude");
        assert_eq!(find(" Claude-Desktop ").unwrap().id, "claude-desktop");
        assert!(find("cursor").is_none());
    }

    /// registry_json 结构:agents 数组 9 项,每项带完整字段。
    #[test]
    fn registry_json_shape() {
        let v = registry_json();
        let arr = v["agents"].as_array().unwrap();
        assert_eq!(arr.len(), 9);
        for m in arr {
            assert!(m["id"].is_string());
            assert!(m["name"].is_string());
            assert!(m["available"].is_boolean());
            assert!(m["egress"].is_string());
        }
    }
}

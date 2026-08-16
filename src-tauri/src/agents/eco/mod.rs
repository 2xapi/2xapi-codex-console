//! 生态管理(开发组·生态中心,A+B 段):各平台的 MCP 服务器管理。
//!
//! 铁律照抄托管:叠加写入 · 零触碰已有(手动条目 409 拒写)· 备份先行 · 可还原。
//! 来源标记走侧车登记表 `<codex_home>/eco-managed.json`(Console 写过的条目才登记,
//! 登记表没有 = 用户手动添加,只读展示);「停用」语义 = 从平台配置移除 + 登记表留
//! spec 标 enabled=false(启用时写回)——JSON/TOML/YAML 均无原生 disabled 字段,统一此策略。
//!
//! 支持平台独立于 agents 托管注册表(cursor/trae 无托管世界也可管理生态):
//! A 段 = codex(config.toml)+cursor(mcp.json);B 段 = claude-desktop/grokbuild/opencode/hermes/trae。

pub mod codex;
pub mod cursor;
pub mod hermes;
pub mod opencode;

use serde_json::{json, Map, Value};
use std::collections::BTreeMap;
use std::path::{Path, PathBuf};

pub type OpError = (u16, String, String);

/// eco 支持平台表(id 规范化;顺序即前端平台 tab 顺序)。
pub const SUPPORTED: &[&str] = &[
    "codex",
    "cursor",
    "claude-desktop",
    "grokbuild",
    "opencode",
    "hermes",
    "trae",
];

/// 平台显示名(前端平台 tab;独立于托管注册表命名,含无托管世界的 cursor/trae)。
pub fn display_name(agent: &str) -> &'static str {
    match agent {
        "codex" => "Codex",
        "cursor" => "Cursor",
        "claude-desktop" => "Claude 桌面版",
        "grokbuild" => "Grok Build",
        "opencode" => "OpenCode",
        "hermes" => "Hermes",
        "trae" => "TRAE",
        _ => "未知平台",
    }
}

pub fn supported(agent: &str) -> Option<&'static str> {
    let norm = agent.trim().to_ascii_lowercase();
    SUPPORTED.iter().find(|a| **a == norm).copied()
}

/// 平台载体读写抽象。
/// read/write 只动 MCP 段,其余内容零触碰(由实现保证)。
pub trait EcoStore {
    fn id(&self) -> &'static str;
    /// 读平台配置里的全部 MCP 条目(id → 原生 spec Value)。
    fn read(&self) -> Result<BTreeMap<String, Value>, OpError>;
    /// 整体写回 MCP 段(其余内容保留)。
    fn write(&self, servers: &BTreeMap<String, Value>) -> Result<(), OpError>;
    /// 写前备份载体文件。
    fn backup(&self, backup_dir: &Path) -> Result<(), OpError>;
}

/// 侧车登记表路径(providers.json 同目录,本产品数据区)。
fn registry_path(codex_home: &Path) -> PathBuf {
    codex_home.join("eco-managed.json")
}

fn load_registry(codex_home: &Path) -> Map<String, Value> {
    let raw = std::fs::read_to_string(registry_path(codex_home)).unwrap_or_default();
    serde_json::from_str::<Value>(&raw)
        .ok()
        .and_then(|v| v.get("agents").and_then(|a| a.as_object().cloned()))
        .unwrap_or_default()
}

fn save_registry(codex_home: &Path, agents: &Map<String, Value>) {
    let path = registry_path(codex_home);
    if let Some(parent) = path.parent() {
        std::fs::create_dir_all(parent).ok();
    }
    let body = json!({ "version": 1, "agents": agents });
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

/// 登记表里该平台的名册(name → {enabled, spec})。
fn agent_registry(agents: &Map<String, Value>, agent: &str) -> Map<String, Value> {
    agents
        .get(agent)
        .and_then(|v| v.as_object().cloned())
        .unwrap_or_default()
}

fn is_valid_name(name: &str) -> bool {
    !name.is_empty()
        && name.len() <= 64
        && name
            .chars()
            .all(|c| c.is_ascii_alphanumeric() || c == '-' || c == '_')
}

/// spec 规范化与校验:A 期仅支持 stdio 型(command + 可选 args/env)。
/// Windows 下 npx 等命令需 cmd /c 包装(照抄 cc-switch claude_mcp.rs 实证)。
#[cfg(windows)]
const WRAP_COMMANDS: &[&str] = &["npx", "npm", "yarn", "pnpm", "node", "bun", "deno"];

fn normalize_spec(spec: &Value) -> Result<Value, OpError> {
    let bad = |msg: &str| (400u16, "E_ECO_BAD_SPEC".to_string(), msg.to_string());
    let Some(obj) = spec.as_object() else {
        return Err(bad("服务器配置必须是对象"));
    };
    let mut out = Map::new();
    if let Some(cmd) = obj.get("command").and_then(|v| v.as_str()) {
        let cmd = cmd.trim();
        if cmd.is_empty() {
            return Err(bad("command 不能为空"));
        }
        out.insert("command".into(), json!(cmd));
        if let Some(args) = obj.get("args").and_then(|v| v.as_array()) {
            let list: Vec<Value> = args
                .iter()
                .map(|a| json!(a.as_str().unwrap_or_default()))
                .collect();
            out.insert("args".into(), Value::Array(list));
        }
        if let Some(env) = obj.get("env").and_then(|v| v.as_object()) {
            let mut m = Map::new();
            for (k, v) in env {
                m.insert(k.clone(), json!(v.as_str().unwrap_or_default()));
            }
            if !m.is_empty() {
                out.insert("env".into(), Value::Object(m));
            }
        }
    } else {
        return Err(bad(
            "A 期仅支持 stdio 型 MCP 服务器(command + args),远程型后续开放",
        ));
    }
    let mut v = Value::Object(out);
    wrap_for_windows(&mut v);
    Ok(v)
}

#[cfg(windows)]
fn wrap_for_windows(spec: &mut Value) {
    let Some(obj) = spec.as_object_mut() else {
        return;
    };
    let Some(cmd) = obj
        .get("command")
        .and_then(|v| v.as_str())
        .unwrap_or_default()
        .to_string();
    let stem = Path::new(&cmd)
        .file_stem()
        .and_then(|s| s.to_str())
        .unwrap_or(&cmd)
        .to_ascii_lowercase();
    if !WRAP_COMMANDS.contains(&stem.as_str()) {
        return;
    }
    let mut args = vec![json!("/c"), json!(cmd)];
    if let Some(old) = obj.get("args").and_then(|v| v.as_array()).cloned() {
        args.extend(old);
    }
    obj.insert("command".into(), json!("cmd"));
    obj.insert("args".into(), Value::Array(args));
}

#[cfg(not(windows))]
fn wrap_for_windows(_spec: &mut Value) {}

fn spec_summary(spec: &Value) -> String {
    if let Some(cmd) = spec.get("command").and_then(|v| v.as_str()) {
        let args = spec
            .get("args")
            .and_then(|v| v.as_array())
            .map(|a| {
                a.iter()
                    .filter_map(|x| x.as_str())
                    .collect::<Vec<_>>()
                    .join(" ")
            })
            .unwrap_or_default();
        return if args.is_empty() {
            cmd.to_string()
        } else {
            format!("{cmd} {args}")
        };
    }
    if let Some(url) = spec.get("url").and_then(|v| v.as_str()) {
        return url.to_string();
    }
    "—".to_string()
}

/// GET 列表:平台配置实际条目(manual/console)+ 登记表停用条目(console, enabled=false)。
pub fn list(store: &dyn EcoStore, codex_home: &Path) -> Result<Value, OpError> {
    let live = store.read()?;
    let agents = load_registry(codex_home);
    let roster = agent_registry(&agents, store.id());
    let mut servers: Vec<Value> = Vec::new();
    let mut seen: Vec<String> = Vec::new();
    for (name, spec) in &live {
        let source = if roster.contains_key(name) {
            "console"
        } else {
            "manual"
        };
        servers.push(json!({
            "id": name, "name": name, "source": source, "enabled": true,
            "summary": spec_summary(spec), "spec": spec,
        }));
        seen.push(name.clone());
    }
    for (name, entry) in &roster {
        if seen.contains(name) {
            continue;
        }
        let spec = entry.get("spec").cloned().unwrap_or(Value::Null);
        servers.push(json!({
            "id": name, "name": name, "source": "console", "enabled": false,
            "summary": spec_summary(&spec), "spec": spec,
        }));
    }
    servers.sort_by(|a, b| {
        a["id"]
            .as_str()
            .unwrap_or("")
            .cmp(b["id"].as_str().unwrap_or(""))
    });
    Ok(json!({
        "agent": store.id(),
        "servers": servers,
        "tabs": [
            { "id": "mcp", "label": "MCP 服务器", "ready": true },
            { "id": "plugins", "label": "插件", "ready": false },
            { "id": "skills", "label": "技能", "ready": false }
        ],
    }))
}

fn registry_upsert(codex_home: &Path, agent: &str, name: &str, enabled: bool, spec: &Value) {
    let mut agents = load_registry(codex_home);
    let entry = agents.entry(agent.to_string()).or_insert_with(|| json!({}));
    if let Some(m) = entry.as_object_mut() {
        m.insert(
            name.to_string(),
            json!({ "enabled": enabled, "spec": spec }),
        );
    }
    save_registry(codex_home, &agents);
}

fn registry_remove(codex_home: &Path, agent: &str, name: &str) {
    let mut agents = load_registry(codex_home);
    if let Some(entry) = agents.get_mut(agent).and_then(|v| v.as_object_mut()) {
        entry.remove(name);
        if entry.is_empty() {
            agents.remove(agent);
        }
    }
    save_registry(codex_home, &agents);
}

/// 手动条目拦截:live 有但登记表无 → 409(零触碰铁律)。
fn guard_manual(
    live: &BTreeMap<String, Value>,
    roster: &Map<String, Value>,
    name: &str,
) -> Result<(), OpError> {
    if live.contains_key(name) && !roster.contains_key(name) {
        return Err((
            409,
            "E_ECO_MANUAL".into(),
            format!(
                "「{name}」为手动添加的条目,Console 不改动手动配置;如需移除请在平台配置中自行操作"
            ),
        ));
    }
    Ok(())
}

/// 安装(预设或自定义):写平台配置 + 登记。同名手动条目 409;Console 条目幂等覆盖。
pub fn install(
    store: &dyn EcoStore,
    codex_home: &Path,
    backup_dir: &Path,
    name: &str,
    spec: &Value,
) -> Result<Value, OpError> {
    if !is_valid_name(name) {
        return Err((
            400,
            "E_ECO_BAD_NAME".into(),
            "名称仅限字母/数字/-/_,长度 1-64".into(),
        ));
    }
    let spec = normalize_spec(spec)?;
    let mut live = store.read()?;
    let roster = agent_registry(&load_registry(codex_home), store.id());
    guard_manual(&live, &roster, name)?;
    store.backup(backup_dir)?;
    live.insert(name.to_string(), spec.clone());
    store.write(&live)?;
    registry_upsert(codex_home, store.id(), name, true, &spec);
    list(store, codex_home)
}

/// 停用:从平台配置移除,登记表标 enabled=false(spec 保留供启用写回)。
pub fn disable(
    store: &dyn EcoStore,
    codex_home: &Path,
    backup_dir: &Path,
    name: &str,
) -> Result<Value, OpError> {
    let live = store.read()?;
    let roster = agent_registry(&load_registry(codex_home), store.id());
    guard_manual(&live, &roster, name)?;
    if !roster.contains_key(name) {
        return Err((404, "E_ECO_NOT_FOUND".into(), format!("条目不存在: {name}")));
    }
    if live.contains_key(name) {
        store.backup(backup_dir)?;
        let mut next = live.clone();
        next.remove(name);
        store.write(&next)?;
    }
    let spec = roster
        .get(name)
        .and_then(|e| e.get("spec"))
        .cloned()
        .unwrap_or(Value::Null);
    registry_upsert(codex_home, store.id(), name, false, &spec);
    list(store, codex_home)
}

/// 启用:登记表停用条目写回平台配置。
pub fn enable(
    store: &dyn EcoStore,
    codex_home: &Path,
    backup_dir: &Path,
    name: &str,
) -> Result<Value, OpError> {
    let live = store.read()?;
    let roster = agent_registry(&load_registry(codex_home), store.id());
    if !roster.contains_key(name) {
        if live.contains_key(name) {
            return Err((
                409,
                "E_ECO_MANUAL".into(),
                "手动条目默认启用,无需此操作".into(),
            ));
        }
        return Err((404, "E_ECO_NOT_FOUND".into(), format!("条目不存在: {name}")));
    }
    if !live.contains_key(name) {
        let spec = roster
            .get(name)
            .and_then(|e| e.get("spec"))
            .cloned()
            .unwrap_or(Value::Null);
        store.backup(backup_dir)?;
        let mut next = live.clone();
        next.insert(name.to_string(), spec);
        store.write(&next)?;
    }
    let spec = roster
        .get(name)
        .and_then(|e| e.get("spec"))
        .cloned()
        .unwrap_or(Value::Null);
    registry_upsert(codex_home, store.id(), name, true, &spec);
    list(store, codex_home)
}

/// 卸载:平台配置移除 + 登记表删登记。
pub fn uninstall(
    store: &dyn EcoStore,
    codex_home: &Path,
    backup_dir: &Path,
    name: &str,
) -> Result<Value, OpError> {
    let live = store.read()?;
    let roster = agent_registry(&load_registry(codex_home), store.id());
    guard_manual(&live, &roster, name)?;
    if !roster.contains_key(name) {
        return Err((404, "E_ECO_NOT_FOUND".into(), format!("条目不存在: {name}")));
    }
    if live.contains_key(name) {
        store.backup(backup_dir)?;
        let mut next = live.clone();
        next.remove(name);
        store.write(&next)?;
    }
    registry_remove(codex_home, store.id(), name);
    list(store, codex_home)
}

// ── MCP 预设市场 ──────────────────────────────────────────────
// 预设数据从 cc-switch(mcpPresets.ts)提炼 + 演示 HTML 市场清单;
// B 段新增 filesystem/sqlite(装时填参:args 里 $KEY 占位,前端弹参数表单)。

/// 装时填参预设的参数声明。
pub struct PresetParam {
    pub key: &'static str,
    pub label: &'static str,
    pub placeholder: &'static str,
    pub required: bool,
}

pub struct Preset {
    pub id: &'static str,
    pub name: &'static str,
    pub desc: &'static str,
    pub command: &'static str,
    /// 可含 $KEY 占位(装时由 params 替换)
    pub args: &'static [&'static str],
    pub needs_env: &'static [&'static str],
    pub params: &'static [PresetParam],
}

pub const PRESETS: &[Preset] = &[
    Preset {
        id: "playwright",
        name: "Playwright",
        desc: "浏览器自动化 · 截图 · 表单",
        command: "npx",
        args: &["@playwright/mcp@latest"],
        needs_env: &[],
        params: &[],
    },
    Preset {
        id: "github",
        name: "GitHub",
        desc: "仓库 · PR · Issue 全操作",
        command: "npx",
        args: &["@modelcontextprotocol/server-github"],
        needs_env: &["GITHUB_TOKEN"],
        params: &[],
    },
    Preset {
        id: "fetch",
        name: "Fetch",
        desc: "网页抓取转 Markdown",
        command: "uvx",
        args: &["mcp-server-fetch"],
        needs_env: &[],
        params: &[],
    },
    Preset {
        id: "context7",
        name: "Context7",
        desc: "库文档实时检索",
        command: "npx",
        args: &["-y", "@upstash/context7-mcp"],
        needs_env: &[],
        params: &[],
    },
    Preset {
        id: "memory",
        name: "Memory",
        desc: "跨会话知识图谱",
        command: "npx",
        args: &["-y", "@modelcontextprotocol/server-memory"],
        needs_env: &[],
        params: &[],
    },
    Preset {
        id: "sequentialthinking",
        name: "Sequential Thinking",
        desc: "结构化推理",
        command: "npx",
        args: &["-y", "@modelcontextprotocol/server-sequentialthinking"],
        needs_env: &[],
        params: &[],
    },
    Preset {
        id: "filesystem",
        name: "Filesystem",
        desc: "受控目录文件读写(装时选目录)",
        command: "npx",
        args: &["-y", "@modelcontextprotocol/server-filesystem", "$DIR"],
        needs_env: &[],
        params: &[PresetParam {
            key: "DIR",
            label: "允许访问的目录(绝对路径)",
            placeholder: "/Users/you/Documents",
            required: true,
        }],
    },
    Preset {
        id: "sqlite",
        name: "SQLite",
        desc: "数据库查询浏览(装时选库文件)",
        command: "uvx",
        args: &["mcp-server-sqlite", "--db-path", "$DB"],
        needs_env: &[],
        params: &[PresetParam {
            key: "DB",
            label: "数据库文件路径(不存在会创建)",
            placeholder: "/Users/you/data.db",
            required: true,
        }],
    },
];

pub fn find_preset(id: &str) -> Option<&'static Preset> {
    PRESETS.iter().find(|p| p.id == id)
}

pub fn presets_json() -> Value {
    json!({
        "presets": PRESETS
            .iter()
            .map(|p| {
                json!({
                    "id": p.id, "name": p.name, "desc": p.desc,
                    "command": p.command, "args": p.args,
                    "needsEnv": p.needs_env,
                    "params": p.params.iter().map(|x| json!({
                        "key": x.key, "label": x.label,
                        "placeholder": x.placeholder, "required": x.required,
                    })).collect::<Vec<_>>(),
                })
            })
            .collect::<Vec<_>>(),
        "agents": SUPPORTED
            .iter()
            .map(|a| json!({ "id": a, "name": display_name(a) }))
            .collect::<Vec<_>>(),
    })
}

/// 按预设生成安装 spec。params:前端提交的装时参数(装时填参预设);
/// required 缺失 → 400;$KEY 占位替换进 args。needs_env 条目 env 值留空占位,前端提示填写。
pub fn preset_spec(p: &Preset, params: Option<&Value>) -> Result<Value, OpError> {
    let mut obj = Map::new();
    obj.insert("command".into(), json!(p.command));
    let mut args: Vec<String> = p.args.iter().map(|a| a.to_string()).collect();
    for param in p.params {
        let val = params
            .and_then(|m| m.get(param.key))
            .and_then(|v| v.as_str())
            .unwrap_or("")
            .trim()
            .to_string();
        if param.required && val.is_empty() {
            return Err((
                400,
                "E_ECO_PARAM_REQUIRED".into(),
                format!("「{}」需要填写:{}", p.name, param.label),
            ));
        }
        for a in args.iter_mut() {
            *a = a.replace(&format!("${}", param.key), &val);
        }
    }
    obj.insert("args".into(), Value::Array(args.into_iter().map(|a| json!(a)).collect()));
    if !p.needs_env.is_empty() {
        let mut env = Map::new();
        for k in p.needs_env {
            env.insert(k.to_string(), json!(""));
        }
        obj.insert("env".into(), Value::Object(env));
    }
    Ok(Value::Object(obj))
}

#[cfg(test)]
mod tests {
    use super::*;

    /// 内存 EcoStore 测试替身。
    struct Mem {
        id: &'static str,
        file: std::cell::RefCell<BTreeMap<String, Value>>,
    }

    impl EcoStore for Mem {
        fn id(&self) -> &'static str {
            self.id
        }
        fn read(&self) -> Result<BTreeMap<String, Value>, OpError> {
            Ok(self.file.borrow().clone())
        }
        fn write(&self, servers: &BTreeMap<String, Value>) -> Result<(), OpError> {
            *self.file.borrow_mut() = servers.clone();
            Ok(())
        }
        fn backup(&self, _backup_dir: &Path) -> Result<(), OpError> {
            Ok(())
        }
    }

    fn setup(tag: &str) -> (std::path::PathBuf, Mem) {
        let root = std::env::temp_dir().join(format!("2xapi-eco-{tag}-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&root);
        std::fs::create_dir_all(&root).unwrap();
        let store = Mem {
            id: "codex",
            file: std::cell::RefCell::new(BTreeMap::new()),
        };
        (root, store)
    }

    fn spec() -> Value {
        json!({ "command": "npx", "args": ["mcp-server-x"] })
    }

    #[test]
    fn install_then_list_and_uninstall() {
        let (root, store) = setup("cycle");
        // 预安装一个手动条目(直接写平台配置,不经 Console)
        store.file.borrow_mut().insert("manual-one".into(), spec());

        let v = install(&store, &root, &root, "fetch", &spec()).unwrap();
        let servers = v["servers"].as_array().unwrap();
        let fetch = servers.iter().find(|s| s["id"] == "fetch").unwrap();
        assert_eq!(fetch["source"], "console");
        let manual = servers.iter().find(|s| s["id"] == "manual-one").unwrap();
        assert_eq!(manual["source"], "manual");

        // 手动条目 install 同名 → 409
        let err = install(&store, &root, &root, "manual-one", &spec()).unwrap_err();
        assert_eq!(err.0, 409);
        assert_eq!(err.1, "E_ECO_MANUAL");

        // disable → 不在 live,登记表 enabled=false
        let v = disable(&store, &root, &root, "fetch").unwrap();
        let fetch = v["servers"]
            .as_array()
            .unwrap()
            .iter()
            .find(|s| s["id"] == "fetch")
            .unwrap();
        assert_eq!(fetch["enabled"], Value::Bool(false));
        assert!(!store.file.borrow().contains_key("fetch"));

        // enable → 写回
        let v = enable(&store, &root, &root, "fetch").unwrap();
        let fetch = v["servers"]
            .as_array()
            .unwrap()
            .iter()
            .find(|s| s["id"] == "fetch")
            .unwrap();
        assert_eq!(fetch["enabled"], Value::Bool(true));
        assert!(store.file.borrow().contains_key("fetch"));

        // uninstall → live+登记表双清;手动条目仍在
        let v = uninstall(&store, &root, &root, "fetch").unwrap();
        let ids: Vec<&str> = v["servers"]
            .as_array()
            .unwrap()
            .iter()
            .map(|s| s["id"].as_str().unwrap())
            .collect();
        assert_eq!(ids, vec!["manual-one"]);
        // 手动条目 uninstall → 409
        let err = uninstall(&store, &root, &root, "manual-one").unwrap_err();
        assert_eq!(err.0, 409);
        // 不存在 → 404
        assert_eq!(uninstall(&store, &root, &root, "nope").unwrap_err().0, 404);
    }

    #[test]
    fn install_idempotent_overwrites_console_entry() {
        let (root, store) = setup("idem");
        install(
            &store,
            &root,
            &root,
            "fetch",
            &json!({ "command": "uvx", "args": ["mcp-server-fetch"] }),
        )
        .unwrap();
        // 同名再装(Console 自己的)→ 幂等覆盖,不 409
        install(&store, &root, &root, "fetch", &spec()).unwrap();
        assert_eq!(store.file.borrow()["fetch"]["command"], "npx");
    }

    #[test]
    fn bad_names_and_specs_rejected() {
        let (root, store) = setup("validate");
        assert_eq!(
            install(&store, &root, &root, "", &spec()).unwrap_err().1,
            "E_ECO_BAD_NAME"
        );
        assert_eq!(
            install(&store, &root, &root, "a b", &spec()).unwrap_err().1,
            "E_ECO_BAD_NAME"
        );
        assert_eq!(
            install(&store, &root, &root, "ok", &json!({ "url": "https://x" }))
                .unwrap_err()
                .1,
            "E_ECO_BAD_SPEC"
        );
        assert_eq!(
            install(&store, &root, &root, "ok", &json!({ "command": "" }))
                .unwrap_err()
                .1,
            "E_ECO_BAD_SPEC"
        );
    }

    #[test]
    fn presets_shape_and_spec() {
        let v = presets_json();
        let presets = v["presets"].as_array().unwrap();
        assert_eq!(presets.len(), 8);

        let gh = find_preset("github").unwrap();
        assert_eq!(gh.needs_env, &["GITHUB_TOKEN"]);
        let spec = preset_spec(gh, None).unwrap();
        assert_eq!(spec["env"]["GITHUB_TOKEN"], "");
        let mem = find_preset("memory").unwrap();
        assert!(preset_spec(mem, None).unwrap().get("env").is_none());
        // 装时填参:required 缺失 400;参数替换进 args
        let fs_p = find_preset("filesystem").unwrap();
        assert!(preset_spec(fs_p, None).is_err(), "filesystem 无参数应 400");
        let spec = preset_spec(fs_p, Some(&serde_json::json!({ "DIR": "/tmp/docs" }))).unwrap();
        assert_eq!(spec["args"][2], "/tmp/docs", "$DIR 占位应被替换");
        assert_eq!(v["agents"].as_array().unwrap().len(), 7, "B 段支持 7 平台");
    }

    #[test]
    fn supported_table_and_summary() {
        assert_eq!(supported("Codex"), Some("codex"));
        assert_eq!(supported("cursor"), Some("cursor"));
        assert_eq!(supported("gemini"), None, "gemini 无 MCP 载体,B 段未入编");
        assert_eq!(
            spec_summary(&json!({ "command": "npx", "args": ["a", "b"] })),
            "npx a b"
        );
        assert_eq!(spec_summary(&json!({ "url": "https://x" })), "https://x");
    }
}

/// 真机 e2e(#[ignore],cargo test -- --ignored eco_real 手动驱动;grok 批次先例)。
/// codex:真实 ~/.codex/config.toml 字节副本上 install→diff 精确→uninstall→diff=零。
/// cursor:真实 HOME 写入 ~/.cursor/mcp.json(原文件不存在)→验证→uninstall→文件删除零残留。
/// 真实 ~/.codex 与 ~/.cursor 全程零触碰(codex 走副本;cursor 写入物为新建文件,验后即删)。
#[cfg(test)]
mod real {
    use super::*;
    use std::path::PathBuf;

    fn real_home() -> PathBuf {
        PathBuf::from(std::env::var("HOME").unwrap_or_default())
    }

    #[test]
    #[ignore = "真机验收:读真实 ~/.codex 副本与真实 HOME(只读+新建即删),手动驱动"]
    fn eco_real_machine_e2e() {
        // ── codex:字节副本 ──
        let home = real_home();
        let real_config = home.join(".codex").join("config.toml");
        let tmp = std::env::temp_dir().join(format!("2xapi-eco-real-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&tmp);
        std::fs::create_dir_all(&tmp).unwrap();
        assert!(real_config.exists(), "真实 config.toml 应存在");
        let original = std::fs::read(&real_config).unwrap();
        std::fs::write(tmp.join("config.toml"), &original).unwrap();

        let store = crate::agents::eco::codex::TomlStore::new(&tmp.join("config.toml"));
        let backup_dir = tmp.join("backups");
        std::fs::create_dir_all(&backup_dir).unwrap();

        let before: BTreeMap<String, Value> = store.read().unwrap();
        println!(
            "[codex] 副本已有 MCP 条目: {:?}",
            before.keys().collect::<Vec<_>>()
        );

        // install 预设 fetch
        let v = install(
            &store,
            &tmp,
            &backup_dir,
            "fetch",
            &preset_spec(find_preset("fetch").unwrap(), None).unwrap(),
        )
        .unwrap();
        let after = store.read().unwrap();
        assert!(after.contains_key("fetch"), "install 后应含 fetch");
        for k in before.keys() {
            assert!(after.contains_key(k), "已有条目 {k} 必须保留");
            assert_eq!(after[k], before[k], "已有条目 {k} 内容必须零变化");
        }
        let src = v["servers"]
            .as_array()
            .unwrap()
            .iter()
            .find(|s| s["id"] == "fetch")
            .unwrap();
        assert_eq!(src["source"], "console");

        // diff 精确性:除 mcp_servers 外的顶层键零变化
        let orig_toml: toml::Value = String::from_utf8(original.clone())
            .unwrap()
            .parse()
            .unwrap();
        let now_toml: toml::Value = std::fs::read_to_string(tmp.join("config.toml"))
            .unwrap()
            .parse()
            .unwrap();
        for (k, v0) in orig_toml.as_table().unwrap() {
            if k == "mcp_servers" {
                continue;
            }
            assert_eq!(now_toml.get(k), Some(v0), "顶层键 {k} 零触碰");
        }

        // disable → 移除;enable → 写回
        disable(&store, &tmp, &backup_dir, "fetch").unwrap();
        assert!(!store.read().unwrap().contains_key("fetch"));
        enable(&store, &tmp, &backup_dir, "fetch").unwrap();
        assert!(store.read().unwrap().contains_key("fetch"));

        // uninstall → mcp_servers 段回到原样(原有条目数),其他键不变
        uninstall(&store, &tmp, &backup_dir, "fetch").unwrap();
        let final_toml: toml::Value = std::fs::read_to_string(tmp.join("config.toml"))
            .unwrap()
            .parse()
            .unwrap();
        let final_mcp = final_toml
            .get("mcp_servers")
            .and_then(|m| m.as_table())
            .map(|m| m.len())
            .unwrap_or(0);
        let orig_mcp = orig_toml
            .get("mcp_servers")
            .and_then(|m| m.as_table())
            .map(|m| m.len())
            .unwrap_or(0);
        assert_eq!(final_mcp, orig_mcp, "uninstall 后 MCP 段条目数应回到原始");
        assert!(backup_dir.read_dir().unwrap().count() >= 1, "备份链存在");
        let _ = std::fs::remove_dir_all(&tmp);
        println!("[codex] 真机副本 e2e 通过");

        // ── cursor:真实 HOME 写入(原文件不存在)──
        assert!(
            !home.join(".cursor").join("mcp.json").exists(),
            "前提:真实 ~/.cursor/mcp.json 不存在(存在则本测试不应运行)"
        );
        let cstore = crate::agents::eco::cursor::JsonStore::new(&home);
        let cbackup = home.join(".codex").join("config-backups");
        let v = install(
            &cstore,
            &home.join(".codex"),
            &cbackup,
            "playwright",
            &preset_spec(find_preset("playwright").unwrap(), None).unwrap(),
        )
        .unwrap();
        let raw = std::fs::read_to_string(home.join(".cursor").join("mcp.json")).unwrap();
        let doc: Value = serde_json::from_str(&raw).unwrap();
        assert_eq!(
            doc["mcpServers"]["playwright"]["command"], "npx",
            "真实写入形状"
        );
        assert_eq!(
            doc["mcpServers"]["playwright"]["args"][0],
            "@playwright/mcp@latest"
        );
        assert!(v["servers"]
            .as_array()
            .unwrap()
            .iter()
            .any(|s| s["id"] == "playwright"));

        uninstall(&cstore, &home.join(".codex"), &cbackup, "playwright").unwrap();
        // 卸载后:登记表无 playwright,mcpServers 段清空
        let after_raw =
            std::fs::read_to_string(home.join(".cursor").join("mcp.json")).unwrap_or_default();
        let after_doc: Value = serde_json::from_str(&after_raw).unwrap_or(Value::Null);
        assert!(
            after_doc.get("mcpServers").is_none(),
            "卸载后 mcpServers 段应清空"
        );
        // 零残留:只删本测试新建的 mcp.json;~/.cursor 目录是 Cursor IDE 用户数据目录
        // (真机实证:含 extensions/plugins/projects 等,始终存在),任何情况下不得删除目录本身。
        let _ = std::fs::remove_file(home.join(".cursor").join("mcp.json"));
        assert!(!home.join(".cursor").join("mcp.json").exists(), "零残留");
        println!("[cursor] 真机写入+卸载+零残留 通过");
    }
}

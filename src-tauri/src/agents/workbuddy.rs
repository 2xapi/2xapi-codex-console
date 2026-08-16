//! WorkBuddy / CodeBuddy adapter(多平台方案 B 阶段;批次任务书 workbuddy,2026-08-16)
//!
//! 叠加平台(D1 语义):CLI(`~/.codebuddy/models.json`)与桌面版(`~/.workbuddy/models.json`)
//! 两个载体的 `models` 数组仅追加/覆盖 `vendor=2xapi-gateway` 的条目;`availableModels`
//! (项目级完全覆盖语义,写了会隐藏用户已有自定义模型)与用户条目零触碰;
//! `settings.json` 的 `model` 指针属用户偏好,本产品恒不写(unhost 也不碰)。
//!
//! 实证依据(workbuddy批次探索结论.md,2026-08-16 罗盘):
//! - 双路径完全分离,桌面版 CustomModelsProductProvider 热监听 models.json
//! - `${VAR}` 仅解析真实进程 env,桌面 App 注不进 → 统一直接写 provider.api_key 值
//! - url 必须完整路径以 `/chat/completions` 结尾;协议仅 OpenAI Chat → 网关零改动
//! - 同 id 覆盖/异 id 追加(SmartMerge);首版单条目,多模型条目集留后续批次

use serde_json::{json, Value};
use std::path::{Path, PathBuf};

/// 本产品在 models.json 里的身份标记:条目 vendor 与 id 均用它,unhost 按 vendor 整集移除。
pub const VENDOR: &str = "2xapi-gateway";
/// 网关 Chat 入口(gateway.rs 根路径直收 /chat/completions)。
const GATEWAY_CHAT_URL: &str = "http://127.0.0.1:8787/workbuddy/v1/chat/completions";

type OpError = (u16, String, String);

/// 两个配置载体:CLI 与桌面版,互不读取对方目录(实证),同一套条目各写一份。
fn config_roots(home: &Path) -> Vec<(&'static str, PathBuf)> {
    vec![("cli", home.join(".codebuddy")), ("desktop", home.join(".workbuddy"))]
}

fn models_path(root: &Path) -> PathBuf {
    root.join("models.json")
}

/// 读 models.json;无文件→空对象;坏 JSON→E_PARSE 拒碰用户文件(不擅自治愈)。
fn read_models(root: &Path) -> Result<Value, OpError> {
    let p = models_path(root);
    if !p.exists() {
        return Ok(json!({}));
    }
    let raw = std::fs::read_to_string(&p).map_err(|e| (500, "E_IO".into(), e.to_string()))?;
    serde_json::from_str(&raw).map_err(|_| {
        (422, "E_PARSE".into(), format!("{} 不是合法 JSON,请先手动修复(本产品不改动坏文件)", p.display()))
    })
}

/// 移除本产品条目集后写回前的载荷;返回 (新对象, 是否移除过条目)。
fn strip_ours(cfg: &Value) -> (Value, bool) {
    let mut out = cfg.clone();
    let mut removed = false;
    if let Some(models) = out.get_mut("models").and_then(|v| v.as_array_mut()) {
        let before = models.len();
        models.retain(|m| m.get("vendor").and_then(|v| v.as_str()) != Some(VENDOR));
        removed = models.len() != before;
    }
    (out, removed)
}

/// 生成条目(网关模式 url 指向本机网关;direct 指向上游站,拼法对齐 gateway.rs:202)。
fn build_entry(provider: &crate::providers::Provider, way: &str) -> Value {
    let url = if way == "gateway" {
        GATEWAY_CHAT_URL.to_string()
    } else {
        format!("{}/chat/completions", provider.base_url.trim_end_matches('/'))
    };
    json!({
        "id": VENDOR,
        "name": format!("2xapi 网关({})", provider.name),
        "vendor": VENDOR,
        "apiKey": provider.api_key,
        "url": url,
        "maxInputTokens": 128000,
        "maxOutputTokens": 16384,
        "supportsToolCall": true,
        "supportsImages": false,
    })
}

/// 原子写(临时文件+rename),权限 600(models.json 官方建议,含 Key)。
fn write_models_atomic(root: &Path, cfg: &Value) -> Result<(), OpError> {
    std::fs::create_dir_all(root).map_err(|e| (500, "E_IO".into(), e.to_string()))?;
    let p = models_path(root);
    let tmp = p.with_extension("json.tmp");
    let raw = format!("{}\n", serde_json::to_string_pretty(cfg).map_err(|e| (500, "E_IO".into(), e.to_string()))?);
    std::fs::write(&tmp, raw).map_err(|e| (500, "E_IO".into(), e.to_string()))?;
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        let _ = std::fs::set_permissions(&tmp, std::fs::Permissions::from_mode(0o600));
    }
    std::fs::rename(&tmp, &p).map_err(|e| (500, "E_IO".into(), e.to_string()))
}

/// host 单载体:已含本产品条目且内容一致→no-op;否则备份后写入(幂等由 diff 保证)。
fn host_root(root: &Path, entry: &Value, backup_dir: &Path) -> Result<bool, OpError> {
    let cfg = read_models(root)?;
    let (mut merged, _) = strip_ours(&cfg); // 先清旧条目(同 id 覆盖语义)
    if let Some(obj) = merged.as_object_mut() {
        let models = obj.entry("models").or_insert(json!([]));
        if let Some(arr) = models.as_array_mut() {
            arr.push(entry.clone());
        }
    }
    if serde_json::to_string_pretty(&merged).ok() == serde_json::to_string_pretty(&cfg).ok() {
        return Ok(false); // 内容一致(幂等):不写盘不备份
    }
    let p = models_path(root);
    if p.exists() {
        // backup_file 不自建目录(main.rs 生产路径建过);adapter 自愈,测试与直调同安全
        std::fs::create_dir_all(backup_dir).map_err(|e| (500, "E_IO".into(), e.to_string()))?;
        crate::config::backup_file(&p, backup_dir, "workbuddy-models", "pre-host")
            .map_err(|e| (500, "E_IO".into(), e))?;
    }
    write_models_atomic(root, &merged)?;
    Ok(true)
}

/// POST /api/desktop/workbuddy/host {providerId, way}
pub fn host(
    wb_home: &Path,
    backup_dir: &Path,
    providers_path: &Path,
    provider_id: &str,
    way: &str,
) -> Result<Value, OpError> {
    if way != "gateway" && way != "direct" {
        return Err((400, "E_BAD_WAY".into(), "未知托管方式,仅支持 gateway / direct".into()));
    }
    let data = crate::providers::load(providers_path);
    let provider = data
        .providers
        .iter()
        .find(|p| p.id == provider_id)
        .cloned()
        .ok_or_else(|| (404, "E_PROVIDER_NOT_FOUND".to_string(), "找不到该供应商".to_string()))?;
    if provider.model.is_empty() {
        return Err((422, "E_NO_MODEL".to_string(), "该供应商未配置默认模型,请先在编辑里拉取模型或手填".into()));
    }
    let entry = build_entry(&provider, way);
    let mut changed = serde_json::Map::new();
    for (key, root) in config_roots(wb_home) {
        let wrote = host_root(&root, &entry, backup_dir)?;
        changed.insert(key.into(), json!(wrote));
    }
    crate::providers::set_active(providers_path, &provider.id);
    Ok(json!({
        "hosted": true,
        "way": way,
        "entryId": VENDOR,
        "changed": Value::Object(changed),
        "hint": "叠加平台:模型条目已写入,请在 CodeBuddy/WorkBuddy 模型列表中选择「2xapi 网关」",
    }))
}

/// POST /api/desktop/workbuddy/unhost —— 仅移除本产品条目集;用户条目与 availableModels 不动。
pub fn unhost(wb_home: &Path, backup_dir: &Path) -> Result<Value, OpError> {
    let mut changed = serde_json::Map::new();
    for (key, root) in config_roots(wb_home) {
        let cfg = read_models(&root)?;
        let (merged, removed) = strip_ours(&cfg);
        if !removed {
            changed.insert(key.into(), json!(false));
            continue;
        }
        let p = models_path(&root);
        std::fs::create_dir_all(backup_dir).map_err(|e| (500, "E_IO".into(), e.to_string()))?;
        crate::config::backup_file(&p, backup_dir, "workbuddy-models", "pre-unhost")
            .map_err(|e| (500, "E_IO".into(), e))?;
        write_models_atomic(&root, &merged)?;
        changed.insert(key.into(), json!(true));
    }
    Ok(json!({ "hosted": false, "changed": Value::Object(changed) }))
}

/// GET /api/desktop/workbuddy/state —— 托管态 + 安装检测(安装是验收/UX 提示用,不门控)。
pub fn state(wb_home: &Path) -> Value {
    let mut entries = serde_json::Map::new();
    let mut hosted_any = false;
    for (key, root) in config_roots(wb_home) {
        let p = models_path(&root);
        let (file_exists, ours) = match read_models(&root) {
            Ok(cfg) => {
                let ours = cfg
                    .get("models")
                    .and_then(|v| v.as_array())
                    .map(|a| a.iter().filter(|m| m.get("vendor").and_then(|v| v.as_str()) == Some(VENDOR)).count())
                    .unwrap_or(0);
                (p.exists(), ours)
            }
            Err(_) => (p.exists(), 0), // 坏 JSON:文件在但无法确认,不冒充托管态
        };
        hosted_any = hosted_any || ours > 0;
        entries.insert(key.into(), json!({ "file": file_exists, "ours": ours }));
    }
    let cli_installed = which_codebuddy().is_some();
    let desktop_installed = ["/Applications/WorkBuddy.app", &format!("{}/Applications/WorkBuddy.app", std::env::var("HOME").unwrap_or_default())]
        .iter().any(|p| Path::new(p).exists());
    json!({
        "agent": "workbuddy",
        // hosting 契约对齐 B 阶段通用世界(grokbuild/opencode 等:{…}|null);hosted 保留兼容
        "hosting": if hosted_any { json!({ "way": "gateway", "entryId": VENDOR }) } else { Value::Null },
        "hosted": hosted_any,
        "entries": Value::Object(entries),
        "installed": { "cli": cli_installed, "desktop": desktop_installed },
    })
}

/// PATH 及常见安装位找 codebuddy CLI(只为 UI 提示,找不到不报错)。
fn which_codebuddy() -> Option<PathBuf> {
    let name = if cfg!(windows) { "codebuddy.exe" } else { "codebuddy" };
    for dir in std::env::var("PATH").unwrap_or_default().split(':') {
        if dir.is_empty() { continue; }
        let p = Path::new(dir).join(name);
        if p.exists() { return Some(p); }
    }
    let home = std::env::var("HOME").unwrap_or_default();
    let fallback = [format!("{home}/.local/bin/{name}"), format!("/usr/local/bin/{name}"), format!("/opt/homebrew/bin/{name}")];
    fallback.iter().map(PathBuf::from).find(|p| p.exists())
}

/// POST /api/desktop/workbuddy/start —— CLI 注入式启动信息(命令可复制;桌面版无命令,UI 提示自选模型)。
pub fn start(providers_path: &Path, way: &str, provider_id: &str, wb_home: &Path) -> Result<Value, OpError> {
    let p = if !provider_id.trim().is_empty() {
        let data = crate::providers::load(providers_path);
        data.providers.iter().find(|p| p.id == provider_id).cloned()
            .ok_or((400u16, "E_NO_PROVIDER".to_string(), "供应商不存在".to_string()))?
    } else {
        crate::providers::get_provider_for_agent(providers_path, "workbuddy")
            .ok_or((503u16, "E_NO_WORKBUDDY_PROVIDER".to_string(), "请先选择 WorkBuddy 供应商".to_string()))?
    };
    // 条目须先 host(url/apiKey 都在条目里,start 不再传 Key)
    let hosted = state(wb_home).get("hosted").and_then(|v| v.as_bool()).unwrap_or(false);
    if !hosted {
        return Err((409u16, "E_NOT_HOSTED".to_string(), "请先托管,再启动".to_string()));
    }
    Ok(json!({
        "command": format!("codebuddy --model {VENDOR}"),
        "model": VENDOR,
        "way": way,
        "providerId": p.id,
        "providerName": p.name,
        "desktopHint": "WorkBuddy 桌面版:打开 App,在模型列表选择「2xapi 网关」即可",
    }))
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::fs;

    fn tmp(tag: &str) -> PathBuf {
        let d = std::env::temp_dir().join(format!("wb-test-{}-{}", tag, std::process::id()));
        let _ = fs::remove_dir_all(&d);
        fs::create_dir_all(&d).unwrap();
        d
    }

    fn write_provider_file(dir: &Path) -> PathBuf {
        let p = dir.join("providers.json");
        fs::write(&p, serde_json::json!({
            "providers": [{
                "id": "pv1", "name": "测试站", "agent": "workbuddy",
                "base_url": "https://example.com/v1", "api_key": "sk-test-key",
                "model": "gpt-test",
            }]
        }).to_string()).unwrap();
        p
    }

    /// host 写入断言:双路径条目追加、用户条目与 availableModels 与未知字段不动、幂等。
    #[test]
    fn host_appends_entry_and_preserves_user_data() {
        let home = tmp("host1");
        let backup = home.join("backups");
        fs::create_dir_all(&backup).unwrap();
        let pp = write_provider_file(&home);
        // 用户已有 CLI 配置:自条目 + availableModels + 未知顶层字段
        fs::create_dir_all(home.join(".codebuddy")).unwrap();
        fs::write(home.join(".codebuddy/models.json"), serde_json::json!({
            "models": [
                {"id": "user-model", "name": "User", "vendor": "other", "apiKey": "sk-user",
                 "url": "https://u.example/v1/chat/completions"}
            ],
            "availableModels": ["user-model"],
            "futureField": {"keep": true}
        }).to_string()).unwrap();

        let v = host(&home, &backup, &pp, "pv1", "gateway").unwrap();
        assert_eq!(v["hosted"], json!(true));
        assert_eq!(v["changed"]["cli"], json!(true));
        assert_eq!(v["changed"]["desktop"], json!(true)); // 桌面目录不存在 → 新建写入

        for d in [".codebuddy", ".workbuddy"] {
            let cfg: Value = serde_json::from_str(&fs::read_to_string(home.join(d).join("models.json")).unwrap()).unwrap();
            let models = cfg["models"].as_array().unwrap();
            assert_eq!(models.len(), if d == ".codebuddy" { 2 } else { 1 }, "{d} 应为本产品条目+用户条目");
            let ours = models.iter().find(|m| m["vendor"] == VENDOR).unwrap();
            assert_eq!(ours["url"], GATEWAY_CHAT_URL);
            assert_eq!(ours["apiKey"], "sk-test-key");
            assert_eq!(ours["id"], VENDOR);
            if d == ".codebuddy" {
                assert!(models.iter().any(|m| m["id"] == "user-model"), "用户条目零触碰");
                assert_eq!(cfg["availableModels"], json!(["user-model"]), "availableModels 零触碰");
                assert_eq!(cfg["futureField"]["keep"], json!(true), "未知字段保留");
            }
        }

        // 幂等:同参再 host → 全 no-op,文件字节不变
        let before = fs::read(home.join(".codebuddy/models.json")).unwrap();
        let v2 = host(&home, &backup, &pp, "pv1", "gateway").unwrap();
        assert_eq!(v2["changed"]["cli"], json!(false));
        let after = fs::read(home.join(".codebuddy/models.json")).unwrap();
        assert_eq!(before, after);
    }

    /// direct 的 url 拼接对齐 gateway.rs(trim_end('/') + /chat/completions)。
    #[test]
    fn direct_url_join() {
        let home = tmp("direct");
        let pp = write_provider_file(&home);
        host(&home, &home.join("bk"), &pp, "pv1", "direct").unwrap();
        let cfg: Value = serde_json::from_str(&fs::read_to_string(home.join(".codebuddy/models.json")).unwrap()).unwrap();
        assert_eq!(cfg["models"][0]["url"], "https://example.com/v1/chat/completions");
    }

    /// unhost 仅移除本产品条目;二次 unhost no-op。
    #[test]
    fn unhost_removes_only_ours() {
        let home = tmp("unhost");
        let backup = home.join("backups");
        fs::create_dir_all(&backup).unwrap();
        let pp = write_provider_file(&home);
        host(&home, &backup, &pp, "pv1", "gateway").unwrap();

        let v = unhost(&home, &backup).unwrap();
        assert_eq!(v["hosted"], json!(false));
        assert_eq!(v["changed"]["cli"], json!(true));
        let cfg: Value = serde_json::from_str(&fs::read_to_string(home.join(".codebuddy/models.json")).unwrap()).unwrap();
        assert!(cfg["models"].as_array().unwrap().is_empty());

        let v2 = unhost(&home, &backup).unwrap();
        assert_eq!(v2["changed"]["cli"], json!(false), "二次 unhost no-op");
    }

    /// 坏 JSON 拒碰:E_PARSE 且文件原样。
    #[test]
    fn bad_json_refuses() {
        let home = tmp("bad");
        fs::create_dir_all(home.join(".codebuddy")).unwrap();
        let raw = "{broken json";
        fs::write(home.join(".codebuddy/models.json"), raw).unwrap();
        let pp = write_provider_file(&home);
        let err = host(&home, &home.join("bk"), &pp, "pv1", "gateway").unwrap_err();
        assert_eq!(err.1, "E_PARSE");
        assert_eq!(fs::read_to_string(home.join(".codebuddy/models.json")).unwrap(), raw);
    }

    /// 无模型供应商拒绝 host(与 codex E_NO_MODEL 口径一致)。
    #[test]
    fn no_model_rejects() {
        let home = tmp("nomodel");
        let pp = home.join("providers.json");
        fs::write(&pp, serde_json::json!({
            "providers": [{"id": "pv2", "name": "空", "agent": "workbuddy",
                "base_url": "https://x.example", "api_key": "k", "model": ""}]
        }).to_string()).unwrap();
        let err = host(&home, &home.join("bk"), &pp, "pv2", "gateway").unwrap_err();
        assert_eq!(err.1, "E_NO_MODEL");
    }

    /// state:安装检测不依赖本机真实状态——只断言结构;host 后 hosted=true。
    #[test]
    fn state_shape_and_hosted() {
        let home = tmp("state");
        let pp = write_provider_file(&home);
        let s0 = state(&home);
        assert_eq!(s0["hosted"], json!(false));
        assert!(s0["entries"]["cli"]["file"].is_boolean());
        assert!(s0["installed"]["cli"].is_boolean());
        host(&home, &home.join("bk"), &pp, "pv1", "gateway").unwrap();
        let s1 = state(&home);
        assert_eq!(s1["hosted"], json!(true));
        assert_eq!(s1["hosting"]["way"], "gateway", "通用世界前端读 hosting 判定托管态");
        assert_eq!(s1["entries"]["desktop"]["ours"], 1);
    }

    /// start:未托管 409;托管后返回命令与提示。
    #[test]
    fn start_requires_hosting() {
        let home = tmp("start");
        let pp = write_provider_file(&home);
        let err = start(&pp, "gateway", "pv1", &home).unwrap_err();
        assert_eq!(err.1, "E_NOT_HOSTED");
        host(&home, &home.join("bk"), &pp, "pv1", "gateway").unwrap();
        let v = start(&pp, "gateway", "pv1", &home).unwrap();
        assert_eq!(v["command"], "codebuddy --model 2xapi-gateway");
        assert!(v["desktopHint"].is_string());
    }

    /// 换 way 重写:同 id 覆盖(gateway→direct url 变),条目数不涨。
    #[test]
    fn switch_way_overrides_entry() {
        let home = tmp("switch");
        let pp = write_provider_file(&home);
        host(&home, &home.join("bk"), &pp, "pv1", "gateway").unwrap();
        host(&home, &home.join("bk"), &pp, "pv1", "direct").unwrap();
        let cfg: Value = serde_json::from_str(&fs::read_to_string(home.join(".codebuddy/models.json")).unwrap()).unwrap();
        let models = cfg["models"].as_array().unwrap();
        assert_eq!(models.len(), 1, "同 id 覆盖,条目不重复");
        assert_eq!(models[0]["url"], "https://example.com/v1/chat/completions");
    }
}

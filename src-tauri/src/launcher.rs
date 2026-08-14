//! Codex 启动器（M7，直连版）
//!
//! - 每次「使用」生成独立临时 CODEX_HOME（`/tmp/codex-launch-<uuid>/`）
//! - config.toml 用 `env_key` 从环境变量读 key（key 不写持久文件）
//! - 生成启动脚本 → 打开 macOS 系统终端运行交互式 Codex CLI（直连中转站 base_url，不经过本地网关 8787）
//! - 进程退出（脚本 EXIT trap）自动清理临时目录；`stop` 可主动终止并清理
//! - 完全不碰 `~/.codex` 正式配置与登录态

use serde_json::{json, Value};
use std::collections::HashMap;
use std::path::PathBuf;
use std::sync::Mutex;
use uuid::Uuid;

/// 一个启动会话（脱敏字段不含 api_key）。
#[derive(Debug)]
pub struct LaunchSession {
    pub id: String,
    pub temp_dir: PathBuf,
    pub script_path: PathBuf,
    pub base_url: String,
    pub model: String,
    pub project_dir: String,
    pub started_at: String,
}

impl LaunchSession {
    /// 脚本会把自身 pid 写入 `<temp>/codex.pid`（exec codex 后 pid 不变）。
    fn read_codex_pid(&self) -> Option<u32> {
        let p = self.temp_dir.join("codex.pid");
        std::fs::read_to_string(&p).ok()?.trim().parse::<u32>().ok()
    }
}

/// 全局启动器状态（挂在 AppState 上，跨请求共享）。
#[derive(Default)]
pub struct LauncherState {
    pub sessions: Mutex<HashMap<String, LaunchSession>>,
}

const ENV_KEY_NAME: &str = "CODEX_LAUNCHER_API_KEY";

/// 单引号安全包裹（内部 `'` 转义为 `'\''`），用于写入 shell 脚本。
fn sh_quote(s: &str) -> String {
    format!("'{}'", s.replace('\'', "'\\''"))
}

/// TOML 双引号字符串转义。
fn toml_str(s: &str) -> String {
    format!("\"{}\"", s.replace('\\', "\\\\").replace('"', "\\\""))
}

fn is_alive(pid: u32) -> bool {
    std::process::Command::new("kill")
        .arg("-0")
        .arg(pid.to_string())
        .status()
        .map(|s| s.success())
        .unwrap_or(false)
}

/// 解析 codex CLI 路径：优先 `CODEX_CLI_PATH`，其次 `command -v codex`，最后已知默认路径。
fn resolve_codex_path() -> String {
    if let Ok(p) = std::env::var("CODEX_CLI_PATH") {
        if !p.trim().is_empty() {
            return p.trim().to_string();
        }
    }
    if let Ok(out) = std::process::Command::new("sh")
        .args(["-lc", "command -v codex"])
        .output()
    {
        if out.status.success() {
            let s = String::from_utf8_lossy(&out.stdout).trim().to_string();
            if !s.is_empty() {
                return s;
            }
        }
    }
    "/Applications/ChatGPT.app/Contents/Resources/codex".to_string()
}

/// 写临时 CODEX_HOME 下的 config.toml（env_key 模式，直连 base_url）。
fn write_config(temp_dir: &PathBuf, base_url: &str, model: &str, wire_api: &str) -> Result<(), String> {
    let cfg = format!(
        "model_provider = \"custom\"\nmodel = {}\n\n[model_providers.custom]\nname = \"custom\"\nbase_url = {}\nwire_api = \"{}\"\nenv_key = \"{}\"\nrequires_openai_auth = false\n",
        toml_str(model),
        toml_str(base_url),
        wire_api,
        ENV_KEY_NAME,
    );
    std::fs::write(temp_dir.join("config.toml"), cfg).map_err(|e| format!("写 config.toml 失败: {e}"))
}

/// 写启动脚本：注入 CODEX_HOME / key 环境变量，EXIT 时清理临时目录，exec 交互式 codex。
fn write_script(
    temp_dir: &PathBuf,
    codex: &str,
    api_key: &str,
    model: &str,
    project_dir: &str,
) -> Result<PathBuf, String> {
    let script_path = temp_dir.join("start.command");
    let script = format!(
        "#!/bin/bash\n\
         echo $$ > {pid}\n\
         export CODEX_HOME={home}\n\
         export {env}={key}\n\
         trap 'rm -rf -- {home}' EXIT\n\
         cd {proj} 2>/dev/null || true\n\
         exec {codex} -C {proj} -m {model} --ephemeral --skip-git-repo-check -s workspace-write\n",
        pid = sh_quote(&temp_dir.join("codex.pid").to_string_lossy()),
        home = sh_quote(&temp_dir.to_string_lossy()),
        env = ENV_KEY_NAME,
        key = sh_quote(api_key),
        proj = sh_quote(project_dir),
        codex = sh_quote(codex),
        model = sh_quote(model),
    );
    std::fs::write(&script_path, script).map_err(|e| format!("写启动脚本失败: {e}"))?;
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        std::fs::set_permissions(&script_path, std::fs::Permissions::from_mode(0o700))
            .map_err(|e| format!("设置脚本权限失败: {e}"))?;
    }
    Ok(script_path)
}

/// 打开 macOS 系统终端执行 .command 脚本。
fn open_terminal(script_path: &PathBuf) -> Result<(), String> {
    std::process::Command::new("open")
        .args(["-a", "Terminal"])
        .arg(script_path)
        .spawn()
        .map_err(|e| format!("打开终端失败: {e}"))?;
    Ok(())
}

/// POST /api/launcher/start
/// body 两种来源：
///   - `{ projectDir, providerId, model? }`：key/base_url 从软件 providers.json 取（自己用，不手输）
///   - `{ projectDir, baseUrl, apiKey, model, wireApi? }`：手动直连（客户各自 key → 单独计费）
pub fn start(
    state: &LauncherState,
    input: &Value,
    providers_path: &PathBuf,
) -> Result<Value, String> {
    let project_dir = input
        .get("projectDir")
        .or_else(|| input.get("project_dir"))
        .and_then(|v| v.as_str())
        .unwrap_or("")
        .trim()
        .to_string();
    if project_dir.is_empty() {
        return Err("projectDir 不能为空".into());
    }

    let wire_api = input
        .get("wireApi")
        .and_then(|v| v.as_str())
        .unwrap_or("responses")
        .to_string();
    if wire_api != "responses" && wire_api != "chat_completions" {
        return Err(format!("不支持的 wire_api: {wire_api}"));
    }

    // 来源一：软件已存 provider（key 在 providers.json，前端无需接触）
    let provider_id = input
        .get("providerId")
        .and_then(|v| v.as_str())
        .unwrap_or("")
        .trim()
        .to_string();
    let (base_url, api_key, mut model) = if !provider_id.is_empty() {
        let data = crate::providers::load(providers_path);
        let p = data
            .providers
            .iter()
            .find(|p| p.id == provider_id)
            .ok_or_else(|| "找不到该 Provider".to_string())?;
        (p.base_url.clone(), p.api_key.clone(), p.model.clone())
    } else {
        // 来源二：手动填写（直连）
        let base_url = input
            .get("baseUrl")
            .or_else(|| input.get("base_url"))
            .and_then(|v| v.as_str())
            .unwrap_or("")
            .trim()
            .to_string();
        let api_key = input
            .get("apiKey")
            .or_else(|| input.get("api_key"))
            .and_then(|v| v.as_str())
            .unwrap_or("")
            .trim()
            .to_string();
        if base_url.is_empty() {
            return Err("baseUrl 不能为空".into());
        }
        if api_key.is_empty() {
            return Err("apiKey 不能为空".into());
        }
        let model = input
            .get("model")
            .and_then(|v| v.as_str())
            .unwrap_or("")
            .trim()
            .to_string();
        if model.is_empty() {
            return Err("model 不能为空".into());
        }
        (base_url, api_key, model)
    };

    // 可选覆盖模型（手动模式已从上面取，Provider 模式允许覆盖）
    if let Some(m) = input.get("model").and_then(|v| v.as_str()) {
        let m = m.trim();
        if !m.is_empty() {
            model = m.to_string();
        }
    }

    let codex = resolve_codex_path();

    let id = Uuid::new_v4().to_string();
    let temp_dir = std::env::temp_dir().join(format!("codex-launch-{id}"));
    std::fs::create_dir_all(&temp_dir).map_err(|e| format!("创建临时目录失败: {e}"))?;

    write_config(&temp_dir, &base_url, &model, &wire_api)?;
    let script_path = write_script(&temp_dir, &codex, &api_key, &model, &project_dir)?;
    open_terminal(&script_path)?;

    let session = LaunchSession {
        id: id.clone(),
        temp_dir: temp_dir.clone(),
        script_path,
        base_url: base_url.clone(),
        model: model.clone(),
        project_dir: project_dir.clone(),
        started_at: chrono::Local::now().format("%Y-%m-%d %H:%M:%S").to_string(),
    };
    state.sessions.lock().unwrap().insert(id.clone(), session);

    Ok(json!({
        "sessionId": id,
        "tempDir": temp_dir.to_string_lossy(),
        "baseUrl": base_url,
        "model": model,
        "projectDir": project_dir,
        "codex": codex,
        "note": "已打开系统终端运行 Codex CLI（独立 CODEX_HOME，直连中转站）",
    }))
}

/// POST /api/launcher/stop { sessionId }
pub fn stop(state: &LauncherState, session_id: &str) -> Result<Value, String> {
    let session = {
        let mut m = state.sessions.lock().unwrap();
        m.remove(session_id).ok_or_else(|| "找不到该启动会话".to_string())?
    };

    // 终止 codex 进程（TERM → 稍候 → KILL）
    if let Some(pid) = session.read_codex_pid() {
        let _ = std::process::Command::new("kill").arg("-TERM").arg(pid.to_string()).status();
        std::thread::sleep(std::time::Duration::from_millis(800));
        if is_alive(pid) {
            let _ = std::process::Command::new("kill").arg("-KILL").arg(pid.to_string()).status();
        }
    }

    // 清理临时目录（脚本 trap 兜底；这里强制清一次）
    let _ = std::fs::remove_dir_all(&session.temp_dir);

    Ok(json!({
        "sessionId": session_id,
        "cleaned": true,
        "tempDir": session.temp_dir.to_string_lossy(),
    }))
}

/// GET /api/launcher/status
pub fn status(state: &LauncherState) -> Value {
    let m = state.sessions.lock().unwrap();
    let sessions: Vec<Value> = m
        .values()
        .map(|s| {
            let pid = s.read_codex_pid();
            json!({
                "sessionId": s.id,
                "tempDir": s.temp_dir.to_string_lossy(),
                "baseUrl": s.base_url,
                "model": s.model,
                "projectDir": s.project_dir,
                "startedAt": s.started_at,
                "pid": pid,
                "alive": pid.map(is_alive).unwrap_or(false),
            })
        })
        .collect();
    json!({ "sessions": sessions })
}

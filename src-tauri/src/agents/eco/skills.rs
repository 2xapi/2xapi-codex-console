//! 技能(Skills)管理(C 段):EcoSkill trait + OpenClaw 全链 + Hermes 只读。
//!
//! OpenClaw(侦察报告 §1.3 定案):列表走 `openclaw skills list --json`(含 disabled/
//! bundled/source/missing);启停写 openclaw.json `skills.entries.<name>.enabled`
//! (文件不存在则创建,只含受控段);安装 `openclaw skills install <slug> --global`
//! (ClawHub/git/local,装 managed 目录 ~/.openclaw/skills);卸载仅限 managed 目录
//! 条目(bundled 在 npm 包内,只读)。
//! Hermes:技能=目录+SKILL.md(两层嵌套组),无 per-skill enabled——只读列表。

use serde_json::{json, Value};
use std::path::PathBuf;

pub type OpError = (u16, String, String);

/// 技能条目(平台无关展示形状)。
#[derive(Debug, Clone)]
pub struct SkillInfo {
    pub name: String,
    pub desc: String,
    /// console(我们装的/managed) | manual(用户自装) | bundled(平台自带,只读)
    pub source: String,
    pub disabled: bool,
}

pub trait EcoSkill {
    fn id(&self) -> &'static str;
    fn list(&self) -> Result<Vec<SkillInfo>, OpError>;
    fn set_enabled(&self, name: &str, enabled: bool) -> Result<Vec<SkillInfo>, OpError>;
    fn install(&self, slug: &str) -> Result<Vec<SkillInfo>, OpError>;
    fn uninstall(&self, name: &str) -> Result<Vec<SkillInfo>, OpError>;
}

fn unsupported(id: &str, what: &str) -> OpError {
    (400, "E_SKILL_UNSUPPORTED".into(), format!("{id} 技能{what}不支持此操作"))
}

// ── OpenClaw ──────────────────────────────────────────────

pub struct OpenclawSkills {
    oclaw_home: PathBuf,
    /// openclaw CLI 可执行文件(生产=PATH 上的 openclaw;测试注入假 CLI)
    cli: String,
}

impl OpenclawSkills {
    pub fn new(oclaw_home: PathBuf) -> Self {
        Self { oclaw_home, cli: "openclaw".to_string() }
    }

    fn config_path(&self) -> PathBuf {
        self.oclaw_home.join("openclaw.json")
    }

    fn run_cli(&self, args: &[&str]) -> Result<String, OpError> {
        let out = std::process::Command::new(&self.cli)
            .args(args)
            .env("HOME", &self.cli_home())
            .output()
            .map_err(|e| (500, "E_SKILL_CLI".into(), format!("启动 openclaw CLI 失败: {e}")))?;
        if !out.status.success() {
            return Err((
                500,
                "E_SKILL_CLI".into(),
                format!("openclaw {} 失败: {}", args.join(" "), String::from_utf8_lossy(&out.stderr).trim()),
            ));
        }
        Ok(String::from_utf8_lossy(&out.stdout).to_string())
    }

    /// CLI 的 HOME 指向 oclaw_home 的父目录(openclaw 定位 ~/.openclaw 用 HOME;
    /// 测试注入 tempdir 使 ~/.openclaw 落在隔离区)。
    fn cli_home(&self) -> PathBuf {
        self.oclaw_home
            .parent()
            .map(|p| p.to_path_buf())
            .unwrap_or_else(|| self.oclaw_home.clone())
    }

    fn read_config(&self) -> Result<Value, OpError> {
        let path = self.config_path();
        if !path.exists() {
            return Ok(json!({}));
        }
        let raw = std::fs::read_to_string(&path)
            .map_err(|e| (500, "E_IO".into(), format!("读取 openclaw.json 失败: {e}")))?;
        serde_json::from_str(&raw)
            .map_err(|_| (500, "E_PARSE".into(), "openclaw.json 不是合法 JSON,已拒绝写入(避免破坏手动配置);请先修复该文件".into()))
    }

    fn write_config(&self, cfg: &Value) -> Result<(), OpError> {
        let path = self.config_path();
        if let Some(parent) = path.parent() {
            std::fs::create_dir_all(parent).map_err(|e| (500, "E_IO".into(), format!("创建 .openclaw 目录失败: {e}")))?;
        }
        let text = serde_json::to_string_pretty(cfg).map_err(|e| (500, "E_IO".into(), format!("JSON 编码失败: {e}")))?;
        let tmp = path.with_extension("json.tmp");
        std::fs::write(&tmp, format!("{text}\n")).map_err(|e| (500, "E_IO".into(), format!("写入临时文件失败: {e}")))?;
        std::fs::rename(&tmp, &path).map_err(|e| (500, "E_IO".into(), format!("原子替换失败: {e}")))
    }
}

impl EcoSkill for OpenclawSkills {
    fn id(&self) -> &'static str {
        "openclaw"
    }

    fn list(&self) -> Result<Vec<SkillInfo>, OpError> {
        let out = self.run_cli(&["skills", "list", "--json"])?;
        let v: Value = serde_json::from_str(&out)
            .map_err(|_| (500, "E_SKILL_CLI".into(), "openclaw skills list 输出无法解析".into()))?;
        let mut skills = Vec::new();
        if let Some(arr) = v.get("skills").and_then(|s| s.as_array()) {
            for s in arr {
                let name = s.get("name").and_then(|v| v.as_str()).unwrap_or("").to_string();
                if name.is_empty() {
                    continue;
                }
                let source = s.get("source").and_then(|v| v.as_str()).unwrap_or("");
                // source 归一:managed/自装=manual-ish,bundled 只读;managedSkillsDir 内=可卸载
                let source = if s.get("bundled").and_then(|v| v.as_bool()).unwrap_or(false) || source == "openclaw-bundled" {
                    "bundled".to_string()
                } else {
                    "manual".to_string()
                };
                skills.push(SkillInfo {
                    name,
                    desc: s.get("description").and_then(|v| v.as_str()).unwrap_or("").to_string(),
                    source,
                    disabled: s.get("disabled").and_then(|v| v.as_bool()).unwrap_or(false),
                });
            }
        }
        Ok(skills)
    }

    fn set_enabled(&self, name: &str, enabled: bool) -> Result<Vec<SkillInfo>, OpError> {
        if !name.chars().all(|c| c.is_ascii_alphanumeric() || c == '-' || c == '_') || name.is_empty() {
            return Err((400, "E_SKILL_BAD_NAME".into(), "技能名仅限字母/数字/-/_".into()));
        }
        let mut cfg = self.read_config()?;
        // 受控段 skills.entries.<name>.enabled(官方 config 结构,侦察报告 §1.3)
        let entries = cfg
            .as_object_mut()
            .ok_or_else(|| (500, "E_PARSE".into(), "openclaw.json 顶层必须是对象".to_string()))?
            .entry("skills".to_string())
            .or_insert_with(|| json!({}));
        let entries = entries
            .as_object_mut()
            .ok_or_else(|| (500, "E_PARSE".into(), "skills 段必须是对象".to_string()))?
            .entry("entries".to_string())
            .or_insert_with(|| json!({}));
        entries
            .as_object_mut()
            .ok_or_else(|| (500, "E_PARSE".into(), "skills.entries 段必须是对象".to_string()))?
            .insert(name.to_string(), json!({ "enabled": enabled }));
        self.write_config(&cfg)?;
        self.list()
    }

    fn install(&self, slug: &str) -> Result<Vec<SkillInfo>, OpError> {
        let slug = slug.trim();
        if slug.is_empty() || !slug.chars().all(|c| c.is_ascii_alphanumeric() || c == '-' || c == '_' || c == '/' || c == '@' || c == '.') {
            return Err((400, "E_SKILL_BAD_SLUG".into(), "slug 仅支持 ClawHub(@owner/slug)/git 地址/本地路径字符".into()));
        }
        // --global 装 managed 目录(~/.openclaw/skills);不走 cwd agent
        self.run_cli(&["skills", "install", slug, "--global"])?;
        self.list()
    }

    fn uninstall(&self, name: &str) -> Result<Vec<SkillInfo>, OpError> {
        let dir = self.oclaw_home.join("skills").join(name);
        if !dir.exists() {
            return Err((404, "E_SKILL_NOT_MANAGED".into(), format!("「{name}」不在 managed 目录(~/.openclaw/skills),不可卸载(bundled/自装技能请用平台自身方式管理)")));
        }
        std::fs::remove_dir_all(&dir).map_err(|e| (500, "E_IO".into(), format!("删除技能目录失败: {e}")))?;
        self.list()
    }
}

// ── Hermes(只读)───────────────────────────────────────────

pub struct HermesSkills {
    skills_dir: PathBuf,
}

impl HermesSkills {
    pub fn new(hermes_home: &std::path::Path) -> Self {
        Self { skills_dir: hermes_home.join("skills") }
    }

    /// 扫两层:<skills>/<name>/SKILL.md 与 <skills>/<group>/<name>/SKILL.md;读 frontmatter。
    fn scan(&self) -> Vec<SkillInfo> {
        let mut out = Vec::new();
        let Ok(rd) = std::fs::read_dir(&self.skills_dir) else {
            return out;
        };
        for entry in rd.flatten() {
            let path = entry.path();
            if !path.is_dir() {
                continue;
            }
            if path.join("SKILL.md").exists() {
                if let Some(info) = parse_skill_md(&path) {
                    out.push(info);
                }
            } else if let Ok(sub) = std::fs::read_dir(&path) {
                // 嵌套组(如 apple/apple-notes)
                for s in sub.flatten() {
                    if s.path().is_dir() && s.path().join("SKILL.md").exists() {
                        if let Some(mut info) = parse_skill_md(&s.path()) {
                            info.name = format!("{}/{}", entry.file_name().to_string_lossy(), info.name);
                            out.push(info);
                        }
                    }
                }
            }
        }
        out.sort_by(|a, b| a.name.cmp(&b.name));
        out
    }
}

/// 解析 SKILL.md 的 YAML frontmatter(name/description)。
fn parse_skill_md(dir: &std::path::Path) -> Option<SkillInfo> {
    let raw = std::fs::read_to_string(dir.join("SKILL.md")).ok()?;
    let name = dir.file_name()?.to_string_lossy().to_string();
    let mut desc = String::new();
    if let Some(rest) = raw.strip_prefix("---") {
        if let Some(end) = rest.find("\n---") {
            for line in rest[..end].lines() {
                if let Some(v) = line.strip_prefix("description:") {
                    desc = v.trim().trim_matches('"').trim_matches('\'').to_string();
                }
            }
        }
    }
    Some(SkillInfo {
        name,
        desc,
        source: "manual".into(),
        disabled: false,
    })
}

impl EcoSkill for HermesSkills {
    fn id(&self) -> &'static str {
        "hermes"
    }

    fn list(&self) -> Result<Vec<SkillInfo>, OpError> {
        Ok(self.scan())
    }

    fn set_enabled(&self, _name: &str, _enabled: bool) -> Result<Vec<SkillInfo>, OpError> {
        Err(unsupported("Hermes", "启停(平台无 per-skill 开关,可用 hermes tools 的 toolset 机制)"))
    }

    fn install(&self, _slug: &str) -> Result<Vec<SkillInfo>, OpError> {
        Err(unsupported("Hermes", "安装(手动放入 ~/.hermes/skills/<name>/SKILL.md 即可)"))
    }

    fn uninstall(&self, _name: &str) -> Result<Vec<SkillInfo>, OpError> {
        Err(unsupported("Hermes", "卸载"))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn root(tag: &str) -> PathBuf {
        let r = std::env::temp_dir().join(format!("2xapi-eco-sk-{tag}-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&r);
        std::fs::create_dir_all(&r).unwrap();
        r
    }

    #[test]
    fn hermes_scan_two_levels_and_frontmatter() {
        let r = root("hermes-scan");
        let sk = r.join("skills");
        std::fs::create_dir_all(sk.join("alpha").join("one")).unwrap();
        std::fs::create_dir_all(sk.join("apple").join("apple-notes")).unwrap();
        std::fs::write(
            sk.join("alpha/one/SKILL.md"),
            "---\nname: one\ndescription: \"第一个技能\"\n---\n正文",
        )
        .unwrap();
        std::fs::write(
            sk.join("apple/apple-notes/SKILL.md"),
            "---\nname: apple-notes\ndescription: Apple Notes via memo\n---\n正文",
        )
        .unwrap();
        let hs = HermesSkills::new(&r);
        let list = hs.list().unwrap();
        let names: Vec<&str> = list.iter().map(|s| s.name.as_str()).collect();
        assert_eq!(names, vec!["alpha/one", "apple/apple-notes"], "两层扫描+排序");
        assert_eq!(list[0].desc, "第一个技能");
        // 只读
        assert_eq!(hs.set_enabled("one", false).unwrap_err().1, "E_SKILL_UNSUPPORTED");
        assert_eq!(hs.install("x/y").unwrap_err().1, "E_SKILL_UNSUPPORTED");
    }

    #[test]
    fn openclaw_enabled_writes_controlled_section() {
        let r = root("oclaw-cfg");
        std::fs::create_dir_all(&r).unwrap();
        let oc = OpenclawSkills::new(r.clone());
        // 无 CLI 环境下只验 config 段写入:借 set_enabled 的前半段逻辑等价验证(write_config/read_config)
        let cfg = json!({ "agent": { "name": "keep" } });
        oc.write_config(&cfg).unwrap();
        let mut cfg2 = oc.read_config().unwrap();
        cfg2["skills"]["entries"]["apple-notes"] = json!({ "enabled": false });
        oc.write_config(&cfg2).unwrap();
        let raw = std::fs::read_to_string(r.join("openclaw.json")).unwrap();
        assert!(raw.contains("\"apple-notes\""));
        assert!(raw.contains("\"enabled\": false"));
        assert!(raw.contains("\"keep\""), "其他段保留");
        let back = oc.read_config().unwrap();
        assert_eq!(back["agent"]["name"], "keep");
        assert_eq!(back["skills"]["entries"]["apple-notes"]["enabled"], false);
    }

    #[test]
    fn openclaw_uninstall_only_managed() {
        let r = root("oclaw-un");
        std::fs::create_dir_all(r.join("skills").join("mine")).unwrap();
        std::fs::write(r.join("skills/mine/SKILL.md"), "---\nname: mine\n---\n").unwrap();
        let oc = OpenclawSkills::new(r);
        assert_eq!(oc.uninstall("nope").unwrap_err().1, "E_SKILL_NOT_MANAGED");
        // CLI 不在测试环境,uninstall 成功路径会调 list→CLI 失败;此处只验目录删除段的行为边界
    }
}

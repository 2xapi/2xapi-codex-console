use serde_json::{json, Value};
use std::path::Path;

pub fn list(backup_dir: &Path) -> Vec<Value> {
    let mut entries = Vec::new();
    let files = match std::fs::read_dir(backup_dir) {
        Ok(f) => f,
        Err(_) => return entries,
    };
    for f in files.flatten() {
        let name = f.file_name().to_string_lossy().to_string();
        if name.ends_with(".manifest.json") {
            continue;
        }
        let metadata = match f.metadata() {
            Ok(m) => m,
            Err(_) => continue,
        };
        let modified = metadata
            .modified()
            .ok()
            .and_then(|t| t.duration_since(std::time::UNIX_EPOCH).ok())
            .map(|d| {
                chrono::DateTime::<chrono::Utc>::from_timestamp(d.as_secs() as i64, 0)
                    .map(|dt| dt.to_rfc3339())
                    .unwrap_or_default()
            })
            .unwrap_or_default();

        let (kind, purpose, title) = if name.starts_with("history-") {
            ("history", "", "历史会话修复前")
        } else if name.contains("manual") {
            ("config", "manual", "手动配置快照")
        } else {
            ("config", "pre-apply", "应用配置前")
        };

        entries.push(json!({
            "id": name,
            "kind": kind,
            "purpose": purpose,
            "title": title,
            "path": f.path().to_string_lossy(),
            "createdAt": modified,
        }));
    }
    entries.sort_by(|a, b| {
        b.get("createdAt")
            .and_then(|v| v.as_str())
            .unwrap_or("")
            .cmp(a.get("createdAt").and_then(|v| v.as_str()).unwrap_or(""))
    });
    entries
}

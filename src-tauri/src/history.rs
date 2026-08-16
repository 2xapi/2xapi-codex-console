use serde_json::{json, Value};
use std::path::Path;

pub fn inspect(codex_home: &Path) -> Value {
    let sessions_dir = codex_home.join("sessions");
    let total = std::fs::read_dir(&sessions_dir)
        .map(|e| e.count())
        .unwrap_or(0);

    let rollout_dir = codex_home.join("rollouts");
    let rollout_total = std::fs::read_dir(&rollout_dir)
        .map(|e| e.count())
        .unwrap_or(0);

    json!({
        "ok": true,
        "state": { "total": total, "rolloutTotal": rollout_total }
    })
}

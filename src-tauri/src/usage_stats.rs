//! 用量仪表盘后端(竞品对标吸收项;2026-08-17 后端开发部,用户拍板)。
//!
//! - 落盘:网关每请求 append 一行 JSONL 到 `{codex_home}/usage-stats.jsonl`
//!   (ts/provider/key 脱敏/route/line/延迟/ok),不落明文 Key(安全约定与 usage 块一致)。
//! - 聚合:GET /api/usage-stats 全量读文件,按 provider 聚合
//!   {count, p50, p90, ok_rate, last_ts}——P50/P90 = 性能自然基准(调研部建议路线)。
//! - 规模:桌面应用单机流量小,全量读可接受;量大再上 SQLite/索引(ponytail:)。

use serde::Serialize;
use std::path::Path;

#[derive(Debug, Clone, Serialize)]
pub struct ReqLog {
    pub ts: i64,
    pub provider_id: String,
    pub provider_name: String,
    pub key_masked: String,
    /// codex | anthropic | gemini | images。
    pub route: String,
    /// 线路 id;直连 = "direct"。
    pub line: String,
    pub latency_ms: u64,
    pub ok: bool,
}

fn log_path(codex_home: &Path) -> std::path::PathBuf {
    codex_home.join("usage-stats.jsonl")
}

/// Key 脱敏:前 3 + … + 尾 4;过短只留省略号(与 server usage 块同形态,不落明文)。
pub fn mask_key(key: &str) -> String {
    let n = key.chars().count();
    if n >= 8 {
        let head: String = key.chars().take(3).collect();
        let tail: String = key.chars().skip(n - 4).collect();
        format!("{head}…{tail}")
    } else {
        "…".to_string()
    }
}

/// 追加一行请求日志(尽力而为,失败不阻塞网关)。
pub fn log_request(codex_home: &Path, r: &ReqLog) {
    use std::io::Write;
    let Ok(raw) = serde_json::to_string(r) else {
        return;
    };
    let path = log_path(codex_home);
    std::fs::create_dir_all(codex_home).ok();
    if let Ok(mut f) = std::fs::OpenOptions::new()
        .create(true)
        .append(true)
        .open(&path)
    {
        let _ = writeln!(f, "{raw}");
    }
}

/// 全量读并聚合(按 provider_id 分组);无数据 → 空。
pub fn summary(codex_home: &Path) -> serde_json::Value {
    use serde_json::Value;
    let raw = match std::fs::read_to_string(log_path(codex_home)) {
        Ok(r) => r,
        Err(_) => return serde_json::json!({ "providers": [] }),
    };
    let mut by_provider: std::collections::BTreeMap<String, Vec<Value>> = Default::default();
    for line in raw.lines() {
        if let Ok(v) = serde_json::from_str::<Value>(line) {
            if let Some(pid) = v.get("provider_id").and_then(|x| x.as_str()) {
                by_provider.entry(pid.to_string()).or_default().push(v);
            }
        }
    }
    let providers: Vec<Value> = by_provider
        .into_iter()
        .map(|(pid, rows)| {
            let name = rows
                .iter()
                .find_map(|r| r.get("provider_name").and_then(|x| x.as_str()))
                .unwrap_or(&pid)
                .to_string();
            let mut lats: Vec<u64> = rows
                .iter()
                .filter_map(|r| r.get("latency_ms").and_then(|x| x.as_u64()))
                .collect();
            lats.sort_unstable();
            let p = |q: f64| {
                lats.get(((lats.len() as f64) * q).floor() as usize)
                    .copied()
                    .unwrap_or(0)
            };
            let ok = rows
                .iter()
                .filter(|r| r.get("ok").and_then(|x| x.as_bool()).unwrap_or(false))
                .count();
            let routes: Vec<&str> = rows
                .iter()
                .filter_map(|r| r.get("route").and_then(|x| x.as_str()))
                .collect();
            let mut routes = routes;
            routes.sort_unstable();
            routes.dedup();
            let last_ts = rows
                .iter()
                .filter_map(|r| r.get("ts").and_then(|x| x.as_i64()))
                .max()
                .unwrap_or(0);
            serde_json::json!({
                "providerId": pid,
                "providerName": name,
                "count": rows.len(),
                "p50Ms": p(0.50),
                "p90Ms": p(0.90),
                "okRate": if rows.is_empty() { 0.0 } else { ok as f64 / rows.len() as f64 },
                "lastTs": last_ts,
                "routes": routes,
            })
        })
        .collect();
    serde_json::json!({ "providers": providers })
}

#[cfg(test)]
mod tests {
    use super::*;

    fn tmp(tag: &str) -> std::path::PathBuf {
        let p = std::env::temp_dir().join(format!("2xapi-us-{tag}-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&p);
        std::fs::create_dir_all(&p).unwrap();
        p
    }

    #[test]
    fn mask_hides_middle() {
        assert_eq!(mask_key("sk-abcdefghijkl"), "sk-…ijkl");
        assert_eq!(mask_key("short"), "…");
    }

    #[test]
    fn log_and_summarize_per_provider_p50_p90() {
        let home = tmp("sum");
        for (i, ok, ms) in [(1, true, 10), (2, true, 20), (3, true, 30), (4, false, 40)] {
            log_request(
                &home,
                &ReqLog {
                    ts: i,
                    provider_id: "p1".into(),
                    provider_name: "P1".into(),
                    key_masked: mask_key("sk-abcdefghijkl"),
                    route: "codex".into(),
                    line: "direct".into(),
                    latency_ms: ms,
                    ok,
                },
            );
        }
        let v = summary(&home);
        let prov = &v["providers"][0];
        assert_eq!(prov["providerId"], "p1");
        assert_eq!(prov["count"], 4);
        // 4 条:0.5*4=2 → 索引 2 = 30;0.9*4=3.6 → floor 3 = 40
        assert_eq!(prov["p50Ms"], 30);
        assert_eq!(prov["p90Ms"], 40);
        assert!((prov["okRate"].as_f64().unwrap() - 0.75).abs() < 1e-9);
        assert_eq!(prov["lastTs"], 4);
        assert_eq!(prov["routes"][0], "codex");
        // 无数据 → 空
        let empty_home = tmp("empty");
        assert_eq!(
            summary(&empty_home)["providers"].as_array().unwrap().len(),
            0
        );
        let _ = std::fs::remove_dir_all(&home);
        let _ = std::fs::remove_dir_all(&empty_home);
    }
}

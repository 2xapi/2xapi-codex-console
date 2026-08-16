//! 历史会话管理(阶段 3,开发任务书 §四)。
//!
//! 读 `~/.codex/sqlite/codex-dev.db` 的 `local_thread_catalog`(真实 schema,探索笔记见交接日志):
//! 列表(updatedAt 倒序,分页,provider 过滤);repair(对账 rollout 文件与 db,补缺失/归属);
//! autoRepairBeforeHost 设置(host 前自动跑轻量 repair)。
//!
//! 安全约定:任何写操作(repair)前先整库备份到 backup_dir;只读操作永不改 db。
//! 本期只上「列表+修复+设置」,删除第二版(带备份恢复验证后再放)。

use rusqlite::{Connection, OpenFlags};
use serde_json::{json, Value};
use std::path::{Path, PathBuf};

/// 探测真实 db 路径:优先 sqlite/codex-dev.db,回退 state_5.sqlite,再回退旧 sessions.sqlite。
fn probe_db_path(codex_home: &Path) -> Option<PathBuf> {
    let sqlite_dir = codex_home.join("sqlite");
    for name in ["codex-dev.db", "state_5.sqlite"] {
        let p = sqlite_dir.join(name);
        if p.exists() {
            return Some(p);
        }
    }
    let legacy = codex_home.join("sessions.sqlite");
    if legacy.exists() {
        return Some(legacy);
    }
    None
}

/// catalog 主表的列是否存在(新/旧 schema 自适应)。
fn has_column(db: &Connection, table: &str, column: &str) -> bool {
    let sql = format!("PRAGMA table_info({table})");
    if let Ok(mut stmt) = db.prepare(&sql) {
        if let Ok(rows) = stmt.query_map([], |r| r.get::<_, String>(1)) {
            for col in rows.flatten() {
                if col == column {
                    return true;
                }
            }
        }
    }
    false
}

/// 单条会话(契约 items 项)。
#[derive(Debug, Clone)]
pub struct SessionItem {
    pub id: String,
    pub title: String,
    pub cwd: String,
    pub provider_tag: String,
    pub updated_at_ms: i64,
    pub archived: bool,
    /// 对账缺失标记(repair 写 missing_candidate):API 输出供前端展示「缺失会话」用。
    pub missing: bool,
}

/// GET /api/sessions?page&size&provider → {total, items}
/// 按 updated_at 倒序;providerTag 从 catalog.model_provider(推不出标 "unknown")。
pub fn list_sessions(codex_home: &Path, page: usize, size: usize, provider: &str) -> Value {
    let Some(db_path) = probe_db_path(codex_home) else {
        return json!({ "total": 0, "items": [], "db": null });
    };
    let conn = match Connection::open_with_flags(&db_path, OpenFlags::SQLITE_OPEN_READ_ONLY) {
        Ok(c) => c,
        Err(_) => {
            return json!({ "total": 0, "items": [], "db": db_path.to_string_lossy(), "error": "打开数据库失败" })
        }
    };

    // 主表探测:新 schema local_thread_catalog / 旧 schema threads
    let catalog = "local_thread_catalog";
    let table = if has_column(&conn, catalog, "display_title") {
        catalog
    } else {
        "threads"
    };
    let cols = if table == catalog {
        // 新 schema:主键 (host_id, thread_id),无独立 id 列
        (
            "thread_id",
            "display_title",
            "cwd",
            "model_provider",
            "source_updated_at",
            "missing_candidate",
        )
    } else {
        (
            "id",
            "title",
            "cwd",
            "model_provider",
            "updated_at_ms",
            "archived",
        )
    };
    let archived_expr = if table == catalog {
        "0"
    } else {
        "COALESCE(archived,0)"
    };
    let missing_expr = if table == catalog {
        "COALESCE(missing_candidate,0)"
    } else {
        "0"
    };
    let updated_expr = if table == catalog {
        // source_updated_at 是 REAL 秒 → 毫秒
        "CAST(source_updated_at * 1000 AS INTEGER)"
    } else if has_column(&conn, table, "updated_at_ms") {
        "updated_at_ms"
    } else {
        "CAST(updated_at * 1000 AS INTEGER)"
    };

    let where_provider = if provider.is_empty() {
        String::new()
    } else {
        format!(" AND {} = :provider", cols.3)
    };

    // total
    let total_sql = format!("SELECT COUNT(*) FROM {table} WHERE 1=1{where_provider}");
    let total: i64 = if provider.is_empty() {
        conn.query_row(&total_sql, [], |r| r.get(0)).unwrap_or(0)
    } else {
        conn.query_row(&total_sql, rusqlite::params![provider], |r| r.get(0))
            .unwrap_or(0)
    };

    // 分页列表(updatedAt 倒序;同值按 id 倒序稳定)
    let page = page.max(1);
    let size = size.clamp(1, 100);
    let offset = (page - 1) * size;
    let list_sql = format!(
        "SELECT {col_id}, {col_title}, {col_cwd}, {col_provider}, {updated_expr}, {archived_expr}, {missing_expr}
         FROM {table}
         WHERE 1=1{where_provider}
         ORDER BY {updated_expr} DESC, {col_id} DESC
         LIMIT {size} OFFSET {offset}",
        col_id = cols.0, col_title = cols.1, col_cwd = cols.2, col_provider = cols.3,
        updated_expr = updated_expr, archived_expr = archived_expr, missing_expr = missing_expr,
    );

    let mut items = Vec::new();
    if let Ok(mut stmt) = conn.prepare(&list_sql) {
        let query = |r: &rusqlite::Row| {
            Ok((
                r.get::<_, String>(0)?,
                r.get::<_, Option<String>>(1)?,
                r.get::<_, Option<String>>(2)?,
                r.get::<_, Option<String>>(3)?,
                r.get::<_, i64>(4)?,
                r.get::<_, i64>(5)?,
                r.get::<_, i64>(6)?,
            ))
        };
        let rows = if provider.is_empty() {
            stmt.query_map([], query)
        } else {
            stmt.query_map(rusqlite::params![provider], query)
        };
        if let Ok(rows) = rows {
            for row in rows.flatten() {
                let (id, title, cwd, provider_tag, updated_ms, archived, missing) = row;
                items.push(SessionItem {
                    id,
                    title: title.unwrap_or_default(),
                    cwd: cwd.unwrap_or_default(),
                    provider_tag: provider_tag
                        .filter(|s| !s.is_empty())
                        .unwrap_or_else(|| "unknown".into()),
                    updated_at_ms: updated_ms,
                    archived: archived != 0,
                    missing: missing != 0,
                });
            }
        }
    }

    json!({
        "total": total,
        "items": items.iter().map(|s| json!({
            "id": s.id, "title": s.title, "cwd": s.cwd,
            "providerTag": s.provider_tag, "updatedAt": s.updated_at_ms, "archived": s.archived,
            "missing": s.missing,
        })).collect::<Vec<_>>(),
        "db": db_path.to_string_lossy(),
    })
}

// ── repair ─────────────────────────────────────────────────

/// POST /api/sessions/repair → {fixed, scanned}
/// 对账 rollout 文件与 db 记录:db 指向的 rollout 文件存在 → 归属可确认;缺失 → 记 missing;
/// 修复 missing_candidate 标记。写操作前整库备份。
pub fn repair_sessions(codex_home: &Path, backup_dir: &Path) -> Value {
    let Some(db_path) = probe_db_path(codex_home) else {
        return json!({ "fixed": 0, "scanned": 0, "error": "未找到会话数据库" });
    };

    // 写前整库备份(三保险)
    let _ = std::fs::create_dir_all(backup_dir);
    let backup_name = format!(
        "sessions-{}.db",
        chrono::Local::now().format("%Y%m%d-%H%M%S")
    );
    let backup_path = backup_dir.join(&backup_name);
    if let Ok(data) = std::fs::read(&db_path) {
        let _ = std::fs::write(&backup_path, &data);
    }

    let conn = match Connection::open(&db_path) {
        Ok(c) => c,
        Err(e) => {
            return json!({ "fixed": 0, "scanned": 0, "error": format!("打开数据库失败: {e}") })
        }
    };
    let catalog = "local_thread_catalog";
    if !has_column(&conn, catalog, "source_detail") {
        return json!({ "fixed": 0, "scanned": 0, "error": "不支持的 schema(缺 source_detail)" });
    }

    let mut scanned = 0u32;
    let mut fixed = 0u32;

    // 遍历全部 catalog 行,对账 rollout 文件存在性
    let sql = format!("SELECT thread_id, source_detail, missing_candidate FROM {catalog}");
    let ids_to_fix: Vec<(String, i64)> = {
        let mut stmt = match conn.prepare(&sql) {
            Ok(s) => s,
            Err(_) => return json!({ "fixed": 0, "scanned": 0, "error": "查询失败" }),
        };
        let Ok(rows) = stmt.query_map([], |r| {
            Ok((
                r.get::<_, String>(0)?,
                r.get::<_, Option<String>>(1)?,
                r.get::<_, Option<i64>>(2)?,
            ))
        }) else {
            return json!({ "fixed": 0, "scanned": 0, "error": "查询失败" });
        };
        let mut out = Vec::new();
        for row in rows.flatten() {
            scanned += 1;
            let (tid, detail, current_missing) = row;
            let exists = detail
                .as_deref()
                .map(|p| {
                    let p = p.trim_start_matches("\\?/"); // 本机实测 config 里有 \?/ 前缀,兜底
                    Path::new(p).exists() || {
                        // 相对路径 → 相对 codex_home 找
                        let abs = codex_home.join(p.trim_start_matches('/'));
                        abs.exists() || codex_home.join("..").join(p).exists()
                    }
                })
                .unwrap_or(false);
            let want_missing = if exists { 0 } else { 1 };
            if (current_missing.unwrap_or(0) != want_missing)
                || (want_missing == 0 && current_missing.is_none())
            {
                out.push((tid, want_missing));
            }
        }
        out
    };

    // 落库修复(每行 UPDATE)
    for (tid, want_missing) in &ids_to_fix {
        let upd = format!("UPDATE {catalog} SET missing_candidate = ?1 WHERE thread_id = ?2");
        if conn
            .execute(&upd, rusqlite::params![want_missing, tid])
            .is_ok()
        {
            fixed += 1;
        }
    }

    json!({ "fixed": fixed, "scanned": scanned })
}

// ── autoRepairBeforeHost 设置 ───────────────────────────────

fn settings_path(codex_home: &Path) -> PathBuf {
    codex_home.join("2xapi-settings.json")
}

fn read_settings(codex_home: &Path) -> Value {
    std::fs::read_to_string(settings_path(codex_home))
        .ok()
        .and_then(|raw| serde_json::from_str(&raw).ok())
        .unwrap_or_else(|| json!({}))
}

fn write_settings(codex_home: &Path, v: &Value) {
    let _ = std::fs::write(
        settings_path(codex_home),
        serde_json::to_string_pretty(v).unwrap_or_default(),
    );
}

/// GET /api/sessions/settings → {autoRepairBeforeHost}
pub fn get_settings(codex_home: &Path) -> Value {
    let s = read_settings(codex_home);
    json!({
        "autoRepairBeforeHost": s.get("autoRepairBeforeHost").and_then(|v| v.as_bool()).unwrap_or(true),
    })
}

/// POST /api/sessions/settings {autoRepairBeforeHost}
pub fn set_settings(codex_home: &Path, auto_repair: bool) -> Value {
    let mut s = read_settings(codex_home);
    if let Some(o) = s.as_object_mut() {
        o.insert("autoRepairBeforeHost".into(), json!(auto_repair));
    }
    write_settings(codex_home, &s);
    json!({ "autoRepairBeforeHost": auto_repair })
}

/// host 前自动 repair(轻量:只对账,不重建)。供 desktop.rs host 调用。
pub fn auto_repair_if_enabled(codex_home: &Path, backup_dir: &Path) {
    if get_settings(codex_home)
        .get("autoRepairBeforeHost")
        .and_then(|v| v.as_bool())
        .unwrap_or(true)
    {
        let _ = repair_sessions(codex_home, backup_dir);
    }
}

// ── 单测 ─────────────────────────────────────────────────────

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::atomic::{AtomicU64, Ordering};

    static N: AtomicU64 = AtomicU64::new(0);

    fn sandbox(label: &str) -> (PathBuf, PathBuf) {
        let n = N.fetch_add(1, Ordering::SeqCst);
        let root =
            std::env::temp_dir().join(format!("2xapi-stage3-{label}-{}-{n}", std::process::id()));
        let _ = std::fs::remove_dir_all(&root);
        let home = root.join("codex");
        std::fs::create_dir_all(home.join("sqlite")).unwrap();
        (home, root.join("backups"))
    }

    /// 构造一个与真实 catalog 同 schema 的内存 db(写文件)。
    fn make_catalog_db(root: &Path, rows: &[(&str, &str, &str, &str, i64)]) -> PathBuf {
        let db_path = root.join("sqlite/codex-dev.db");
        let conn = Connection::open(&db_path).unwrap();
        conn.execute_batch(
            "CREATE TABLE local_thread_catalog (
                host_id TEXT NOT NULL, thread_id TEXT NOT NULL, display_title TEXT NOT NULL,
                source_created_at REAL NOT NULL, source_updated_at REAL NOT NULL, cwd TEXT NOT NULL,
                source_kind TEXT NOT NULL, source_detail TEXT, model_provider TEXT NOT NULL,
                git_branch TEXT, observation_sequence INTEGER NOT NULL,
                missing_candidate INTEGER NOT NULL DEFAULT 0,
                PRIMARY KEY (host_id, thread_id));",
        )
        .unwrap();
        for (tid, title, provider, detail, updated_sec) in rows {
            conn.execute(
                "INSERT INTO local_thread_catalog
                 (host_id, thread_id, display_title, source_created_at, source_updated_at, cwd, source_kind, source_detail, model_provider, git_branch, observation_sequence)
                 VALUES ('local', ?1, ?2, 0, ?5, '/tmp/proj', 'vscode', ?4, ?3, NULL, 0)",
                rusqlite::params![tid, title, provider, detail, *updated_sec as f64],
            )
            .unwrap();
        }
        db_path
    }

    #[test]
    fn list_sessions_paginated_and_provider_filtered() {
        let (root, _bk) = sandbox("list");
        // 两个 provider、不同时间
        let rollout = root.join("r.jsonl");
        let _ = std::fs::write(&rollout, "{}");
        make_catalog_db(
            &root,
            &[
                ("t1", "会话甲", "custom", rollout.to_str().unwrap(), 1000),
                ("t2", "会话乙", "2xapi", "", 3000),
                ("t3", "会话丙", "custom", "", 2000),
            ],
        );
        let home = &root;

        eprintln!(
            "[DBG] db_path exists: {}",
            home.join("sqlite/codex-dev.db").exists()
        );
        let r = list_sessions(home, 1, 10, "");
        eprintln!("[DBG] list result: {}", r);
        assert_eq!(r["total"], 3);
        let items = r["items"].as_array().unwrap();
        assert_eq!(items.len(), 3);
        assert_eq!(items[0]["id"], "t2", "应按 updatedAt 倒序");
        assert_eq!(items[0]["providerTag"], "2xapi");
        assert_eq!(items[1]["title"], "会话丙");

        // provider 过滤
        let r2 = list_sessions(home, 1, 10, "custom");
        assert_eq!(r2["total"], 2);
        let _ = std::fs::remove_dir_all(&root);
    }

    #[test]
    fn repair_marks_missing_and_clears() {
        let (root, bk) = sandbox("repair");
        let good = root.join("good.jsonl");
        let _ = std::fs::write(&good, "{}");
        // t1 文件存在(missing 应为 0);t2 文件不存在(missing 应为 1);t3 文件存在但标记了 1(应清 0)
        make_catalog_db(
            &root,
            &[
                ("t1", "甲", "custom", good.to_str().unwrap(), 100),
                ("t2", "乙", "custom", "/nonexistent/x.jsonl", 200),
                ("t3", "丙", "custom", good.to_str().unwrap(), 300),
            ],
        );
        // t3 手工标 missing=1
        let conn = Connection::open(root.join("sqlite/codex-dev.db")).unwrap();
        conn.execute(
            "UPDATE local_thread_catalog SET missing_candidate=1 WHERE thread_id='t3'",
            [],
        )
        .unwrap();
        drop(conn);

        let r = repair_sessions(&root, &bk);
        assert_eq!(r["scanned"], 3, "应扫描全部");
        assert_eq!(
            r["fixed"], 2,
            "t2 标 missing + t3 清 missing 共 2 行修正;t1 本正确不动"
        );
        // 备份已建
        assert!(
            std::fs::read_dir(&bk).unwrap().next().is_some(),
            "写前应有整库备份"
        );

        // 验证落库
        let conn = Connection::open(root.join("sqlite/codex-dev.db")).unwrap();
        let get = |tid: &str| -> i64 {
            conn.query_row(
                "SELECT missing_candidate FROM local_thread_catalog WHERE thread_id=?1",
                [tid],
                |r| r.get(0),
            )
            .unwrap()
        };
        assert_eq!(get("t1"), 0);
        assert_eq!(get("t2"), 1, "缺失 rollout 应标记 missing");
        assert_eq!(get("t3"), 0, "存在文件应清除 missing");
        let _ = std::fs::remove_dir_all(&root);
    }

    #[test]
    fn settings_default_on_and_roundtrip() {
        let (root, _bk) = sandbox("settings");
        assert!(
            get_settings(&root)["autoRepairBeforeHost"]
                .as_bool()
                .unwrap(),
            "默认开"
        );
        set_settings(&root, false);
        assert!(!get_settings(&root)["autoRepairBeforeHost"]
            .as_bool()
            .unwrap());
        let _ = std::fs::remove_dir_all(&root);
    }
}

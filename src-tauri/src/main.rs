// Windows GUI 程序不弹控制台窗口(release 生效,debug 保留控制台便于排障)
#![cfg_attr(not(debug_assertions), windows_subsystem = "windows")]

mod server;
// M8:launcher 模块根为 launcher/mod.rs。显式 #[path] 是为了兼容旧 launcher.rs
// 还未删除的过渡期(rustc 见到两者并存会报 ambiguous);删除旧文件后此写法同样有效。
mod acclines;
mod agents;
mod auth;
mod backups;
mod claude_sessions;
mod config;
mod desktop;
mod diagnose;
mod gateway;
mod gateway_conv;
mod gateway_gemini_conv;
mod grok_config;
mod history;
#[path = "launcher/mod.rs"]
mod launcher;
mod nodecreds;
mod probe;
mod providers;
mod sessions;

use std::net::TcpListener;
use tauri::{Manager, WebviewWindowBuilder};

fn codex_home() -> std::path::PathBuf {
    // Windows 无 HOME 环境变量 → 回退 USERPROFILE,否则 home 解析为空导致 .codex 写错位置
    let home = std::env::var("HOME")
        .ok()
        .filter(|s| !s.trim().is_empty())
        .or_else(|| std::env::var("USERPROFILE").ok())
        .unwrap_or_default();
    let h = std::env::var("CODEX_HOME").unwrap_or_else(|_| format!("{}/.codex", home));
    std::path::PathBuf::from(h)
}

fn main() {
    let codex_home = codex_home();
    let config_path = codex_home.join("config.toml");
    let backup_dir = codex_home.join("config-backups");
    let providers_path = codex_home.join("providers.json");

    std::fs::create_dir_all(&backup_dir).ok();

    // 网关固定监听 127.0.0.1:8787（契约要求：Codex 的 config.toml 里 custom.base_url 指向此地址）
    let listener = TcpListener::bind("127.0.0.1:8787")
        .expect("无法绑定 127.0.0.1:8787（端口可能被占用，请先释放后重试）");
    // tokio::from_std 要求非阻塞 socket（否则 panic，tokio#7172）
    listener.set_nonblocking(true).expect("set_nonblocking");
    let app_url = "http://127.0.0.1:8787".to_string();

    // M8:启动器状态 → 先清扫崩溃残留(只清带 launcher.json 标记的目录),再起后台退出监控
    let launcher_state = std::sync::Arc::new(launcher::LauncherState::default());
    launcher::sweep_orphans();
    launcher::spawn_monitor(launcher_state.clone());

    // 阶段 4:加速线路装配——启动即加载线路填入健康状态;accel 配置从 2xapi-settings.json 读入
    let lines = crate::acclines::load_lines(&codex_home);
    let health_state = std::sync::Arc::new(crate::acclines::HealthState::new(lines.lines));
    let accel_state =
        std::sync::Arc::new(std::sync::Mutex::new(server::load_accel_cfg(&codex_home)));
    // 星图 任务 B:每账号节点凭证表(兼容迁移旧单对象 → legacy)
    let nodecreds_store =
        std::sync::Arc::new(std::sync::RwLock::new(nodecreds::load_store(&codex_home)));

    let state = server::AppState {
        config_path: config_path.clone(),
        backup_dir: backup_dir.clone(),
        providers_path: providers_path.clone(),
        codex_home: codex_home.clone(),
        // workbuddy 双载体(~/.codebuddy 与 ~/.workbuddy)的公共根;测试传 tempdir(server/gateway 测试态)
        wb_home: std::path::PathBuf::from(
            std::env::var("HOME")
                .ok()
                .filter(|s| !s.trim().is_empty())
                .or_else(|| std::env::var("USERPROFILE").ok())
                .unwrap_or_default(),
        ),
        hermes_home: crate::agents::hermes::hermes_home(),
        // gemini 载体根(~/.gemini 所在);测试传 tempdir
        gem_home: std::path::PathBuf::from(
            std::env::var("HOME")
                .ok()
                .filter(|s| !s.trim().is_empty())
                .or_else(|| std::env::var("USERPROFILE").ok())
                .unwrap_or_default(),
        ),
        grok_home: crate::grok_config::default_grok_home(),
        oc_home: std::path::PathBuf::from(
            std::env::var("HOME")
                .ok()
                .filter(|s| !s.trim().is_empty())
                .or_else(|| std::env::var("USERPROFILE").ok())
                .unwrap_or_default(),
        ),
        oclaw_home: {
            let home = std::env::var("HOME")
                .ok()
                .filter(|s| !s.trim().is_empty())
                .or_else(|| std::env::var("USERPROFILE").ok())
                .unwrap_or_default();
            std::path::PathBuf::from(home).join(".openclaw")
        },
        // Claude Desktop(macOS):~/Library/Application Support(Claude/ 与 Claude-3p/ 的父;
        // Windows 的 APPDATA 路径未实证,首版 macOS 为主)
        cd_home: {
            let home = std::env::var("HOME")
                .ok()
                .filter(|s| !s.trim().is_empty())
                .or_else(|| std::env::var("USERPROFILE").ok())
                .unwrap_or_default();
            std::path::PathBuf::from(home)
                .join("Library")
                .join("Application Support")
        },
        // Cursor 生态管理(A 段):~/.cursor 所在根(eco adapter join(".cursor/mcp.json"))
        cursor_home: std::path::PathBuf::from(
            std::env::var("HOME")
                .ok()
                .filter(|s| !s.trim().is_empty())
                .or_else(|| std::env::var("USERPROFILE").ok())
                .unwrap_or_default(),
        ),
        launcher: launcher_state,
        health: health_state.clone(),
        accel: accel_state,
        nodecreds: nodecreds_store,
    };

    let router = server::build_router(state);

    // Start HTTP server in a dedicated thread with its own tokio runtime
    std::thread::spawn(move || {
        let rt = tokio::runtime::Runtime::new().unwrap();
        rt.block_on(async move {
            let listener = tokio::net::TcpListener::from_std(listener).unwrap();
            // 阶段 4:后台健康探测循环(每 30s 快照 HealthState.lines 探测;线路可经 set_lines 更新)。
            // spawn_health_loop 内部自行 tokio::spawn,此处直接调用即可。
            crate::acclines::spawn_health_loop(
                health_state.clone(),
                std::time::Duration::from_secs(30),
            );
            // 任务书 §五:远程线路表刷新(启动即拉 + 每 60min;accel-remote.json
            // 未配置时静默跳过,不影响内置/缓存表)。
            crate::acclines::spawn_refresh_loop(
                health_state.clone(),
                codex_home.clone(),
                std::time::Duration::from_secs(3600),
            );
            axum::serve(listener, router).await.unwrap();
        });
    });

    tauri::Builder::default()
        .setup(move |app| {
            let window = WebviewWindowBuilder::new(
                app,
                "main",
                tauri::WebviewUrl::External(app_url.parse().unwrap()),
            )
            .title("2xapi Codex Console")
            .inner_size(1000.0, 720.0)
            .min_inner_size(800.0, 600.0)
            .build()?;

            // 关窗口 → 隐藏而非退出（保持网关 8787 常驻；从托盘重新显示/退出）
            let wh = window.clone();
            window.on_window_event(move |event| {
                if let tauri::WindowEvent::CloseRequested { api, .. } = event {
                    api.prevent_close();
                    let _ = wh.hide();
                }
            });

            // 托盘菜单
            use tauri::menu::{Menu, MenuItem};
            use tauri::tray::TrayIconBuilder;
            let show = MenuItem::with_id(app, "show", "显示窗口", true, None::<&str>)?;
            let quit = MenuItem::with_id(app, "quit", "退出（关闭网关）", true, None::<&str>)?;
            let menu = Menu::with_items(app, &[&show, &quit])?;
            let _tray = TrayIconBuilder::with_id("main-tray")
                .icon(app.default_window_icon().cloned().unwrap())
                .menu(&menu)
                .tooltip("2xapi Codex Console（关窗口不退出，网关保持运行）")
                .on_menu_event(|app, event| match event.id.as_ref() {
                    "show" => {
                        if let Some(w) = app.get_webview_window("main") {
                            let _ = w.show();
                            let _ = w.set_focus();
                        }
                    }
                    "quit" => app.exit(0),
                    _ => {}
                })
                .build(app)?;

            Ok(())
        })
        .run(tauri::generate_context!())
        .expect("error while running tauri application");
}

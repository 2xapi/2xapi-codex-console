//! Grok Build adapter(接线层):泛化路由分支 ↔ grok_config 配置引擎。
//! 引擎(grok_config.rs)已含语法校验/官方态识别/凭据解析/托管写入与受控还原;
//! 本层只做 provider 解析(引擎吃 &Provider)与泛化路由形态对接。
//! 前端世界批次未交付(frontend_ready=false),state 契约先按引擎原生形态,
//! 前端批次落地时再对齐 UI。

use serde_json::Value;
use std::path::{Path, PathBuf};

pub type OpError = (u16, String, String);

/// live 配置路径:`<grok_home>/config.toml`(grok_home 由 AppState 注入;生产=默认 ~/.grok,测试=tempdir)。
pub fn config_path(grok_home: &Path) -> PathBuf {
    grok_home.join("config.toml")
}

/// 托管态(detect_hosting 原生形态)。
pub fn state(grok_home: &Path) -> Value {
    crate::grok_config::detect_hosting(&config_path(grok_home))
}

fn find_provider(
    providers_path: &Path,
    provider_id: &str,
) -> Result<crate::providers::Provider, OpError> {
    let data = crate::providers::load(providers_path);
    data.providers
        .into_iter()
        .find(|p| p.id == provider_id)
        .ok_or_else(|| (404, "E_NO_PROVIDER".into(), format!("供应商不存在: {provider_id}")))
}

pub fn host(
    grok_home: &Path,
    backup_dir: &Path,
    providers_path: &Path,
    provider_id: &str,
    way: &str,
) -> Result<Value, OpError> {
    let provider = find_provider(providers_path, provider_id)?;
    crate::grok_config::host(&config_path(grok_home), backup_dir, &provider, way)
}

pub fn unhost(grok_home: &Path, backup_dir: &Path) -> Result<Value, OpError> {
    crate::grok_config::unhost(&config_path(grok_home), backup_dir)
}

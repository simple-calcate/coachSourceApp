//! 应用设置（与业务数据分开存，settings.json）。
//!
//! 只放「影响界面行为的开关」，目前是分词器相关两项；
//! 以后新增设置项时往 AppSettings 里加字段并给 #[serde(default)]，
//! 老文件读进来不会坏。

use std::fs;

use serde::{Deserialize, Serialize};
use tauri::AppHandle;

use crate::store::data_dir;

fn d_uncovered_color() -> String {
    // 暗色主题下的暖红，一眼能从正文里挑出来
    "#e06c75".to_string()
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub struct AppSettings {
    /// 分词器总开关，默认关闭
    #[serde(default)]
    pub tokenizer_enabled: bool,
    /// 分词视图里「未被词汇覆盖的字」的颜色
    #[serde(default = "d_uncovered_color")]
    pub uncovered_color: String,
}

impl Default for AppSettings {
    fn default() -> Self {
        Self {
            tokenizer_enabled: false,
            uncovered_color: d_uncovered_color(),
        }
    }
}

fn settings_path(app: &AppHandle) -> Result<std::path::PathBuf, String> {
    Ok(data_dir(app)?.join("settings.json"))
}

pub fn load(app: &AppHandle) -> Result<AppSettings, String> {
    let p = settings_path(app)?;
    if !p.exists() {
        return Ok(AppSettings::default());
    }
    let s = fs::read_to_string(&p).map_err(|e| e.to_string())?;
    serde_json::from_str(&s).map_err(|e| format!("设置文件解析失败：{}", e))
}

/// 原子写入，和数据文件同一套路
pub fn save(app: &AppHandle, s: &AppSettings) -> Result<(), String> {
    let p = settings_path(app)?;
    let tmp = p.with_extension("json.tmp");
    let body = serde_json::to_string_pretty(s).map_err(|e| e.to_string())?;
    fs::write(&tmp, body).map_err(|e| e.to_string())?;
    fs::rename(&tmp, &p).map_err(|e| e.to_string())?;
    Ok(())
}

#[tauri::command]
pub fn get_app_settings(app: AppHandle) -> Result<AppSettings, String> {
    load(&app)
}

#[tauri::command]
pub fn save_app_settings(app: AppHandle, settings: AppSettings) -> Result<(), String> {
    // 颜色值简单校验：必须是 #rgb / #rrggbb，防止把界面写坏
    let c = settings.uncovered_color.trim().to_string();
    let ok = (c.len() == 4 || c.len() == 7)
        && c.starts_with('#')
        && c[1..].chars().all(|ch| ch.is_ascii_hexdigit());
    if !ok {
        return Err("颜色格式不对，应该是 #e06c75 这样的".to_string());
    }
    let mut s = settings;
    s.uncovered_color = c.to_lowercase();
    save(&app, &s)
}

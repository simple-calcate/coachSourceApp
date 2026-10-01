use std::fs;
use std::path::PathBuf;

use tauri::AppHandle;
// 安卓 / iOS 没有系统文件对话框，DialogExt 只在桌面端引入
#[cfg(desktop)]
use tauri_plugin_dialog::DialogExt;

use crate::model::{Record, Turn};
use crate::store;

#[derive(Debug, Clone, serde::Serialize)]
#[serde(rename_all = "snake_case")]
pub struct ImportResult {
    pub added: usize,
    pub skipped: usize,
    pub images: usize,
}

/// 把一行 JSON 转成内部 Record。同时兼容两种格式：
/// - {"conversations":[...]}  对话格式
/// - {"text":"..."}           纯文本格式
fn line_to_record(v: &serde_json::Value) -> Option<Record> {
    if let Some(conv) = v.get("conversations").and_then(|c| c.as_array()) {
        let mut turns = Vec::new();
        for c in conv {
            let role = c
                .get("role")
                .and_then(|r| r.as_str())
                .unwrap_or("user")
                .to_string();
            let content = c
                .get("content")
                .and_then(|x| x.as_str())
                .unwrap_or("")
                .to_string();
            let reasoning = c
                .get("reasoning_content")
                .and_then(|x| x.as_str())
                .unwrap_or("")
                .to_string();
            let imgs = v
                .get("images")
                .and_then(|x| x.as_array())
                .map(|arr| {
                    arr.iter()
                        .filter_map(|s| s.as_str())
                        .map(|s| {
                            PathBuf::from(s)
                                .file_name()
                                .map(|n| n.to_string_lossy().to_string())
                                .unwrap_or_default()
                        })
                        .filter(|s| !s.is_empty())
                        .collect::<Vec<_>>()
                })
                .unwrap_or_default();

            // 图片统一挂在第一轮 user 上（我们只需要能取回文件）
            let images = if role == "user" && turns.is_empty() {
                imgs
            } else {
                Vec::new()
            };

            turns.push(Turn {
                role,
                content,
                reasoning_content: reasoning,
                images,
            });
        }
        if turns.is_empty() {
            return None;
        }
        let id = v
            .get("id")
            .and_then(|x| x.as_str())
            .unwrap_or("")
            .to_string();
        return Some(Record {
            id,
            title: String::new(),
            source: v
                .get("source")
                .and_then(|x| x.as_str())
                .unwrap_or("import")
                .to_string(),
            tags: v
                .get("tags")
                .and_then(|x| x.as_array())
                .map(|a| {
                    a.iter()
                        .filter_map(|s| s.as_str().map(|s| s.to_string()))
                        .collect()
                })
                .unwrap_or_default(),
            turns,
            created_at: v
                .get("created_at")
                .and_then(|x| x.as_str())
                .unwrap_or("")
                .to_string(),
            updated_at: String::new(),
            exported_at: None,
            archived_at: None,
            sync_batch: None,
        });
    }

    if let Some(text) = v.get("text").and_then(|t| t.as_str()) {
        if text.trim().is_empty() {
            return None;
        }
        return Some(Record {
            id: String::new(),
            title: String::new(),
            source: "pretrain".to_string(),
            tags: Vec::new(),
            turns: vec![Turn {
                role: "assistant".to_string(),
                content: text.to_string(),
                reasoning_content: String::new(),
                images: Vec::new(),
            }],
            created_at: String::new(),
            updated_at: String::new(),
            exported_at: None,
            archived_at: None,
            sync_batch: None,
        });
    }

    None
}

fn merge(app: &AppHandle, incoming: Vec<Record>, image_src: Option<PathBuf>) -> Result<ImportResult, String> {
    let mut rs = store::load_all(app)?;
    let mut existing_ids: std::collections::HashSet<String> =
        rs.iter().map(|r| r.id.clone()).collect();
    let mut existing_fps: std::collections::HashSet<String> =
        rs.iter().map(|r| r.fingerprint()).collect();

    let mut added = 0usize;
    let mut skipped = 0usize;
    let mut images = 0usize;

    // 若来源是 zip，先把里面的图片收进 images 目录
    if let Some(src) = image_src {
        let idir = store::image_dir(app)?;
        fs::create_dir_all(&idir).map_err(|e| e.to_string())?;
        if let Ok(rd) = fs::read_dir(&src) {
            for e in rd.flatten() {
                let p = e.path();
                if !p.is_file() {
                    continue;
                }
                let dest = idir.join(p.file_name().unwrap_or_default());
                if !dest.exists() {
                    fs::copy(&p, &dest).map_err(|e| e.to_string())?;
                }
                images += 1;
            }
        }
    }

    let now = chrono::Local::now().format("%Y-%m-%d %H:%M:%S").to_string();

    for mut rec in incoming {
        if existing_fps.contains(&rec.fingerprint()) {
            skipped += 1;
            continue;
        }
        if rec.id.is_empty() || existing_ids.contains(&rec.id) {
            rec.id = uuid::Uuid::new_v4().to_string();
        }
        if rec.created_at.is_empty() {
            rec.created_at = now.clone();
        }
        rec.updated_at = now.clone();
        existing_ids.insert(rec.id.clone());
        existing_fps.insert(rec.fingerprint());
        rs.push(rec);
        added += 1;
    }

    store::save_all(app, &rs)?;
    Ok(ImportResult {
        added,
        skipped,
        images,
    })
}

fn parse_text(content: &str) -> Vec<Record> {
    let mut out = Vec::new();

    // 先尝试整体按 JSON 解析（备份文件 / json 数组）
    if let Ok(v) = serde_json::from_str::<serde_json::Value>(content) {
        if let Some(arr) = v.as_array() {
            for item in arr {
                if let Some(r) = line_to_record(item) {
                    out.push(r);
                }
            }
            return out;
        }
        if let Some(r) = line_to_record(&v) {
            out.push(r);
            return out;
        }
    }

    // 逐行 jsonl
    for line in content.lines() {
        let line = line.trim();
        if line.is_empty() {
            continue;
        }
        if let Ok(v) = serde_json::from_str::<serde_json::Value>(line) {
            if let Some(r) = line_to_record(&v) {
                out.push(r);
            }
        }
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::model::Turn;

    fn turn(role: &str, content: &str, images: Vec<&str>) -> Turn {
        Turn {
            role: role.into(),
            content: content.into(),
            reasoning_content: String::new(),
            images: images.into_iter().map(|s| s.to_string()).collect(),
        }
    }

    #[test]
    fn import_sft_line() {
        let line = r#"{"conversations":[{"role":"user","content":"问题"},{"role":"assistant","content":"回答"}],"images":["images/img_a.jpg"]}"#;
        let v: serde_json::Value = serde_json::from_str(line).unwrap();
        let r = line_to_record(&v).unwrap();
        assert_eq!(r.turns.len(), 2);
        assert_eq!(r.turns[0].role, "user");
        assert_eq!(r.turns[1].content, "回答");
        // 路径要被还原成纯文件名，避免带上别的机器的目录结构
        assert_eq!(r.turns[0].images, vec!["img_a.jpg".to_string()]);
    }

    #[test]
    fn import_text_line() {
        let line = r#"{"text":"一整段话"}"#;
        let v: serde_json::Value = serde_json::from_str(line).unwrap();
        let r = line_to_record(&v).unwrap();
        assert_eq!(r.turns.len(), 1);
        assert_eq!(r.turns[0].content, "一整段话");
        assert_eq!(r.source, "pretrain");
    }

    #[test]
    fn export_then_import_keeps_content() {
        // 导出 -> 导入 往返一遍，正文不应该丢或被改写
        let orig = Record {
            id: "x".into(),
            title: String::new(),
            source: "manual".into(),
            tags: vec![],
            turns: vec![turn("user", "提问\n第二行", vec![]), turn("assistant", "回答", vec![])],
            created_at: String::new(),
            updated_at: String::new(),
            exported_at: None,
            archived_at: None,
            sync_batch: None,
        };
        let v = crate::exporters::sft_line(&orig, false);
        let back = line_to_record(&v).unwrap();
        assert_eq!(back.turns.len(), 2);
        assert_eq!(back.turns[0].content, "提问\n第二行");
        assert_eq!(back.turns[1].content, "回答");
    }

    #[test]
    fn jsonl_and_json_array_both_parse() {
        let jsonl = "{\"text\":\"a\"}\n{\"text\":\"b\"}\n";
        assert_eq!(parse_text(jsonl).len(), 2);

        let arr = r#"[{"text":"a"},{"text":"b"}]"#;
        assert_eq!(parse_text(arr).len(), 2);
    }

    #[test]
    fn garbage_lines_are_ignored() {
        let text = "这不是 json\n{\"text\":\"有效\"}\n";
        assert_eq!(parse_text(text).len(), 1);
    }
}

/// 从文件内容导入（移动端和网页端走这条路）
#[tauri::command]
pub fn import_text(app: AppHandle, content: String) -> Result<ImportResult, String> {
    let incoming = parse_text(&content);
    merge(&app, incoming, None)
}

/// 从文件路径导入，支持 .jsonl / .json / .txt / .zip（zip 里带图片）
#[tauri::command]
pub fn import_file(app: AppHandle, path: String) -> Result<ImportResult, String> {
    let p = PathBuf::from(&path);
    if !p.exists() {
        return Err("文件不存在".to_string());
    }

    let ext = p
        .extension()
        .and_then(|e| e.to_str())
        .unwrap_or("")
        .to_lowercase();

    if ext == "zip" {
        let tmp = std::env::temp_dir().join(format!(
            "coachsource_import_{}",
            uuid::Uuid::new_v4().simple()
        ));
        fs::create_dir_all(&tmp).map_err(|e| e.to_string())?;

        let f = fs::File::open(&p).map_err(|e| e.to_string())?;
        let mut archive = zip::ZipArchive::new(f).map_err(|e| format!("zip 解压失败：{}", e))?;

        let mut jsonl_text: Option<String> = None;
        let img_tmp = tmp.join("images");
        fs::create_dir_all(&img_tmp).map_err(|e| e.to_string())?;

        for i in 0..archive.len() {
            let mut entry = archive.by_index(i).map_err(|e| e.to_string())?;
            let name = entry.name().to_string();
            if entry.is_dir() {
                continue;
            }
            let mut buf = Vec::new();
            std::io::Read::read_to_end(&mut entry, &mut buf).map_err(|e| e.to_string())?;

            if name.starts_with("images/") {
                let fname = PathBuf::from(&name)
                    .file_name()
                    .unwrap_or_default()
                    .to_string_lossy()
                    .to_string();
                fs::write(img_tmp.join(fname), buf).map_err(|e| e.to_string())?;
            } else if (name.ends_with(".jsonl") || name.ends_with(".json"))
                && jsonl_text.is_none() {
                    jsonl_text = Some(String::from_utf8_lossy(&buf).to_string());
                }
        }

        let content = jsonl_text.ok_or_else(|| "压缩包里没有找到 .jsonl 文件".to_string())?;
        let incoming = parse_text(&content);
        let res = merge(&app, incoming, Some(img_tmp))?;
        let _ = fs::remove_dir_all(&tmp);
        return Ok(res);
    }

    let content = fs::read_to_string(&p).map_err(|e| e.to_string())?;
    let incoming = parse_text(&content);
    merge(&app, incoming, None)
}

/// 弹出文件选择框，返回选中的路径（仅桌面端）
#[cfg(desktop)]
#[tauri::command]
pub fn pick_import_file(app: AppHandle) -> Result<Option<String>, String> {
    let picked = app
        .dialog()
        .file()
        .add_filter("训练数据", &["jsonl", "json", "zip", "txt"])
        .blocking_pick_file();
    Ok(picked.map(|p| p.to_string()))
}

/// 手机端没有系统文件对话框，返回空让前端走「粘贴文本导入」
#[cfg(mobile)]
#[tauri::command]
pub fn pick_import_file(_app: AppHandle) -> Result<Option<String>, String> {
    Ok(None)
}

/// 弹出目录选择框（仅桌面端）
#[cfg(desktop)]
#[tauri::command]
pub fn pick_export_dir(app: AppHandle) -> Result<Option<String>, String> {
    let picked = app.dialog().file().blocking_pick_folder();
    Ok(picked.map(|p| p.to_string()))
}

/// 手机端不支持选择导出目录，一律导出到应用自己的 export 目录
#[cfg(mobile)]
#[tauri::command]
pub fn pick_export_dir(_app: AppHandle) -> Result<Option<String>, String> {
    Ok(None)
}

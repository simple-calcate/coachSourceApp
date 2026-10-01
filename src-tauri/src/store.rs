use std::fs;
use std::path::PathBuf;

use tauri::{AppHandle, Manager};

use crate::model::*;

/// 应用数据目录。桌面端和 Android 端都由 Tauri 给出各自合法的私有目录，
/// 不需要申请任何存储权限，也不用我们自己去拼路径。
pub fn data_dir(app: &AppHandle) -> Result<PathBuf, String> {
    app.path().app_data_dir().map_err(|e| e.to_string())
}

pub fn image_dir(app: &AppHandle) -> Result<PathBuf, String> {
    Ok(data_dir(app)?.join("images"))
}

pub fn export_dir(app: &AppHandle) -> Result<PathBuf, String> {
    Ok(data_dir(app)?.join("export"))
}

fn db_path(app: &AppHandle) -> Result<PathBuf, String> {
    Ok(data_dir(app)?.join("data.json"))
}

/// 启动时把目录结构建好
pub fn init_dirs(app: &AppHandle) -> Result<(), String> {
    let d = data_dir(app)?;
    fs::create_dir_all(d.join("images")).map_err(|e| e.to_string())?;
    fs::create_dir_all(d.join("export")).map_err(|e| e.to_string())?;
    Ok(())
}

pub fn load_all(app: &AppHandle) -> Result<Vec<Record>, String> {
    let p = db_path(app)?;
    if !p.exists() {
        return Ok(Vec::new());
    }
    let s = fs::read_to_string(&p).map_err(|e| e.to_string())?;
    if s.trim().is_empty() {
        return Ok(Vec::new());
    }
    serde_json::from_str(&s).map_err(|e| format!("数据文件解析失败：{}", e))
}

/// 原子写入：先写临时文件再改名，避免写一半崩溃导致整库损坏
pub fn save_all(app: &AppHandle, records: &[Record]) -> Result<(), String> {
    let p = db_path(app)?;
    let tmp = p.with_extension("json.tmp");
    let s = serde_json::to_string_pretty(records).map_err(|e| e.to_string())?;
    fs::write(&tmp, s).map_err(|e| e.to_string())?;
    fs::rename(&tmp, &p).map_err(|e| e.to_string())?;
    Ok(())
}

fn now() -> String {
    chrono::Local::now().format("%Y-%m-%d %H:%M:%S").to_string()
}

#[tauri::command]
pub fn get_config(app: AppHandle) -> Result<Config, String> {
    let d = data_dir(&app)?;
    Ok(Config {
        data_dir: d.to_string_lossy().to_string(),
        image_dir: image_dir(&app)?.to_string_lossy().to_string(),
        export_dir: export_dir(&app)?.to_string_lossy().to_string(),
        db_path: db_path(&app)?.to_string_lossy().to_string(),
    })
}

#[tauri::command]
pub fn get_stats(app: AppHandle) -> Result<Stats, String> {
    let rs = load_all(&app)?;
    let total = rs.len();
    let pending = rs.iter().filter(|r| r.bucket() == Bucket::Pending).count();
    let exported = rs.iter().filter(|r| r.bucket() == Bucket::Exported).count();
    let archived = rs.iter().filter(|r| r.bucket() == Bucket::Archived).count();
    let with_images = rs.iter().filter(|r| r.has_image()).count();
    let total_chars: usize = rs.iter().map(|r| r.chars()).sum();
    let turns: usize = rs.iter().map(|r| r.turns.len()).sum();
    Ok(Stats {
        total,
        exported,
        pending,
        archived,
        syncable: pending + exported,
        with_images,
        total_chars,
        turns,
    })
}

/// bucket: pending  = 还没导出的
///         exported = 本地导出过、还没同步上云
///         archived = 已同步上云，回收站
///         all      = 全部
#[tauri::command]
pub fn list_records(app: AppHandle, query: Option<String>, bucket: Option<String>) -> Result<Vec<RecordSummary>, String> {
    let mut rs = load_all(&app)?;
    rs.sort_by(|a, b| b.updated_at.cmp(&a.updated_at));

    let q = query.unwrap_or_default().trim().to_lowercase();
    let bucket = bucket.unwrap_or_else(|| "all".to_string());

    let out: Vec<RecordSummary> = rs
        .iter()
        .filter(|r| match bucket.as_str() {
            "pending" => r.bucket() == Bucket::Pending,
            "exported" => r.bucket() == Bucket::Exported,
            "archived" => r.bucket() == Bucket::Archived,
            _ => true,
        })
        .filter(|r| {
            if q.is_empty() {
                return true;
            }
            if r.title.to_lowercase().contains(&q)
                || r.source.to_lowercase().contains(&q)
                || r.tags.iter().any(|t| t.to_lowercase().contains(&q))
            {
                return true;
            }
            r.turns.iter().any(|t| t.content.to_lowercase().contains(&q))
        })
        .map(RecordSummary::from)
        .collect();

    Ok(out)
}

#[tauri::command]
pub fn get_record(app: AppHandle, id: String) -> Result<Record, String> {
    load_all(&app)?
        .into_iter()
        .find(|r| r.id == id)
        .ok_or_else(|| "找不到这条记录".to_string())
}

#[tauri::command]
pub fn save_record(app: AppHandle, record: Record) -> Result<Record, String> {
    let mut rs = load_all(&app)?;
    let mut rec = record;
    let ts = now();

    if rec.id.is_empty() {
        rec.id = uuid::Uuid::new_v4().to_string();
        rec.created_at = ts.clone();
    } else if rec.created_at.is_empty() {
        rec.created_at = ts.clone();
    }
    rec.updated_at = ts;

    // 去掉空轮次，避免导出出空对话
    rec.turns.retain(|t| !t.content.trim().is_empty() || !t.images.is_empty());
    if rec.turns.is_empty() {
        return Err("至少要有一轮非空内容".to_string());
    }

    if let Some(pos) = rs.iter().position(|r| r.id == rec.id) {
        rs[pos] = rec.clone();
    } else {
        rs.push(rec.clone());
    }
    save_all(&app, &rs)?;
    Ok(rec)
}

#[tauri::command]
pub fn delete_record(app: AppHandle, id: String) -> Result<(), String> {
    let mut rs = load_all(&app)?;
    if let Some(i) = rs.iter().position(|r| r.id == id) {
        let rec = rs.remove(i);
        // 顺手清掉这条记录独有的图片文件
        let idir = image_dir(&app)?;
        for t in &rec.turns {
            for name in &t.images {
                let _ = fs::remove_file(idir.join(name));
            }
        }
    }
    save_all(&app, &rs)
}

/// 接收前端传来的 base64 图片，落到 images 目录，返回文件名
#[tauri::command]
pub fn save_image(app: AppHandle, ext: String, data_base64: String) -> Result<String, String> {
    use base64::Engine;
    let raw = base64::engine::general_purpose::STANDARD
        .decode(data_base64.trim())
        .map_err(|e| format!("图片数据解码失败：{}", e))?;

    let ext = ext.trim().trim_start_matches('.').to_lowercase();
    let ext = match ext.as_str() {
        "jpg" | "jpeg" => "jpg",
        "png" => "png",
        "webp" => "webp",
        "gif" => "gif",
        _ => "jpg",
    };
    let name = format!("img_{}.{}", uuid::Uuid::new_v4().simple(), ext);
    let dir = image_dir(&app)?;
    fs::create_dir_all(&dir).map_err(|e| e.to_string())?;
    fs::write(dir.join(&name), raw).map_err(|e| e.to_string())?;
    Ok(name)
}

#[tauri::command]
pub fn delete_image(app: AppHandle, name: String) -> Result<(), String> {
    let dir = image_dir(&app)?;
    let p = dir.join(&name);
    if p.exists() {
        fs::remove_file(p).map_err(|e| e.to_string())?;
    }
    Ok(())
}

/// 把某条记录标记为已导出 / 未导出
#[tauri::command]
pub fn mark_exported(app: AppHandle, ids: Vec<String>) -> Result<(), String> {
    let mut rs = load_all(&app)?;
    let ts = chrono::Local::now().format("%Y-%m-%d %H:%M:%S").to_string();
    for r in rs.iter_mut() {
        if ids.contains(&r.id) {
            r.exported_at = Some(ts.clone());
        }
    }
    save_all(&app, &rs)
}

/// 把回收站里的记录还原回待导出列表（清掉已导出标记）
#[tauri::command]
pub fn restore_records(app: AppHandle, ids: Vec<String>) -> Result<usize, String> {
    let mut rs = load_all(&app)?;
    let mut n = 0usize;
    for r in rs.iter_mut() {
        if ids.contains(&r.id) && r.exported_at.is_some() {
            r.exported_at = None;
            n += 1;
        }
    }
    save_all(&app, &rs)?;
    Ok(n)
}

#[tauri::command]
pub fn reset_exported(app: AppHandle) -> Result<(), String> {
    let mut rs = load_all(&app)?;
    for r in rs.iter_mut() {
        r.exported_at = None;
    }
    save_all(&app, &rs)
}

/// 同步上云成功之后调用：把这些记录移进回收站（已归档）。
/// 正文完整保留在本地，只是不再参与下次同步；可恢复，也可彻底删除。
#[tauri::command]
pub fn archive_records(app: AppHandle, ids: Vec<String>, batch: String) -> Result<usize, String> {
    let mut rs = load_all(&app)?;
    let ts = now();
    let mut n = 0usize;
    for r in rs.iter_mut() {
        if ids.contains(&r.id) {
            r.archived_at = Some(ts.clone());
            r.sync_batch = Some(batch.clone());
            n += 1;
        }
    }
    save_all(&app, &rs)?;
    Ok(n)
}

/// 把回收站里的记录还原成「待导出」：清掉归档标记和已导出标记，
/// 这样它会出现在待导出列表里，并参与下一次同步。
#[tauri::command]
pub fn restore_from_archive(app: AppHandle, ids: Vec<String>) -> Result<usize, String> {
    let mut rs = load_all(&app)?;
    let mut n = 0usize;
    for r in rs.iter_mut() {
        if ids.contains(&r.id) && r.archived_at.is_some() {
            r.archived_at = None;
            r.exported_at = None;
            r.sync_batch = None;
            n += 1;
        }
    }
    save_all(&app, &rs)?;
    Ok(n)
}

/// 回收站专用：连同图片文件一起从本地彻底删掉，无法恢复。
#[tauri::command]
pub fn purge_records(app: AppHandle, ids: Vec<String>) -> Result<usize, String> {
    let mut rs = load_all(&app)?;
    let idir = image_dir(&app)?;
    let mut removed: Vec<Record> = Vec::new();
    rs.retain(|r| {
        if ids.contains(&r.id) {
            removed.push(r.clone());
            false
        } else {
            true
        }
    });
    for rec in &removed {
        for t in &rec.turns {
            for name in &t.images {
                let _ = fs::remove_file(idir.join(name));
            }
        }
    }
    save_all(&app, &rs)?;
    Ok(removed.len())
}

/// 下次同步会带上哪些记录：待导出 + 已导出，不含已归档
pub fn load_syncable(app: &AppHandle) -> Result<Vec<Record>, String> {
    let mut rs = load_all(app)?;
    rs.retain(|r| r.bucket() != Bucket::Archived);
    rs.sort_by(|a, b| a.created_at.cmp(&b.created_at));
    Ok(rs)
}

use std::fs;
use std::io::Write;
use std::path::{Path, PathBuf};
use std::str::FromStr;

use tauri::{AppHandle, Manager};
use tauri_plugin_fs::FsExt;

use crate::model::Record;
use crate::store;

#[derive(Debug, Clone, serde::Serialize)]
#[serde(rename_all = "snake_case")]
pub struct ExportResult {
    pub path: String,
    pub zip_path: Option<String>,
    /// 附带导出的分词器字典树文件（词汇表非空时才有）
    pub tokenizer_path: Option<String>,
    pub count: usize,
    pub skipped: usize,
    pub has_images: bool,
}

/// 生成 SFT 格式的一行：{"conversations":[{"role","content","reasoning_content"}...]}
/// 带图片时额外输出 "images":["images/xxx.jpg"]，并在 content 前插入 <image> 占位符。
pub(crate) fn sft_line(rec: &Record, with_meta: bool) -> serde_json::Value {
    let mut conversations: Vec<serde_json::Value> = Vec::new();
    let mut images: Vec<String> = Vec::new();

    for t in &rec.turns {
        let mut content = String::new();
        for name in &t.images {
            content.push_str("<image>\n");
            images.push(format!("images/{}", name));
        }
        content.push_str(&t.content);

        let mut turn = serde_json::Map::new();
        turn.insert("role".into(), serde_json::Value::String(t.role.clone()));
        turn.insert("content".into(), serde_json::Value::String(content));
        if !t.reasoning_content.is_empty() {
            turn.insert(
                "reasoning_content".into(),
                serde_json::Value::String(t.reasoning_content.clone()),
            );
        }
        conversations.push(serde_json::Value::Object(turn));
    }

    let mut obj = serde_json::Map::new();
    obj.insert(
        "conversations".into(),
        serde_json::Value::Array(conversations),
    );
    if !images.is_empty() {
        obj.insert("images".into(), serde_json::json!(images));
    }
    if with_meta {
        obj.insert("id".into(), serde_json::json!(rec.id));
        obj.insert("source".into(), serde_json::json!(rec.source));
        obj.insert("tags".into(), serde_json::json!(rec.tags));
        obj.insert("created_at".into(), serde_json::json!(rec.created_at));
    }
    serde_json::Value::Object(obj)
}

/// 生成预训练格式的一行：{"text":"..."}
fn pretrain_line(rec: &Record) -> serde_json::Value {
    let text: String = rec
        .turns
        .iter()
        .map(|t| t.content.as_str())
        .collect::<Vec<_>>()
        .join("\n");
    serde_json::json!({ "text": text })
}

fn build_jsonl(records: &[Record], format: &str, with_meta: bool) -> (String, usize) {
    let mut out = String::new();
    let mut skipped = 0usize;
    for rec in records {
        if format == "pretrain" && rec.has_image() {
            skipped += 1;
            continue;
        }
        let v = if format == "pretrain" {
            pretrain_line(rec)
        } else {
            sft_line(rec, with_meta)
        };
        out.push_str(&serde_json::to_string(&v).unwrap_or_default());
        out.push('\n');
    }
    (out, skipped)
}

fn make_zip(zip_path: &PathBuf, jsonl_name: &str, jsonl_bytes: &[u8], images_dir: &PathBuf) -> Result<(), String> {
    use zip::write::SimpleFileOptions;

    let file = fs::File::create(zip_path).map_err(|e| e.to_string())?;
    let mut zw = zip::ZipWriter::new(file);
    let opts = SimpleFileOptions::default().compression_method(zip::CompressionMethod::Deflated);

    zw.start_file(jsonl_name, opts)
        .map_err(|e| e.to_string())?;
    zw.write_all(jsonl_bytes).map_err(|e| e.to_string())?;

    if images_dir.exists() {
        let mut entries: Vec<PathBuf> = Vec::new();
        if let Ok(rd) = fs::read_dir(images_dir) {
            for e in rd.flatten() {
                entries.push(e.path());
            }
        }
        entries.sort();
        for p in entries {
            if !p.is_file() {
                continue;
            }
            let name = p.file_name().unwrap_or_default().to_string_lossy().to_string();
            zw.start_file(format!("images/{}", name), opts)
                .map_err(|e| e.to_string())?;
            let bytes = fs::read(&p).map_err(|e| e.to_string())?;
            zw.write_all(&bytes).map_err(|e| e.to_string())?;
        }
    }

    zw.finish().map_err(|e| e.to_string())?;
    Ok(())
}

fn copy_used_images(
    app: &AppHandle,
    selected: &[Record],
    img_out: &Path,
) -> Result<(), String> {
    fs::create_dir_all(img_out).map_err(|e| e.to_string())?;
    let src_dir = store::image_dir(app)?;
    for r in selected {
        for t in &r.turns {
            for name in &t.images {
                let s = src_dir.join(name);
                let d = img_out.join(name);
                if s.exists() && !d.exists() {
                    fs::copy(&s, &d).map_err(|e| e.to_string())?;
                }
            }
        }
    }
    Ok(())
}

/// 往 SAF content:// URI 里写全部字节（手机端「另存为」选中的位置）
fn write_uri(app: &AppHandle, uri: &str, bytes: &[u8]) -> Result<(), String> {
    let mut opts = tauri_plugin_fs::OpenOptions::new();
    opts.write(true).truncate(true);
    let fp = tauri_plugin_fs::FilePath::from_str(uri).expect("FilePath 解析不会失败");
    let mut f = app
        .fs()
        .open(fp, opts)
        .map_err(|e| format!("无法写入所选位置：{}", e))?;
    f.write_all(bytes).map_err(|e| e.to_string())?;
    Ok(())
}

/// 手机端「另存为」：target 是用户在系统保存对话框里选中的 content:// URI。
/// SAF 下只有这一个文件可写、建不了子目录，所以带图片时改写整个 zip 包；
/// 分词器字典树不跟着走（设置页有单独的导出入口）。
fn export_to_uri(
    app: &AppHandle,
    selected: &[Record],
    jsonl: &str,
    base: &str,
    uri: &str,
    has_images: bool,
    wants_zip: bool,
    skipped: usize,
) -> Result<ExportResult, String> {
    let payload: Vec<u8> = if wants_zip {
        // zip 需要可 Seek 的文件，先在应用缓存里落盘，再整个拷进 URI
        let tmp = app
            .path()
            .app_cache_dir()
            .map_err(|e| e.to_string())?
            .join(format!(
                "coachsource_export_{}",
                uuid::Uuid::new_v4().simple()
            ));
        let img_tmp = tmp.join("images");
        copy_used_images(app, selected, &img_tmp)?;
        let zip_path = tmp.join(base.replace(".jsonl", ".zip"));
        make_zip(&zip_path, base, jsonl.as_bytes(), &img_tmp)?;
        let bytes = fs::read(&zip_path).map_err(|e| e.to_string())?;
        let _ = fs::remove_dir_all(&tmp);
        bytes
    } else {
        jsonl.as_bytes().to_vec()
    };

    write_uri(app, uri, &payload)?;

    Ok(ExportResult {
        path: uri.to_string(),
        zip_path: None,
        tokenizer_path: None,
        count: selected.len(),
        skipped,
        has_images,
    })
}

#[tauri::command]
pub fn export_data(
    app: AppHandle,
    format: String,
    scope: String,
    with_meta: bool,
    filename: Option<String>,
    target_dir: Option<String>,
) -> Result<ExportResult, String> {
    let mut all = store::load_all(&app)?;
    all.sort_by(|a, b| a.created_at.cmp(&b.created_at));

    let selected: Vec<Record> = all
        .into_iter()
        .filter(|r| scope != "pending" || r.exported_at.is_none())
        .collect();

    if selected.is_empty() {
        return Err("没有符合条件的记录可以导出".to_string());
    }

    let (jsonl, skipped) = build_jsonl(&selected, &format, with_meta);

    let stamp = chrono::Local::now().format("%Y-%m-%d").to_string();
    let base = filename.unwrap_or_else(|| {
        let prefix = if format == "pretrain" { "pretrain_t2t" } else { "sft_t2t" };
        format!("{}_{}.jsonl", prefix, stamp)
    });

    // 是否包含图片
    let has_images = selected.iter().any(|r| r.has_image());

    // 手机端「另存为」给的是 content:// URI，走单文件写入
    if let Some(uri) = target_dir
        .as_deref()
        .map(str::trim)
        .filter(|d| d.starts_with("content://"))
    {
        return export_to_uri(
            &app,
            &selected,
            &jsonl,
            &base,
            uri,
            has_images,
            has_images && format != "pretrain",
            skipped,
        );
    }

    let out_dir: PathBuf = match target_dir {
        Some(d) if !d.trim().is_empty() => PathBuf::from(d.trim()),
        _ => store::export_dir(&app)?,
    };
    fs::create_dir_all(&out_dir).map_err(|e| e.to_string())?;

    // 有图时把用到的图片复制到 images/ 子目录
    let img_out = out_dir.join("images");
    if has_images {
        copy_used_images(&app, &selected, &img_out)?;
    }

    let jsonl_path = out_dir.join(&base);
    fs::write(&jsonl_path, jsonl.as_bytes()).map_err(|e| e.to_string())?;

    let zip_path = if has_images && format != "pretrain" {
        let zp = out_dir.join(base.replace(".jsonl", ".zip"));
        make_zip(&zp, &base, jsonl.as_bytes(), &img_out)?;
        Some(zp.to_string_lossy().to_string())
    } else {
        None
    };

    if scope == "pending" {
        let ids: Vec<String> = selected.iter().map(|r| r.id.clone()).collect();
        store::mark_exported(app.clone(), ids)?;
    }

    // 字典树单独成文件跟着一起导出（词汇表为空时自动跳过）
    let tokenizer_path = crate::tokenizer::write_export(&app, &out_dir)?
        .map(|p| p.to_string_lossy().to_string());

    Ok(ExportResult {
        path: jsonl_path.to_string_lossy().to_string(),
        zip_path,
        tokenizer_path,
        count: selected.len(),
        skipped,
        has_images,
    })
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

    fn rec(turns: Vec<Turn>) -> Record {
        Record {
            id: "t1".into(),
            title: String::new(),
            source: "manual".into(),
            tags: vec![],
            turns,
            created_at: "2026-10-02 00:00:00".into(),
            updated_at: String::new(),
            exported_at: None,
            archived_at: None,
            sync_batch: None,
        }
    }

    #[test]
    fn sft_line_keys_match_reference_file() {
        let r = rec(vec![turn("user", "问题", vec![]), turn("assistant", "回答", vec![])]);
        let v = sft_line(&r, false);
        let obj = v.as_object().unwrap();

        // 你原文件里每行只有 conversations 一个键，这里必须一致
        assert_eq!(obj.len(), 1, "不带图、不带元数据时应只有一个 conversations 键");
        assert!(obj.contains_key("conversations"));

        let conv = obj["conversations"].as_array().unwrap();
        assert_eq!(conv.len(), 2);
        for c in conv {
            let mut keys: Vec<&str> = c.as_object().unwrap().keys().map(|s| s.as_str()).collect();
            keys.sort();
            assert_eq!(keys, vec!["content", "role"]);
        }
        assert_eq!(conv[0]["role"], serde_json::json!("user"));
        assert_eq!(conv[1]["content"], serde_json::json!("回答"));
    }

    #[test]
    fn reasoning_content_only_when_filled() {
        let mut t = turn("assistant", "答案", vec![]);
        t.reasoning_content = "先换元再积分".into();
        let r = rec(vec![t]);
        let v = sft_line(&r, false);
        let c = &v["conversations"][0];
        assert_eq!(c["reasoning_content"], serde_json::json!("先换元再积分"));

        let r2 = rec(vec![turn("assistant", "答案", vec![])]);
        let v2 = sft_line(&r2, false);
        assert!(v2["conversations"][0].get("reasoning_content").is_none());
    }

    #[test]
    fn sft_line_with_image_has_placeholder_and_relative_path() {
        let r = rec(vec![
            turn("user", "这题怎么做", vec!["img_a.jpg"]),
            turn("assistant", "先换元", vec![]),
        ]);
        let v = sft_line(&r, false);
        assert_eq!(v["images"], serde_json::json!(["images/img_a.jpg"]));
        let c = v["conversations"][0]["content"].as_str().unwrap();
        assert!(c.starts_with("<image>\n"), "正文必须以 <image> 占位符开头");
    }

    #[test]
    fn pretrain_line_only_text_key() {
        let r = rec(vec![turn("user", "甲", vec![]), turn("assistant", "乙", vec![])]);
        let v = pretrain_line(&r);
        let obj = v.as_object().unwrap();
        assert_eq!(obj.len(), 1);
        assert_eq!(obj["text"], serde_json::json!("甲\n乙"));
    }

    #[test]
    fn pretrain_skips_records_with_images() {
        let with_img = rec(vec![turn("user", "x", vec!["a.jpg"])]);
        let plain = rec(vec![turn("user", "y", vec![])]);
        let (out, skipped) = build_jsonl(&[with_img, plain], "pretrain", false);
        assert_eq!(skipped, 1, "纯文本格式装不下图片，必须跳过并计数");
        assert_eq!(out.lines().count(), 1);
    }

    #[test]
    fn one_record_is_exactly_one_line() {
        // 正文里有换行时，JSON 必须把它转义成 \n，否则会把一行撑成多行，整个文件就废了
        let r = rec(vec![turn("user", "第一行\n第二行\n第三行", vec![])]);
        let (out, _) = build_jsonl(&[r], "sft", false);
        assert_eq!(out.lines().count(), 1, "一条记录只能占一行");
        let parsed: serde_json::Value = serde_json::from_str(out.trim()).unwrap();
        assert!(parsed.is_object());
    }
}

/// 把全部数据镜像成固定的两个文件，方便训练脚本永远读同一个路径
#[tauri::command]
pub fn mirror(app: AppHandle) -> Result<Vec<String>, String> {
    // 桌面端保持数据目录不变；Android 上数据目录在内部存储看不到，改放导出目录
    #[cfg(target_os = "android")]
    let dir = store::export_dir(&app)?;
    #[cfg(not(target_os = "android"))]
    let dir = store::data_dir(&app)?;
    let mut written = Vec::new();

    let mut sorted = store::load_all(&app)?;
    sorted.sort_by(|a, b| a.created_at.cmp(&b.created_at));

    let (sft, _) = build_jsonl(&sorted, "sft", false);
    let sft_path = dir.join("sft_t2t.jsonl");
    fs::write(&sft_path, sft.as_bytes()).map_err(|e| e.to_string())?;
    written.push(sft_path.to_string_lossy().to_string());

    let (pt, _) = build_jsonl(&sorted, "pretrain", false);
    let pt_path = dir.join("pretrain_t2t.jsonl");
    fs::write(&pt_path, pt.as_bytes()).map_err(|e| e.to_string())?;
    written.push(pt_path.to_string_lossy().to_string());

    if let Some(p) = crate::tokenizer::write_export(&app, &dir)? {
        written.push(p.to_string_lossy().to_string());
    }

    Ok(written)
}

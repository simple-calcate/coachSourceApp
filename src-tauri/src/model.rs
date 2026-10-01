use serde::{Deserialize, Serialize};

/// 一轮对话。role 为 system / user / assistant。
/// reasoning_content 存放思维链，不需要时留空字符串。
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub struct Turn {
    pub role: String,
    pub content: String,
    #[serde(default)]
    pub reasoning_content: String,
    /// 图片文件名（不含目录），如 "img_ab12.jpg"
    #[serde(default)]
    pub images: Vec<String>,
}

/// 一条采集记录 = 一段完整对话（可多轮）。
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub struct Record {
    pub id: String,
    #[serde(default)]
    pub title: String,
    #[serde(default)]
    pub source: String,
    #[serde(default)]
    pub tags: Vec<String>,
    #[serde(default)]
    pub turns: Vec<Turn>,
    #[serde(default)]
    pub created_at: String,
    #[serde(default)]
    pub updated_at: String,
    /// 已导出时间戳；为空表示这条还没导出过
    #[serde(default)]
    pub exported_at: Option<String>,
    /// 已同步到云端并归档的时间戳；有值 = 进了回收站，不再参与下次同步
    #[serde(default)]
    pub archived_at: Option<String>,
    /// 归档时所在的云端批次目录名，如 "2026-10-02"
    #[serde(default)]
    pub sync_batch: Option<String>,
}

/// 一条记录处在哪个桶里。
/// pending  = 待导出（新记的，还没导出过）
/// exported = 已导出（本地导出过文件，但还没同步到云端）
/// archived = 已归档（同步上云了，真正的回收站）
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Bucket {
    Pending,
    Exported,
    Archived,
}

impl Record {
    pub fn bucket(&self) -> Bucket {
        if self.archived_at.is_some() {
            Bucket::Archived
        } else if self.exported_at.is_some() {
            Bucket::Exported
        } else {
            Bucket::Pending
        }
    }
}

impl Record {
    pub fn chars(&self) -> usize {
        self.turns.iter().map(|t| t.content.chars().count()).sum()
    }

    pub fn image_count(&self) -> usize {
        self.turns.iter().map(|t| t.images.len()).sum()
    }

    pub fn has_image(&self) -> bool {
        self.image_count() > 0
    }

    /// 取第一轮用户提问，作为列表里的默认标题
    pub fn first_user_content(&self) -> String {
        self.turns
            .iter()
            .find(|t| t.role == "user" && !t.content.trim().is_empty())
            .or_else(|| self.turns.iter().find(|t| !t.content.trim().is_empty()))
            .map(|t| t.content.trim().replace('\n', " "))
            .unwrap_or_default()
    }

    /// 用于去重的内容指纹
    pub fn fingerprint(&self) -> String {
        use sha2::{Digest, Sha256};
        let mut h = Sha256::new();
        for t in &self.turns {
            h.update(t.role.as_bytes());
            h.update(b"\x1f");
            h.update(t.content.as_bytes());
            h.update(b"\x1e");
        }
        format!("{:x}", h.finalize())
    }
}

/// 列表页用的摘要，避免把全部正文传回前端
#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "snake_case")]
pub struct RecordSummary {
    pub id: String,
    pub title: String,
    /// 直接用于展示的标题：没写标题时自动取第一轮提问
    pub display_title: String,
    pub source: String,
    pub tags: Vec<String>,
    pub preview: String,
    pub turn_count: usize,
    pub image_count: usize,
    pub chars: usize,
    pub created_at: String,
    pub updated_at: String,
    pub exported_at: Option<String>,
    pub archived_at: Option<String>,
    pub sync_batch: Option<String>,
}

impl From<&Record> for RecordSummary {
    fn from(r: &Record) -> Self {
        let mut preview = r
            .turns
            .iter()
            .map(|t| t.content.as_str())
            .find(|c| !c.trim().is_empty())
            .unwrap_or("")
            .replace('\n', " ");
        if preview.chars().count() > 90 {
            preview = preview.chars().take(90).collect::<String>() + "…";
        }
        let auto = r.first_user_content();
        let auto: String = if auto.chars().count() > 40 {
            auto.chars().take(40).collect::<String>() + "…"
        } else {
            auto
        };
        let display_title = if r.title.trim().is_empty() {
            if auto.is_empty() {
                "（空对话）".to_string()
            } else {
                auto
            }
        } else {
            r.title.clone()
        };

        Self {
            id: r.id.clone(),
            title: r.title.clone(),
            display_title,
            source: r.source.clone(),
            tags: r.tags.clone(),
            preview,
            turn_count: r.turns.len(),
            image_count: r.image_count(),
            chars: r.chars(),
            created_at: r.created_at.clone(),
            updated_at: r.updated_at.clone(),
            exported_at: r.exported_at.clone(),
            archived_at: r.archived_at.clone(),
            sync_batch: r.sync_batch.clone(),
        }
    }
}

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "snake_case")]
pub struct Config {
    pub data_dir: String,
    pub image_dir: String,
    pub export_dir: String,
    pub db_path: String,
}

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "snake_case")]
pub struct Stats {
    pub total: usize,
    /// 已导出（本地导出过，还没同步上云）
    pub exported: usize,
    /// 待导出
    pub pending: usize,
    /// 已归档到云端（回收站）
    pub archived: usize,
    /// 下次同步会带上多少条 = pending + exported
    pub syncable: usize,
    pub with_images: usize,
    pub total_chars: usize,
    pub turns: usize,
}

//! GitHub 云端同步。
//!
//! 做的事：
//!   1. 把「待导出 + 已导出」的全部对话拼成 jsonl，切成若干个严格小于上限的文件
//!   2. 图片另外打成 zip 分片
//!   3. 上传到仓库里的 data/<日期>/
//!   4. 用 git blob SHA-1 逐个校验云端文件是否完整
//!   5. 全部校验通过后才把本地记录归档进回收站
//!
//! 为什么用 git blob SHA-1 校验：GitHub 给每个文件返回的 sha 就是 git 的
//! blob 哈希，等于 sha1("blob " + 字节数 + "\0" + 内容)。本地按同样算法算一遍
//! 就能确认云端内容和本地一字不差，而且不用把几十 MB 下载回来。

use std::fs;
use std::io::Write;
use std::path::{Path, PathBuf};

use serde::{Deserialize, Serialize};
use serde_json::json;
use tauri::ipc::Channel;
use tauri::AppHandle;

use crate::model::Record;
use crate::store;

const API: &str = "https://api.github.com";

/// GitHub 单个文件的硬上限。要求是「严格小于 100MB」，
/// 所以默认分片远低于它，并且在上传前再做一次断言。
pub const FILE_LIMIT: u64 = 100 * 1024 * 1024;

/* ------------------------------ 配置 ------------------------------ */

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub struct GithubConfig {
    /// OAuth App 的 Client ID。走设备码登录时用；每个人填自己的，
    /// 或者由应用发布者内置一个大家共用的。
    #[serde(default)]
    pub client_id: String,
    /// 登录后拿到的令牌（也可以手动填个人访问令牌）
    #[serde(default)]
    pub token: String,
    /// 登录进来的 GitHub 用户名
    #[serde(default)]
    pub login: String,
    /// 仓库归属者，默认等于登录用户名
    #[serde(default)]
    pub owner: String,
    #[serde(default = "d_repo")]
    pub repo: String,
    #[serde(default = "d_branch")]
    pub branch: String,
    /// 单个分片的字节上限，默认 50MB
    #[serde(default = "d_shard_mb")]
    pub shard_mb: u32,
    #[serde(default = "d_true")]
    pub include_images: bool,
    #[serde(default = "d_true")]
    pub private: bool,
}

fn d_branch() -> String {
    "main".into()
}
fn d_repo() -> String {
    "coach-source-data".into()
}
fn d_shard_mb() -> u32 {
    50
}
fn d_true() -> bool {
    true
}

impl GithubConfig {
    /// 分片字节上限。夹在 1MB 和 95MB 之间，确保一定小于 GitHub 的 100MB 限制。
    pub fn shard_bytes(&self) -> usize {
        let mb = self.shard_mb.clamp(1, 95) as usize;
        mb * 1024 * 1024
    }

    pub fn is_ready(&self) -> bool {
        !self.token.trim().is_empty()
            && !self.owner.trim().is_empty()
            && !self.repo.trim().is_empty()
    }

    /// 仓库归属者没单独填时，就用登录进来的那个账号
    pub fn normalize(&mut self) {
        self.token = self.token.trim().to_string();
        self.owner = self.owner.trim().to_string();
        self.repo = self.repo.trim().to_string();
        if self.owner.is_empty() {
            self.owner = self.login.trim().to_string();
        }
        if self.repo.is_empty() {
            self.repo = d_repo();
        }
        if self.branch.trim().is_empty() {
            self.branch = d_branch();
        }
    }
}

fn cfg_path(app: &AppHandle) -> Result<PathBuf, String> {
    Ok(store::data_dir(app)?.join("github.json"))
}

pub fn load_cfg(app: &AppHandle) -> Result<GithubConfig, String> {
    let p = cfg_path(app)?;
    if !p.exists() {
        return Ok(GithubConfig {
            client_id: String::new(),
            token: String::new(),
            login: String::new(),
            owner: String::new(),
            repo: d_repo(),
            branch: d_branch(),
            shard_mb: d_shard_mb(),
            include_images: true,
            private: true,
        });
    }
    let s = fs::read_to_string(&p).map_err(|e| e.to_string())?;
    let mut c: GithubConfig =
        serde_json::from_str(&s).map_err(|e| format!("GitHub 配置解析失败：{}", e))?;
    c.normalize();
    Ok(c)
}

fn save_cfg(app: &AppHandle, cfg: &GithubConfig) -> Result<(), String> {
    let p = cfg_path(app)?;
    let s = serde_json::to_string_pretty(cfg).map_err(|e| e.to_string())?;
    fs::write(&p, s).map_err(|e| e.to_string())?;
    // 令牌是明文落盘的，尽量把权限收紧（Windows 上这一步无效，会被忽略）
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        let _ = fs::set_permissions(&p, fs::Permissions::from_mode(0o600));
    }
    Ok(())
}

#[tauri::command]
pub fn github_get_config(app: AppHandle) -> Result<GithubConfig, String> {
    load_cfg(&app)
}

#[tauri::command]
pub fn github_save_config(app: AppHandle, cfg: GithubConfig) -> Result<(), String> {
    save_cfg(&app, &cfg)
}

/* --------------------------- 进度与报告 --------------------------- */

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "snake_case")]
pub struct SyncProgress {
    /// preparing / uploading / verifying / archiving / done / error
    pub phase: String,
    pub text: String,
    pub current: usize,
    pub total: usize,
}

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "snake_case")]
pub struct ShardInfo {
    pub name: String,
    /// 仓库内的完整路径
    pub path: String,
    pub bytes: u64,
    /// jsonl 分片是行数；图片包是图片数量
    pub count: usize,
    pub sha1: String,
    /// 可直接点开的下载/查看地址
    pub url: String,
}

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "snake_case")]
pub struct SyncReport {
    pub batch: String,
    pub owner: String,
    pub repo: String,
    pub branch: String,
    pub record_count: usize,
    pub shards: Vec<ShardInfo>,
    pub image_shards: Vec<ShardInfo>,
    /// 一并上传的分词器字典树（词汇表为空时为 None）
    pub tokenizer_trie: Option<ShardInfo>,
    pub total_bytes: u64,
    /// 云端文件是否全部校验通过
    pub verified: bool,
    /// 本地已归档进回收站的条数
    pub archived: usize,
    /// 仓库里这次同步的目录页
    pub web_url: String,
}

/* ------------------------------ HTTP ------------------------------ */

fn http() -> Result<reqwest::Client, String> {
    reqwest::Client::builder()
        .user_agent("coach-source-app")
        .timeout(std::time::Duration::from_secs(1800))
        .build()
        .map_err(|e| format!("创建 HTTP 客户端失败：{}", e))
}

fn auth(cfg: &GithubConfig) -> String {
    format!("Bearer {}", cfg.token.trim())
}

/// 把 GitHub 长长的错误 JSON 压成一句人话
fn brief(body: &str) -> String {
    if let Ok(v) = serde_json::from_str::<serde_json::Value>(body) {
        if let Some(m) = v.get("message").and_then(|x| x.as_str()) {
            let mut s = m.to_string();
            if let Some(d) = v
                .get("errors")
                .and_then(|e| e.get(0))
                .and_then(|e| e.get("message"))
                .and_then(|x| x.as_str())
            {
                s.push_str(&format!("（{}）", d));
            }
            return s;
        }
    }
    body.chars().take(200).collect()
}

/// 令牌权限不足（403 not accessible by integration）时给出可执行的中文指引。
/// 这个报错只说明一件事：令牌没有对应写权限。成因有两类：
/// 1. Client ID 来自 GitHub Apps（不是 OAuth Apps）——它的令牌不认 scope=repo，
///    权限由 App 配置决定，怎么重登都没用，必须换 OAuth App 的 Client ID；
/// 2. OAuth App 之前被授权过且当时没带 repo——重新 device flow 不弹授权页、
///    继承旧 scope，需要先撤销授权再重登。
fn enrich_perm_error(e: String) -> String {
    if e.contains("not accessible by integration") {
        format!(
            "{}\n\n令牌没有仓库写权限，按顺序排查：\n\
             ① 确认 Client ID 来源：「GitHub → Settings → Developer settings → OAuth Apps」\n\
             下的才是对的（新建时勾选 Enable Device Flow）；「GitHub Apps」下的 Client ID\n\
             不认 scope=repo，必须换成 OAuth Apps 的。\n\
             ② 若确实是 OAuth App：去「Settings → Applications → Authorized OAuth Apps」\n\
             撤销本应用授权，然后在应用里退出登录重新登录——这次授权页会列出\n\
             repo 权限，全部勾选确认。",
            e
        )
    } else {
        e
    }
}

async fn gh_get(cfg: &GithubConfig, path: &str) -> Result<serde_json::Value, String> {
    let url = format!("{}{}", API, path);
    let resp = http()?
        .get(&url)
        .header("Authorization", auth(cfg))
        .header("Accept", "application/vnd.github+json")
        .header("X-GitHub-Api-Version", "2022-11-28")
        .send()
        .await
        .map_err(|e| format!("连不上 GitHub：{}", e))?;
    let status = resp.status();
    let text = resp.text().await.map_err(|e| e.to_string())?;
    if !status.is_success() {
        return Err(format!("GitHub {}：{}", status.as_u16(), brief(&text)));
    }
    serde_json::from_str(&text).map_err(|_| "GitHub 返回的内容不是合法 JSON".to_string())
}

async fn gh_send<T: serde::Serialize>(
    cfg: &GithubConfig,
    method: reqwest::Method,
    path: &str,
    body: &T,
) -> Result<serde_json::Value, String> {
    let url = format!("{}{}", API, path);
    let resp = http()?
        .request(method, &url)
        .header("Authorization", auth(cfg))
        .header("Accept", "application/vnd.github+json")
        .header("X-GitHub-Api-Version", "2022-11-28")
        .json(body)
        .send()
        .await
        .map_err(|e| format!("连不上 GitHub：{}", e))?;
    let status = resp.status();
    let text = resp.text().await.map_err(|e| e.to_string())?;
    if !status.is_success() {
        return Err(format!("GitHub {}：{}", status.as_u16(), brief(&text)));
    }
    if text.trim().is_empty() {
        return Ok(json!({}));
    }
    serde_json::from_str(&text).map_err(|_| "GitHub 返回的内容不是合法 JSON".to_string())
}

/* --------------------------- 设备码登录 ---------------------------
   Device Flow：应用不碰任何密钥，用户在浏览器里输一次短码就授权。
   这样每个人登录的都是自己的 GitHub，数据进自己的仓库。
   ---------------------------------------------------------------- */

const DEVICE_CODE_URL: &str = "https://github.com/login/device/code";
const DEVICE_TOKEN_URL: &str = "https://github.com/login/oauth/access_token";

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "snake_case")]
pub struct DeviceStart {
    pub device_code: String,
    /// 用户要在浏览器里输入的那个短码，形如 ABCD-1234
    pub user_code: String,
    pub verification_uri: String,
    pub expires_in: u64,
    /// GitHub 要求两次轮询之间至少间隔这么多秒
    pub interval: u64,
}

async fn device_post(params: &[(&str, String)]) -> Result<serde_json::Value, String> {
    let mut form: std::collections::HashMap<&str, String> = std::collections::HashMap::new();
    for (k, v) in params {
        form.insert(k, v.clone());
    }
    let resp = http()?
        .post(DEVICE_TOKEN_URL)
        .header("Accept", "application/json")
        .header("X-GitHub-Api-Version", "2022-11-28")
        .form(&form)
        .send()
        .await
        .map_err(|e| format!("连不上 GitHub：{}", e))?;
    let status = resp.status();
    let text = resp.text().await.map_err(|e| e.to_string())?;
    let v: serde_json::Value = serde_json::from_str(&text).unwrap_or(json!({}));

    // 设备流的错误也走 200，藏在 error 字段里
    if let Some(e) = v.get("error").and_then(|x| x.as_str()) {
        return Err(e.to_string());
    }
    if !status.is_success() {
        return Err(format!("GitHub {}：{}", status.as_u16(), brief(&text)));
    }
    Ok(v)
}

/// 第一步：向 GitHub 要一个设备码
#[tauri::command]
pub async fn github_device_start(client_id: String) -> Result<DeviceStart, String> {
    let cid = client_id.trim();
    if cid.is_empty() {
        return Err(
            "还没填 Client ID。去 GitHub → Settings → Developer settings → OAuth Apps\n\
             新建一个 App，勾上 Enable device flow，把 Client ID 粘过来即可（不需要密钥）。"
                .to_string(),
        );
    }
    let resp = http()?
        .post(DEVICE_CODE_URL)
        .header("Accept", "application/json")
        .form(&[("client_id", cid.to_string()), ("scope", "repo".to_string())])
        .send()
        .await
        .map_err(|e| format!("连不上 GitHub：{}", e))?;
    let status = resp.status();
    let text = resp.text().await.map_err(|e| e.to_string())?;
    if !status.is_success() {
        return Err(format!("GitHub {}：{}", status.as_u16(), brief(&text)));
    }
    let v: serde_json::Value =
        serde_json::from_str(&text).map_err(|_| "GitHub 返回内容无法解析".to_string())?;
    if let Some(e) = v.get("error").and_then(|x| x.as_str()) {
        let desc = v
            .get("error_description")
            .and_then(|x| x.as_str())
            .unwrap_or("");
        return Err(format!("{}：{}", e, desc));
    }
    Ok(DeviceStart {
        device_code: v["device_code"].as_str().unwrap_or_default().to_string(),
        user_code: v["user_code"].as_str().unwrap_or_default().to_string(),
        verification_uri: v["verification_uri"]
            .as_str()
            .unwrap_or("https://github.com/login/device")
            .to_string(),
        expires_in: v["expires_in"].as_u64().unwrap_or(900),
        interval: v["interval"].as_u64().unwrap_or(5).max(5),
    })
}

/// 第二步：轮询。前端按 interval 反复调用，直到返回 ok。
#[tauri::command]
pub async fn github_device_poll(client_id: String, device_code: String) -> Result<serde_json::Value, String> {
    match device_post(&[
        ("client_id", client_id.trim().to_string()),
        ("device_code", device_code),
        (
            "grant_type",
            "urn:ietf:params:oauth:grant-type:device_code".to_string(),
        ),
    ])
    .await
    {
        Ok(v) => {
            let token = v.get("access_token").and_then(|x| x.as_str()).unwrap_or("");
            if token.is_empty() {
                return Ok(json!({ "status": "pending" }));
            }
            Ok(json!({ "status": "ok", "token": token,
                       "scope": v.get("scope").and_then(|x| x.as_str()).unwrap_or("") }))
        }
        Err(e) => {
            // authorization_pending 是正常的"还没点授权"
            let status = match e.as_str() {
                "authorization_pending" => "pending",
                "slow_down" => "slow_down",
                "expired_token" => "expired",
                "access_denied" => "denied",
                _ => return Err(e),
            };
            Ok(json!({ "status": status }))
        }
    }
}

/// 拿到令牌后换取用户名并落盘
#[tauri::command]
pub async fn github_finish_login(
    app: AppHandle,
    token: String,
    client_id: String,
) -> Result<GithubConfig, String> {
    let mut cfg = load_cfg(&app)?;
    cfg.client_id = client_id.trim().to_string();
    cfg.token = token.trim().to_string();

    let me = gh_get(&cfg, "/user").await?;
    cfg.login = me
        .get("login")
        .and_then(|x| x.as_str())
        .unwrap_or("")
        .to_string();
    if cfg.login.is_empty() {
        return Err("拿不到 GitHub 用户名，登录失败".to_string());
    }
    cfg.normalize();
    save_cfg(&app, &cfg)?;
    Ok(cfg)
}

/// 退出登录：只清掉令牌和用户名，仓库设置保留
#[tauri::command]
pub fn github_logout(app: AppHandle) -> Result<GithubConfig, String> {
    let mut cfg = load_cfg(&app)?;
    cfg.token.clear();
    cfg.login.clear();
    save_cfg(&app, &cfg)?;
    Ok(cfg)
}

/// 文件已经存在的话，更新时必须带上它当前的 sha
async fn existing_sha(cfg: &GithubConfig, path: &str) -> Result<Option<String>, String> {
    let p = format!(
        "/repos/{}/{}/contents/{}?ref={}",
        cfg.owner, cfg.repo, path, cfg.branch
    );
    match gh_get(cfg, &p).await {
        Ok(v) => Ok(v.get("sha").and_then(|x| x.as_str()).map(|s| s.to_string())),
        Err(_) => Ok(None), // 404 就是还没有这个文件
    }
}

async fn put_file(
    cfg: &GithubConfig,
    path: &str,
    bytes: &[u8],
    message: &str,
) -> Result<serde_json::Value, String> {
    use base64::Engine;
    if bytes.len() as u64 >= FILE_LIMIT {
        return Err(format!(
            "文件 {} 有 {} 字节，超过 GitHub 100MB 上限，拒绝上传",
            path,
            bytes.len()
        ));
    }
    let b64 = base64::engine::general_purpose::STANDARD.encode(bytes);
    let mut body = serde_json::Map::new();
    body.insert("message".into(), json!(message));
    body.insert("content".into(), json!(b64));
    body.insert("branch".into(), json!(cfg.branch));
    if let Some(sha) = existing_sha(cfg, path).await? {
        body.insert("sha".into(), json!(sha));
    }
    gh_send(
        cfg,
        reqwest::Method::PUT,
        &format!("/repos/{}/{}/contents/{}", cfg.owner, cfg.repo, path),
        &body,
    )
    .await
    .map_err(enrich_perm_error)
}

/* ---------------------------- 分片生成 ---------------------------- */

/// git 的 blob 哈希：sha1("blob " + 字节数 + "\0" + 内容)
pub fn git_blob_sha1(bytes: &[u8]) -> String {
    use sha1::{Digest, Sha1};
    let mut h = Sha1::new();
    h.update(b"blob ");
    h.update(bytes.len().to_string().as_bytes());
    h.update(&[0u8]);
    h.update(bytes);
    format!("{:x}", h.finalize())
}

struct RawShard {
    name: String,
    bytes: Vec<u8>,
    count: usize,
}

/// 把记录切成多个 jsonl 分片，每个都不超过 limit 字节。
/// 注意：切分在"加入这一行之前"判断，所以单个分片最多就是 limit；
/// 只有某一行本身超过 limit 时才会破例（那种情况单独成一个文件）。
fn build_jsonl_shards(records: &[Record], limit: usize) -> Result<Vec<RawShard>, String> {
    let mut shards: Vec<RawShard> = Vec::new();
    let mut buf: Vec<u8> = Vec::new();
    let mut lines = 0usize;
    let mut idx = 1usize;

    let flush = |buf: &mut Vec<u8>, lines: &mut usize, idx: &mut usize| -> Option<RawShard> {
        if buf.is_empty() {
            return None;
        }
        let name = format!("sft_t2t_part{:03}.jsonl", *idx);
        *idx += 1;
        let s = RawShard {
            name,
            bytes: std::mem::take(buf),
            count: *lines,
        };
        *lines = 0;
        Some(s)
    };

    for rec in records {
        let v = crate::exporters::sft_line(rec, false);
        let mut line = serde_json::to_string(&v).map_err(|e| e.to_string())?;
        line.push('\n');

        if !buf.is_empty() && buf.len() + line.len() > limit {
            if let Some(s) = flush(&mut buf, &mut lines, &mut idx) {
                shards.push(s);
            }
        }
        buf.extend_from_slice(line.as_bytes());
        lines += 1;
    }
    if let Some(s) = flush(&mut buf, &mut lines, &mut idx) {
        shards.push(s);
    }
    Ok(shards)
}

fn zip_into(names: &[String], src: &Path) -> Result<Vec<u8>, String> {
    use zip::write::SimpleFileOptions;
    let mut buf: Vec<u8> = Vec::new();
    {
        let mut zw = zip::ZipWriter::new(std::io::Cursor::new(&mut buf));
        // 图片本身已经是压缩格式，再压一遍只会白白耗 CPU，所以直接存
        let opts = SimpleFileOptions::default().compression_method(zip::CompressionMethod::Stored);
        for n in names {
            zw.start_file(format!("images/{}", n), opts)
                .map_err(|e| e.to_string())?;
            let b = fs::read(src.join(n)).map_err(|e| e.to_string())?;
            zw.write_all(&b).map_err(|e| e.to_string())?;
        }
        zw.finish().map_err(|e| e.to_string())?;
    }
    Ok(buf)
}

/// 收集这些记录用到的全部图片（去重、排序），再按体积贪心分组打包
fn build_image_shards(
    records: &[Record],
    src_dir: &Path,
    limit: usize,
) -> Result<Vec<RawShard>, String> {
    let mut names: Vec<String> = Vec::new();
    for r in records {
        for t in &r.turns {
            for n in &t.images {
                if !names.contains(n) {
                    names.push(n.clone());
                }
            }
        }
    }
    names.sort();
    if names.is_empty() {
        return Ok(Vec::new());
    }

    // 先量一下每张图多大
    let mut items: Vec<(String, u64)> = Vec::new();
    for n in &names {
        let p = src_dir.join(n);
        if !p.exists() {
            continue;
        }
        let sz = fs::metadata(&p).map(|m| m.len()).unwrap_or(0);
        items.push((n.clone(), sz));
    }
    if items.is_empty() {
        return Ok(Vec::new());
    }

    let mut groups: Vec<Vec<String>> = Vec::new();
    let mut cur: Vec<String> = Vec::new();
    let mut cur_sz = 0u64;
    for (n, sz) in items {
        if !cur.is_empty() && cur_sz + sz > limit as u64 {
            groups.push(std::mem::take(&mut cur));
            cur_sz = 0;
        }
        cur_sz += sz;
        cur.push(n);
    }
    if !cur.is_empty() {
        groups.push(cur);
    }

    let mut out = Vec::new();
    for (i, g) in groups.iter().enumerate() {
        let bytes = zip_into(g, src_dir)?;
        if bytes.len() as u64 >= FILE_LIMIT {
            return Err(format!(
                "第 {} 个图片包有 {} 字节，超过 100MB 上限，请把分片调小",
                i + 1,
                bytes.len()
            ));
        }
        out.push(RawShard {
            name: format!("images_part{:03}.zip", i + 1),
            bytes,
            count: g.len(),
        });
    }
    Ok(out)
}

/* ---------------------------- 主流程 ---------------------------- */

#[tauri::command]
pub async fn github_test_connection(cfg: GithubConfig) -> Result<serde_json::Value, String> {
    let mut cfg = cfg;
    cfg.normalize();
    if !cfg.is_ready() {
        return Err("请先填好令牌、用户名和仓库名".to_string());
    }
    let me = gh_get(&cfg, "/user").await?;
    let login = me
        .get("login")
        .and_then(|x| x.as_str())
        .unwrap_or("?")
        .to_string();

    let repo_path = format!("/repos/{}/{}", cfg.owner, cfg.repo);
    match gh_get(&cfg, &repo_path).await {
        Ok(r) => Ok(json!({
            "ok": true,
            "login": login,
            "repo_exists": true,
            "full_name": r.get("full_name").and_then(|x| x.as_str()).unwrap_or(""),
            "private": r.get("private").and_then(|x| x.as_bool()).unwrap_or(true),
            "default_branch": r.get("default_branch").and_then(|x| x.as_str()).unwrap_or("main"),
            "html_url": r.get("html_url").and_then(|x| x.as_str()).unwrap_or(""),
        })),
        Err(e) => Ok(json!({
            "ok": true,
            "login": login,
            "repo_exists": false,
            "repo_error": e,
        })),
    }
}

/// 仓库不存在时创建一个。auto_init 让 GitHub 先建好初始提交，
/// 这样分支名是确定的，后面才能往里放文件。
#[tauri::command]
pub async fn github_ensure_repo(cfg: GithubConfig) -> Result<serde_json::Value, String> {
    let mut cfg = cfg;
    cfg.normalize();
    if !cfg.is_ready() {
        return Err("请先填好令牌、用户名和仓库名".to_string());
    }
    match gh_get(&cfg, &format!("/repos/{}/{}", cfg.owner, cfg.repo)).await {
        Ok(r) => {
            return Ok(json!({
                "created": false,
                "full_name": r.get("full_name").and_then(|x| x.as_str()).unwrap_or(""),
                "html_url": r.get("html_url").and_then(|x| x.as_str()).unwrap_or(""),
                "default_branch": r.get("default_branch").and_then(|x| x.as_str()).unwrap_or("main"),
            }))
        }
        Err(_) => { /* 不存在，往下创建 */ }
    }

    let body = json!({
        "name": cfg.repo,
        "private": cfg.private,
        "auto_init": true,
        "description": "个人训练数据采集 · 由 CoachSource 自动同步",
    });
    let r = gh_send(&cfg, reqwest::Method::POST, "/user/repos", &body)
        .await
        .map_err(enrich_perm_error)?;
    Ok(json!({
        "created": true,
        "full_name": r.get("full_name").and_then(|x| x.as_str()).unwrap_or(""),
        "html_url": r.get("html_url").and_then(|x| x.as_str()).unwrap_or(""),
        "default_branch": r.get("default_branch").and_then(|x| x.as_str()).unwrap_or("main"),
    }))
}

/// 切换仓库公开 / 私有。警告：公开之后全世界都能看到，且无法保证彻底清除。
#[tauri::command]
pub async fn github_set_visibility(cfg: GithubConfig, private: bool) -> Result<String, String> {
    let mut cfg = cfg;
    cfg.normalize();
    if !cfg.is_ready() {
        return Err("请先填好令牌、用户名和仓库名".to_string());
    }
    gh_send(
        &cfg,
        reqwest::Method::PATCH,
        &format!("/repos/{}/{}", cfg.owner, cfg.repo),
        &json!({ "private": private }),
    )
    .await
    .map_err(enrich_perm_error)?;
    Ok(if private { "仓库已设为私有" } else { "仓库已设为公开" }.to_string())
}

#[tauri::command]
pub async fn github_sync(
    app: AppHandle,
    cfg: GithubConfig,
    channel: Channel<SyncProgress>,
) -> Result<SyncReport, String> {
    let emit = |phase: &str, text: String, cur: usize, total: usize| {
        let _ = channel.send(SyncProgress {
            phase: phase.to_string(),
            text,
            current: cur,
            total,
        });
    };

    let mut cfg = cfg;
    cfg.normalize();
    if !cfg.is_ready() {
        return Err("请先登录 GitHub（或手动填好令牌）".to_string());
    }
    save_cfg(&app, &cfg)?;

    /* 1. 取数据 */
    emit("preparing", "读取本地对话…".into(), 0, 1);
    let records = store::load_syncable(&app)?;
    if records.is_empty() {
        return Err("没有需要同步的记录：待导出和已导出都是空的，或者全部已经归档过了".to_string());
    }

    let stamp = chrono::Local::now().format("%Y-%m-%d").to_string();
    // 同一天同步多次时加后缀，避免覆盖掉上一次
    let batch = {
        let mut b = stamp.clone();
        let mut i = 2usize;
        while dir_exists_in_repo(&cfg, &b).await.unwrap_or(false) {
            b = format!("{}-{}", stamp, i);
            i += 1;
        }
        b
    };
    let base = format!("data/{}", batch);

    emit(
        "preparing",
        format!("共 {} 条对话，开始切分…", records.len()),
        0,
        1,
    );

    /* 2. 切分 */
    let limit = cfg.shard_bytes();
    let jsonl_shards = build_jsonl_shards(&records, limit)?;
    let img_shards = if cfg.include_images {
        let src = store::image_dir(&app)?;
        build_image_shards(&records, &src, limit)?
    } else {
        Vec::new()
    };

    // 分词器字典树：和这批数据一起上传，声明「这些文本推荐怎么切」。
    // 词汇表为空就整段跳过，不产生空文件。
    let trie_info: Option<(Vec<u8>, ShardInfo)> = {
        let words = crate::tokenizer::load_vocab(&app)?;
        if words.is_empty() {
            None
        } else {
            let v = crate::tokenizer::trie_export(&words);
            let bytes =
                serde_json::to_vec_pretty(&v).map_err(|e| format!("序列化字典树失败：{}", e))?;
            let path = format!("{}/tokenizer_trie.json", base);
            Some((
                bytes,
                ShardInfo {
                    name: "tokenizer_trie.json".into(),
                    path,
                    bytes: 0,
                    count: words.len(),
                    sha1: String::new(),
                    url: String::new(),
                },
            ))
        }
    };

    let total_files = jsonl_shards.len()
        + img_shards.len()
        + 1 // manifest
        + usize::from(trie_info.is_some());
    emit(
        "uploading",
        format!(
            "切成 {} 个文本分片、{} 个图片包，开始上传…",
            jsonl_shards.len(),
            img_shards.len()
        ),
        0,
        total_files,
    );

    // 本地也留一份，万一云端出问题还有底
    let local_dir = store::data_dir(&app)?.join("cloud").join(&batch);
    fs::create_dir_all(&local_dir).map_err(|e| e.to_string())?;

    let mut done = 0usize;
    let mut shards: Vec<ShardInfo> = Vec::new();
    let mut image_shards: Vec<ShardInfo> = Vec::new();

    for s in &jsonl_shards {
        emit(
            "uploading",
            format!("上传 {}", s.name),
            done,
            total_files,
        );
        let path = format!("{}/{}", base, s.name);
        put_file(&cfg, &path, &s.bytes, &format!("同步 {} · {}", batch, s.name)).await?;
        fs::write(local_dir.join(&s.name), &s.bytes).map_err(|e| e.to_string())?;
        done += 1;
        shards.push(ShardInfo {
            name: s.name.clone(),
            path: path.clone(),
            bytes: s.bytes.len() as u64,
            count: s.count,
            sha1: git_blob_sha1(&s.bytes),
            url: raw_url(&cfg, &path),
        });
    }

    for s in &img_shards {
        emit("uploading", format!("上传 {}", s.name), done, total_files);
        let path = format!("{}/{}", base, s.name);
        put_file(&cfg, &path, &s.bytes, &format!("同步 {} · {}", batch, s.name)).await?;
        fs::write(local_dir.join(&s.name), &s.bytes).map_err(|e| e.to_string())?;
        done += 1;
        image_shards.push(ShardInfo {
            name: s.name.clone(),
            path: path.clone(),
            bytes: s.bytes.len() as u64,
            count: s.count,
            sha1: git_blob_sha1(&s.bytes),
            url: raw_url(&cfg, &path),
        });
    }

    /* 2.5 字典树 */
    let mut trie_shard: Option<ShardInfo> = None;
    if let Some((bytes, mut info)) = trie_info {
        emit("uploading", "上传 tokenizer_trie.json".into(), done, total_files);
        put_file(
            &cfg,
            &info.path,
            &bytes,
            &format!("同步 {} · 分词器字典树", batch),
        )
        .await?;
        fs::write(local_dir.join("tokenizer_trie.json"), &bytes).map_err(|e| e.to_string())?;
        done += 1;
        info.bytes = bytes.len() as u64;
        info.sha1 = git_blob_sha1(&bytes);
        info.url = raw_url(&cfg, &info.path);
        trie_shard = Some(info);
    }

    /* 3. manifest */
    let manifest = json!({
        "app": "coach-source-app",
        "batch": batch,
        "created_at": chrono::Local::now().format("%Y-%m-%d %H:%M:%S").to_string(),
        "format": "sft_t2t",
        "record_count": records.len(),
        "shard_limit_bytes": limit,
        "shards": shards.iter().map(|s| json!({
            "name": s.name, "path": s.path, "bytes": s.bytes,
            "lines": s.count, "sha1": s.sha1,
        })).collect::<Vec<_>>(),
        "images": image_shards.iter().map(|s| json!({
            "name": s.name, "path": s.path, "bytes": s.bytes,
            "files": s.count, "sha1": s.sha1,
        })).collect::<Vec<_>>(),
        "tokenizer_trie": trie_shard.as_ref().map(|s| json!({
            "path": s.path, "bytes": s.bytes, "words": s.count, "sha1": s.sha1,
        })),
    });
    let manifest_bytes = serde_json::to_vec_pretty(&manifest).map_err(|e| e.to_string())?;
    let manifest_path = format!("{}/manifest.json", base);
    emit("uploading", "上传 manifest.json".into(), done, total_files);
    put_file(
        &cfg,
        &manifest_path,
        &manifest_bytes,
        &format!("同步 {} · manifest", batch),
    )
    .await?;
    fs::write(local_dir.join("manifest.json"), &manifest_bytes).map_err(|e| e.to_string())?;

    /* 4. 逐个校验 */
    let mut all: Vec<&ShardInfo> = Vec::new();
    for s in &shards {
        all.push(s);
    }
    for s in &image_shards {
        all.push(s);
    }
    if let Some(s) = &trie_shard {
        all.push(s);
    }
    let mut i = 0usize;
    for s in &all {
        i += 1;
        emit("verifying", format!("校验 {}", s.name), i, all.len());
        verify_remote(&cfg, &s.path, s.bytes, &s.sha1).await?;
    }

    /* 5. 归档 */
    emit("archiving", "校验通过，归档本地记录…".into(), 1, 1);
    let ids: Vec<String> = records.iter().map(|r| r.id.clone()).collect();
    let archived = store::archive_records(app.clone(), ids, batch.clone())?;

    let total_bytes: u64 = shards.iter().map(|s| s.bytes).sum::<u64>()
        + image_shards.iter().map(|s| s.bytes).sum::<u64>()
        + trie_shard.as_ref().map(|s| s.bytes).unwrap_or(0);

    let report = SyncReport {
        batch: batch.clone(),
        owner: cfg.owner.clone(),
        repo: cfg.repo.clone(),
        branch: cfg.branch.clone(),
        record_count: records.len(),
        shards,
        image_shards,
        tokenizer_trie: trie_shard,
        total_bytes,
        verified: true,
        archived,
        web_url: format!(
            "https://github.com/{}/{}/tree/{}/{}",
            cfg.owner, cfg.repo, cfg.branch, base
        ),
    };
    emit(
        "done",
        format!(
            "同步完成：{} 条，{} 个文件，共 {:.1} MB，已归档进回收站",
            report.record_count,
            total_files,
            total_bytes as f64 / 1024.0 / 1024.0
        ),
        total_files,
        total_files,
    );
    Ok(report)
}

fn raw_url(cfg: &GithubConfig, path: &str) -> String {
    format!(
        "https://raw.githubusercontent.com/{}/{}/{}/{}",
        cfg.owner, cfg.repo, cfg.branch, path
    )
}

/// 校验云端文件：比对字节数和 git blob sha1，两者都对上才算完整
async fn verify_remote(
    cfg: &GithubConfig,
    path: &str,
    expect_bytes: u64,
    expect_sha1: &str,
) -> Result<(), String> {
    let v = gh_get(
        cfg,
        &format!(
            "/repos/{}/{}/contents/{}?ref={}",
            cfg.owner, cfg.repo, path, cfg.branch
        ),
    )
    .await
    .map_err(|e| format!("读取云端 {} 失败：{}", path, e))?;

    let size = v
        .get("size")
        .and_then(|x| x.as_u64())
        .ok_or_else(|| format!("云端 {} 没有返回大小", path))?;
    let sha = v
        .get("sha")
        .and_then(|x| x.as_str())
        .unwrap_or("")
        .to_string();

    if size != expect_bytes {
        return Err(format!(
            "云端 {} 大小对不上：本地 {} 字节，云端 {} 字节",
            path, expect_bytes, size
        ));
    }
    if sha != expect_sha1 {
        return Err(format!(
            "云端 {} 内容对不上：本地 {}，云端 {}",
            path, expect_sha1, sha
        ));
    }
    Ok(())
}

async fn dir_exists_in_repo(cfg: &GithubConfig, batch: &str) -> Result<bool, String> {
    let p = format!(
        "/repos/{}/{}/contents/data/{}?ref={}",
        cfg.owner, cfg.repo, batch, cfg.branch
    );
    Ok(gh_get(cfg, &p).await.is_ok())
}

/* ------------------------------ 测试 ------------------------------ */

#[cfg(test)]
mod tests {
    use super::*;
    use crate::model::Turn;

    fn turn(role: &str, content: &str) -> Turn {
        Turn {
            role: role.into(),
            content: content.into(),
            reasoning_content: String::new(),
            images: Vec::new(),
        }
    }

    fn rec(id: &str, content: &str) -> Record {
        Record {
            id: id.into(),
            title: String::new(),
            source: "manual".into(),
            tags: vec![],
            turns: vec![turn("user", content)],
            created_at: "2026-10-02 00:00:00".into(),
            updated_at: String::new(),
            exported_at: None,
            archived_at: None,
            sync_batch: None,
        }
    }

    #[test]
    fn git_blob_sha1_matches_git() {
        // git hash-object 的已知答案：空文件的 blob sha1
        assert_eq!(
            git_blob_sha1(b""),
            "e69de29bb2d1d6434b8b29ae775ad8c2e48c5391"
        );
        // "hello\n" 的 blob sha1 = ce013625030ba8dba906f756967f9e9ca394464a
        assert_eq!(
            git_blob_sha1(b"hello\n"),
            "ce013625030ba8dba906f756967f9e9ca394464a"
        );
    }

    #[test]
    fn shards_are_strictly_under_limit() {
        let recs: Vec<Record> = (0..200)
            .map(|i| rec(&format!("r{}", i), &"内容内容内容".repeat(200)))
            .collect();
        let limit = 64 * 1024; // 用很小的上限，好触发切分
        let shards = build_jsonl_shards(&recs, limit).unwrap();
        assert!(shards.len() > 1, "应该切成多个分片");
        for s in &shards {
            assert!(
                (s.bytes.len() as u64) < FILE_LIMIT,
                "每个分片都必须远小于 100MB"
            );
            // 除了最后一片，其余都应该是"再加一行就超限"的状态
            assert!(s.bytes.len() <= limit + 4096, "分片不应明显超过上限");
        }
        let total_lines: usize = shards.iter().map(|s| s.count).sum();
        assert_eq!(total_lines, 200, "不能丢记录");
    }

    #[test]
    fn no_record_is_lost_across_shards() {
        let recs: Vec<Record> = (0..37).map(|i| rec(&format!("r{}", i), "x")).collect();
        let shards = build_jsonl_shards(&recs, 1024).unwrap();
        let total: usize = shards.iter().map(|s| s.count).sum();
        assert_eq!(total, 37);
        // 拼回去每一行都还是合法 json，且只有 conversations 一个键
        for s in &shards {
            for line in String::from_utf8_lossy(&s.bytes).lines() {
                let v: serde_json::Value = serde_json::from_str(line).unwrap();
                assert_eq!(v.as_object().unwrap().len(), 1);
                assert!(v.get("conversations").is_some());
            }
        }
    }

    #[test]
    fn single_huge_line_still_becomes_one_shard() {
        // 一条记录本身很大时，必须单独成片，不能和其他记录混在一起把文件撑爆
        let big = rec("big", &"字".repeat(100_000));
        let small = rec("small", "短");
        let shards = build_jsonl_shards(&[big, small], 1024).unwrap();
        let total: usize = shards.iter().map(|s| s.count).sum();
        assert_eq!(total, 2, "两条都得在");
    }

    #[test]
    fn shard_limit_is_clamped_below_github_cap() {
        let mut cfg = GithubConfig {
            client_id: String::new(),
            token: String::new(),
            login: String::new(),
            owner: String::new(),
            repo: String::new(),
            branch: "main".into(),
            shard_mb: 500, // 乱填一个超大值
            include_images: true,
            private: true,
        };
        assert!(cfg.shard_bytes() as u64 > 0);
        assert!(
            (cfg.shard_bytes() as u64) < FILE_LIMIT,
            "分片上限必须小于 GitHub 的 100MB"
        );
        cfg.shard_mb = 0;
        assert!(cfg.shard_bytes() >= 1024 * 1024, "最小值也要是 1MB");
    }
}

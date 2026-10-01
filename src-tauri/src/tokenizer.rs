//! 分词器：最大前缀匹配 + 词汇表持久化 + 字典树（trie）导出。
//!
//! 设计要点：
//!   - 词汇表是唯一的「真相来源」，存在 vocab.json 里（一个词一行元数据）。
//!     匹配用的 trie 每次从词汇表在内存里构建，数据量小（个人词汇表几千词顶天），
//!     构建是毫秒级，不需要缓存，也就没有「缓存和文件不一致」这一类 bug。
//!   - 导出格式是嵌套字典树 JSON（tokenizer_trie.json）：
//!     每个节点 {"e": 是否成词, "c": {下一字: 节点}}。
//!     两棵树的合并就是逐节点取并集，天然适合「多份导出以后合并训练分词器」。
//!   - 匹配算法：贪心的最大前缀匹配。从当前位置沿 trie 走到不能再走，
//!     记录途中最长成词点；成词就切出一个 token，不成词就把当前这一个字
//!     标为「未被覆盖」。

use std::collections::BTreeMap;
use std::fs;
use std::path::Path;

use serde::{Deserialize, Serialize};
use serde_json::{json, Value};
use tauri::AppHandle;

use crate::store::data_dir;

pub const TRIE_FORMAT: &str = "coachsource-tokenizer-trie";
pub const TRIE_VERSION: u32 = 1;

/* --------------------------- 词汇表持久化 --------------------------- */

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub struct VocabWord {
    pub word: String,
    pub added_at: String,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
struct VocabFile {
    version: u32,
    words: Vec<VocabWord>,
}

fn vocab_path(app: &AppHandle) -> Result<std::path::PathBuf, String> {
    Ok(data_dir(app)?.join("vocab.json"))
}

pub fn load_vocab(app: &AppHandle) -> Result<Vec<VocabWord>, String> {
    let p = vocab_path(app)?;
    if !p.exists() {
        return Ok(Vec::new());
    }
    let s = fs::read_to_string(&p).map_err(|e| e.to_string())?;
    let f: VocabFile = serde_json::from_str(&s).map_err(|e| format!("词汇表解析失败：{}", e))?;
    Ok(f.words)
}

fn save_vocab(app: &AppHandle, words: &[VocabWord]) -> Result<(), String> {
    let p = vocab_path(app)?;
    let tmp = p.with_extension("json.tmp");
    let f = VocabFile {
        version: 1,
        words: words.to_vec(),
    };
    let body = serde_json::to_string_pretty(&f).map_err(|e| e.to_string())?;
    fs::write(&tmp, body).map_err(|e| e.to_string())?;
    fs::rename(&tmp, &p).map_err(|e| e.to_string())?;
    Ok(())
}

fn now() -> String {
    chrono::Local::now().format("%Y-%m-%d %H:%M:%S").to_string()
}

/// 清洗用户输入的词：去首尾空白、去掉换行、拒绝超长词（防误粘整段文字）
fn clean_word(raw: &str) -> Option<String> {
    let w: String = raw.split_whitespace().collect::<Vec<_>>().join("");
    if w.is_empty() || w.chars().count() > 32 {
        return None;
    }
    Some(w)
}

/* ------------------------------ trie ------------------------------ */

#[derive(Debug, Default)]
pub struct TrieNode {
    children: BTreeMap<char, TrieNode>,
    is_word: bool,
}

impl TrieNode {
    pub fn insert(&mut self, word: &str) {
        let mut node = self;
        for ch in word.chars() {
            node = node.children.entry(ch).or_default();
        }
        node.is_word = true;
    }

    pub fn build<'a>(words: impl IntoIterator<Item = &'a str>) -> Self {
        let mut root = Self::default();
        for w in words {
            root.insert(w);
        }
        root
    }
}

/// 切出来的一段。start / end 是字符下标（不是字节下标），前端按字符数组对齐。
#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "snake_case")]
pub struct Span {
    pub start: usize,
    /// 不含 end 位置
    pub end: usize,
    pub text: String,
    /// true = 被词汇表里的某个词覆盖
    pub covered: bool,
}

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "snake_case")]
pub struct TokenizeResult {
    pub spans: Vec<Span>,
    pub total_chars: usize,
    pub covered_chars: usize,
    /// 0.0 ~ 1.0
    pub ratio: f64,
}

/// 最大前缀匹配。整段无词可切时，每个字都是一个未覆盖 span。
pub fn tokenize(text: &str, root: &TrieNode) -> TokenizeResult {
    let chars: Vec<char> = text.chars().collect();
    let mut spans: Vec<Span> = Vec::new();
    let mut covered_chars = 0usize;
    let mut i = 0usize;

    while i < chars.len() {
        // 从 i 出发沿 trie 尽量走远，记住最后一个「成词」的位置
        let mut best_end: Option<usize> = None;
        let mut node: Option<&TrieNode> = root.children.get(&chars[i]);
        let mut j = i;
        while let Some(cur) = node {
            j += 1;
            if cur.is_word {
                best_end = Some(j);
            }
            if j >= chars.len() {
                break;
            }
            node = cur.children.get(&chars[j]);
        }

        let (end, covered) = match best_end {
            Some(e) => (e, true),
            None => (i + 1, false),
        };
        if covered {
            covered_chars += end - i;
        }
        spans.push(Span {
            start: i,
            end,
            text: chars[i..end].iter().collect(),
            covered,
        });
        i = end;
    }

    let total = chars.len();
    TokenizeResult {
        ratio: if total == 0 {
            1.0
        } else {
            covered_chars as f64 / total as f64
        },
        spans,
        total_chars: total,
        covered_chars,
    }
}

/* --------------------------- 字典树导出/合并 --------------------------- */

fn node_to_value(node: &TrieNode) -> Value {
    let mut children = serde_json::Map::new();
    for (ch, child) in &node.children {
        children.insert(ch.to_string(), node_to_value(child));
    }
    json!({ "e": node.is_word as u8, "c": children })
}

/// 生成导出对象。words 为空时 word_count = 0，调用方据此决定要不要写文件。
pub fn trie_export(words: &[VocabWord]) -> Value {
    let root = TrieNode::build(words.iter().map(|w| w.word.as_str()));
    json!({
        "format": TRIE_FORMAT,
        "version": TRIE_VERSION,
        "created_at": now(),
        "word_count": words.len(),
        "root": node_to_value(&root),
    })
}

/// 两棵树逐节点取并集：e 取或，c 递归合并。a 会被就地修改。
fn merge_node(a: &mut Value, b: &Value) {
    let Some(ao) = a.as_object_mut() else { return };
    let Some(bo) = b.as_object() else { return };

    let be = bo.get("e").and_then(|v| v.as_u64()).unwrap_or(0);
    if be == 1 {
        ao.insert("e".into(), json!(1));
    }

    let bc = match bo.get("c").and_then(|v| v.as_object()) {
        Some(m) => m,
        None => return,
    };
    let ac = ao
        .entry("c")
        .or_insert_with(|| json!({}))
        .as_object_mut()
        .unwrap();
    for (k, bv) in bc {
        match ac.get_mut(k) {
            Some(av) => merge_node(av, bv),
            None => {
                ac.insert(k.clone(), bv.clone());
            }
        }
    }
}

/// 合并任意多份导出。合并后 word_count 重算为去重词数。
#[allow(dead_code)]
pub fn trie_merge(files: &[Value]) -> Value {
    let mut out = json!({
        "format": TRIE_FORMAT,
        "version": TRIE_VERSION,
        "created_at": now(),
        "word_count": 0,
        "root": { "e": 0, "c": {} },
    });
    for f in files {
        let Some(root) = f.get("root") else { continue };
        if let Some(dst) = out.get_mut("root") {
            merge_node(dst, root);
        }
    }
    // 先算完再写，避免同时持有 out 的可变和不可变借用
    let words = words_from_trie(&out["root"]);
    if let Some(o) = out.as_object_mut() {
        o.insert("word_count".into(), json!(words.len()));
        o.insert("merged_from".into(), json!(files.len()));
    }
    out
}

/// 从字典树 JSON 里把所有词读出来（用于导入）
pub fn words_from_trie(root: &Value) -> Vec<String> {
    let mut out = Vec::new();
    fn walk(node: &Value, prefix: &mut String, out: &mut Vec<String>) {
        let is_word = node.get("e").and_then(|v| v.as_u64()).unwrap_or(0) == 1;
        if is_word && !prefix.is_empty() {
            out.push(prefix.clone());
        }
        if let Some(children) = node.get("c").and_then(|v| v.as_object()) {
            for (ch, child) in children {
                prefix.push_str(ch);
                walk(child, prefix, out);
                for _ in 0..ch.chars().count() {
                    prefix.pop();
                }
            }
        }
    }
    walk(root, &mut String::new(), &mut out);
    out
}

/// 把导出写成一个文件（词汇表非空才写）。返回写到的路径。
pub fn write_export(app: &AppHandle, dir: &Path) -> Result<Option<std::path::PathBuf>, String> {
    let words = load_vocab(app)?;
    if words.is_empty() {
        return Ok(None);
    }
    let v = trie_export(&words);
    let body = serde_json::to_vec_pretty(&v).map_err(|e| e.to_string())?;
    let p = dir.join("tokenizer_trie.json");
    fs::write(&p, body).map_err(|e| e.to_string())?;
    Ok(Some(p))
}

/* ------------------------------ 命令 ------------------------------ */

/// trie 内存缓存：分词观察开着的时候每敲一段字就 analyze 一次，
/// 不能每次都读盘 + 重建整棵树。加词 / 删词 / 导入成功后主动失效。
static TRIE_CACHE: std::sync::OnceLock<std::sync::Mutex<Option<std::sync::Arc<TrieNode>>>> =
    std::sync::OnceLock::new();

fn cached_trie(app: &AppHandle) -> Result<std::sync::Arc<TrieNode>, String> {
    let cell = TRIE_CACHE.get_or_init(|| std::sync::Mutex::new(None));
    let mut guard = cell.lock().map_err(|_| "分词缓存锁坏了".to_string())?;
    if let Some(t) = guard.as_ref() {
        return Ok(t.clone());
    }
    let words = load_vocab(app)?;
    let t = std::sync::Arc::new(TrieNode::build(words.iter().map(|w| w.word.as_str())));
    *guard = Some(t.clone());
    Ok(t)
}

fn invalidate_trie_cache() {
    if let Some(cell) = TRIE_CACHE.get() {
        if let Ok(mut g) = cell.lock() {
            *g = None;
        }
    }
}

#[tauri::command]
pub fn tokenizer_analyze(app: AppHandle, text: String) -> Result<TokenizeResult, String> {
    let root = cached_trie(&app)?;
    Ok(tokenize(&text, &root))
}

#[tauri::command]
pub fn tokenizer_vocab(app: AppHandle) -> Result<Vec<VocabWord>, String> {
    let mut words = load_vocab(&app)?;
    words.sort_by(|a, b| a.word.cmp(&b.word));
    Ok(words)
}

/// 加词的核心逻辑（不碰文件，便于单测）：清洗、去重、拒绝单字。
///
/// 不收录单个字：单字对最大前缀匹配没有增益（任何字都能单独成"词"），
/// 全是单字的调用直接报错；混着单字时忽略单字、只加多字的。
fn merge_words(vocab: &mut Vec<VocabWord>, words: Vec<String>) -> Result<usize, String> {
    let mut existing: std::collections::HashSet<String> =
        vocab.iter().map(|w| w.word.clone()).collect();

    let ts = now();
    let (mut added, mut multi, mut singles) = (0usize, 0usize, 0usize);
    for raw in words {
        let Some(w) = clean_word(&raw) else { continue };
        if w.chars().count() < 2 {
            singles += 1;
            continue;
        }
        multi += 1;
        if existing.contains(&w) {
            continue;
        }
        existing.insert(w.clone());
        vocab.push(VocabWord {
            word: w,
            added_at: ts.clone(),
        });
        added += 1;
    }
    if multi == 0 && singles > 0 {
        return Err("分词器不收录单个字，请选择两个及以上的相邻字".to_string());
    }
    Ok(added)
}

/// 批量加词，自动去重清洗。返回真正新增的数量。
#[tauri::command]
pub fn tokenizer_add_words(app: AppHandle, words: Vec<String>) -> Result<usize, String> {
    let mut vocab = load_vocab(&app)?;
    let added = merge_words(&mut vocab, words)?;
    vocab.sort_by(|a, b| a.word.cmp(&b.word));
    save_vocab(&app, &vocab)?;
    invalidate_trie_cache();
    Ok(added)
}

/// 批量删词，返回删掉的数量
#[tauri::command]
pub fn tokenizer_remove_words(app: AppHandle, words: Vec<String>) -> Result<usize, String> {
    let mut vocab = load_vocab(&app)?;
    let before = vocab.len();
    let rm: std::collections::HashSet<String> =
        words.into_iter().filter_map(|w| clean_word(&w)).collect();
    vocab.retain(|v| !rm.contains(&v.word));
    save_vocab(&app, &vocab)?;
    invalidate_trie_cache();
    Ok(before - vocab.len())
}

/// 导入一份字典树 JSON（比如从别的设备导出的），词自动去重合并。返回新增数。
#[tauri::command]
pub fn tokenizer_import_trie(app: AppHandle, content: String) -> Result<usize, String> {
    let v: Value = serde_json::from_str(&content).map_err(|e| format!("不是合法的字典树文件：{}", e))?;
    let root = v
        .get("root")
        .ok_or_else(|| "文件里没有 root 字段，不是分词器导出".to_string())?;
    let words = words_from_trie(root);
    if words.is_empty() {
        return Err("文件是合法的，但里面一个词都没有".to_string());
    }
    // 复用 add 的清洗与去重逻辑
    tokenizer_add_words(app, words)
}

/// 把当前词汇表导出成文件（写到导出目录），返回路径。词汇表为空时报错。
#[tauri::command]
pub fn tokenizer_export_file(app: AppHandle) -> Result<String, String> {
    let dir = crate::store::export_dir(&app)?;
    fs::create_dir_all(&dir).map_err(|e| e.to_string())?;
    let p = write_export(&app, &dir)?;
    match p {
        Some(p) => Ok(p.to_string_lossy().to_string()),
        None => Err("词汇表是空的，先在设置里添加一些词".to_string()),
    }
}

/* ------------------------------ 测试 ------------------------------ */

#[cfg(test)]
mod tests {
    use super::*;

    fn vocab(words: &[&str]) -> Vec<VocabWord> {
        words
            .iter()
            .map(|w| VocabWord {
                word: w.to_string(),
                added_at: String::new(),
            })
            .collect()
    }

    #[test]
    fn max_prefix_prefers_longest_match() {
        let root = TrieNode::build(["机器学习", "机器", "学习", "深度学习"]);
        let r = tokenize("机器学习入门", &root);
        let texts: Vec<(&str, bool)> = r.spans.iter().map(|s| (s.text.as_str(), s.covered)).collect();
        assert_eq!(
            texts,
            vec![("机器学习", true), ("入", false), ("门", false)],
            "必须贪心吃掉最长的词，而不是先匹配到「机器」就停"
        );
        assert_eq!(r.covered_chars, 4);
        assert_eq!(r.total_chars, 6);
    }

    #[test]
    fn empty_vocab_marks_everything_uncovered() {
        let root = TrieNode::default();
        let r = tokenize("任意文本", &root);
        assert!(!r.spans.iter().any(|s| s.covered));
        assert_eq!(r.ratio, 0.0);
    }

    #[test]
    fn empty_text_gives_full_ratio() {
        let root = TrieNode::build(["词"]);
        let r = tokenize("", &root);
        assert!(r.spans.is_empty());
        assert_eq!(r.ratio, 1.0); // 0/0 按全覆盖处理，避免除零
    }

    #[test]
    fn repeated_word_matches_every_time() {
        let root = TrieNode::build(["好"]);
        let r = tokenize("好好好", &root);
        assert!(r.spans.iter().all(|s| s.covered));
    }

    #[test]
    fn trie_export_roundtrip() {
        let words = vocab(&["机器学习", "机器", "学习"]);
        let v = trie_export(&words);
        assert_eq!(v["format"], json!(TRIE_FORMAT));
        assert_eq!(v["word_count"], json!(3));
        let back = words_from_trie(&v["root"]);
        let mut expect = vec!["学习".to_string(), "机器".to_string(), "机器学习".to_string()];
        expect.sort();
        let mut got = back.clone();
        got.sort();
        assert_eq!(got, expect, "导出的树必须能无损读回全部词");
    }

    #[test]
    fn trie_merge_is_union() {
        let a = trie_export(&vocab(&["机器", "机器学习"]));
        let b = trie_export(&vocab(&["学习", "机器学习", "深度"]));
        let merged = trie_merge(&[a, b]);
        let mut words = words_from_trie(&merged["root"]);
        words.sort();
        assert_eq!(
            words,
            vec!["学习", "机器", "机器学习", "深度"],
            "合并 = 并集，交集不重复（Rust 按字节序排序）"
        );
        assert_eq!(merged["word_count"], json!(4), "count 是去重后的词数");
    }

    #[test]
    fn merge_with_empty_side_keeps_words() {
        let a = trie_export(&vocab(&["词甲"]));
        let empty = trie_export(&[]);
        let merged = trie_merge(&[a, empty]);
        let mut words = words_from_trie(&merged["root"]);
        words.sort();
        assert_eq!(words, vec!["词甲"]);
    }

    #[test]
    fn clean_word_rejects_garbage() {
        assert_eq!(clean_word("  机器 学习 \n"), Some("机器学习".into()));
        assert_eq!(clean_word("   "), None);
        assert_eq!(clean_word(""), None);
        let long: String = std::iter::repeat_n("长", 33).collect();
        assert_eq!(clean_word(&long), None, "超过 32 字的当误粘处理，拒绝");
    }

    #[test]
    fn merge_words_rejects_all_singles() {
        let mut v = vocab(&[]);
        let r = merge_words(&mut v, vec!["字".into(), " a ".into()]);
        assert!(r.is_err(), "全是单字必须报错，不能静默收录");
        assert!(v.is_empty());
        let r = merge_words(&mut v, vec!["词".into(), "  ".into()]);
        assert!(r.is_err(), "清洗后只剩单字同样报错");
    }

    #[test]
    fn merge_words_skips_singles_but_keeps_multi() {
        let mut v = vocab(&["已有"]);
        let r = merge_words(&mut v, vec!["字".into(), "新词".into(), "已有".into()]);
        assert_eq!(r.unwrap(), 1, "忽略单字和重复，只新增「新词」");
        assert_eq!(v.len(), 2);
        assert!(v.iter().any(|w| w.word == "新词"));
    }
}

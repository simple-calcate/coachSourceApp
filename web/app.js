'use strict';

/* ============================================================
   CoachSource —— 个人训练数据采集
   存储与导出全部由 Rust 后端完成，这里只负责界面。
   若不在 Tauri 环境里（比如用浏览器直接打开），
   自动降级到 localStorage，保证界面仍可用。
   ============================================================ */

const T = window.__TAURI_INTERNALS__;
const IS_TAURI = !!T;

async function invoke(cmd, args) {
  if (!IS_TAURI) throw new Error('不在 Tauri 环境中');
  return T.invoke(cmd, args == null ? {} : args);
}

/* ---------- 浏览器兜底后端 ---------- */
const LS_RECORDS = 'coachsource.records.v1';
const LS_DRAFT = 'coachsource.draft.v1';
const IMG_PREFIX = 'coachsource.img.';

const LocalBackend = {
  load() {
    try {
      return JSON.parse(localStorage.getItem(LS_RECORDS) || '[]');
    } catch (e) {
      return [];
    }
  },
  save(rs) {
    localStorage.setItem(LS_RECORDS, JSON.stringify(rs));
  },
  uid() {
    return 'r-' + Math.random().toString(36).slice(2, 10) + Date.now().toString(36);
  },
  now() {
    const d = new Date();
    const p = (n) => String(n).padStart(2, '0');
    return `${d.getFullYear()}-${p(d.getMonth() + 1)}-${p(d.getDate())} ${p(d.getHours())}:${p(d.getMinutes())}:${p(d.getSeconds())}`;
  },
};

/* ---------- 通用小工具 ---------- */
const $ = (sel) => document.querySelector(sel);
const $$ = (sel) => Array.from(document.querySelectorAll(sel));

function toast(msg, ok) {
  const el = $('#toast');
  if (!el) {
    const d = document.createElement('div');
    d.id = 'toast';
    document.body.appendChild(d);
    return toast(msg, ok);
  }
  el.textContent = msg;
  el.className = 'toast show' + (ok === false ? ' err' : '');
  clearTimeout(el._t);
  el._t = setTimeout(() => {
    el.className = 'toast';
  }, 2600);
}

function download(name, blob) {
  const url = URL.createObjectURL(blob);
  const a = document.createElement('a');
  a.href = url;
  a.download = name;
  document.body.appendChild(a);
  a.click();
  a.remove();
  setTimeout(() => URL.revokeObjectURL(url), 1000);
}

/* ---------- 展示用格式化 ---------- */

/** 字数：一万以下直接显示，以上换算成「万字」 */
function fmtChars(n) {
  if (n < 10000) return n.toLocaleString('zh-CN') + ' 字';
  return (n / 10000).toFixed(1).replace(/\.0$/, '') + ' 万字';
}

/** 列表标题：没写标题就用第一轮提问当标题 */
function displayTitle(rec) {
  const t = (rec.title || '').trim();
  if (t) return t;
  const first =
    (rec.turns || []).find((x) => x.role === 'user' && x.content.trim()) ||
    (rec.turns || []).find((x) => x.content.trim());
  if (!first) return '（空对话）';
  const s = first.content.trim().replace(/\s*\n\s*/g, ' ');
  return s.length > 40 ? s.slice(0, 40) + '…' : s;
}

/** 列表预览：优先显示助手的回复，比重复显示问题更有信息量 */
function previewOf(rec) {
  const t = (rec.turns || []).find((x) => x.role === 'assistant' && x.content.trim());
  const s = t ? t.content.trim().replace(/\s*\n\s*/g, ' ') : '';
  return s.length > 90 ? s.slice(0, 90) + '…' : s;
}

function readFileAsDataURL(file) {
  return new Promise((res, rej) => {
    const r = new FileReader();
    r.onload = () => res(String(r.result));
    r.onerror = rej;
    r.readAsDataURL(file);
  });
}

function readFileAsText(file) {
  return new Promise((res, rej) => {
    const r = new FileReader();
    r.onload = () => res(String(r.result));
    r.onerror = rej;
    r.readAsText(file);
  });
}

/* ---------- 极简 zip（仅存储模式，用于浏览器兜底导出） ---------- */
const CRCT = (() => {
  const t = new Uint32Array(256);
  for (let n = 0; n < 256; n++) {
    let c = n;
    for (let k = 0; k < 8; k++) c = c & 1 ? 0xedb88320 ^ (c >>> 1) : c >>> 1;
    t[n] = c >>> 0;
  }
  return t;
})();

function crc32(buf) {
  let c = 0xffffffff;
  for (let i = 0; i < buf.length; i++) c = CRCT[(c ^ buf[i]) & 0xff] ^ (c >>> 8);
  return (c ^ 0xffffffff) >>> 0;
}

function makeZip(files) {
  const enc = new TextEncoder();
  const chunks = [];
  const central = [];
  let offset = 0;
  const d = new Date();
  const dosTime = ((d.getHours() << 11) | (d.getMinutes() << 5) | (d.getSeconds() >> 1)) & 0xffff;
  const dosDate = (((d.getFullYear() - 1980) << 9) | ((d.getMonth() + 1) << 5) | d.getDate()) & 0xffff;

  for (const f of files) {
    const nb = enc.encode(f.name);
    const crc = crc32(f.data);
    const size = f.data.length;

    const lh = new Uint8Array(30 + nb.length);
    const lv = new DataView(lh.buffer);
    lv.setUint32(0, 0x04034b50, true);
    lv.setUint16(4, 20, true);
    lv.setUint16(6, 0x0800, true);
    lv.setUint16(8, 0, true);
    lv.setUint16(10, dosTime, true);
    lv.setUint16(12, dosDate, true);
    lv.setUint32(14, crc, true);
    lv.setUint32(18, size, true);
    lv.setUint32(22, size, true);
    lv.setUint16(26, nb.length, true);
    lh.set(nb, 30);

    const ch = new Uint8Array(46 + nb.length);
    const cv = new DataView(ch.buffer);
    cv.setUint32(0, 0x02014b50, true);
    cv.setUint16(4, 20, true);
    cv.setUint16(6, 20, true);
    cv.setUint16(8, 0x0800, true);
    cv.setUint16(10, 0, true);
    cv.setUint16(12, dosTime, true);
    cv.setUint16(14, dosDate, true);
    cv.setUint32(16, crc, true);
    cv.setUint32(20, size, true);
    cv.setUint32(24, size, true);
    cv.setUint16(28, nb.length, true);
    cv.setUint32(42, offset, true);
    ch.set(nb, 46);

    chunks.push(lh, f.data);
    central.push(ch);
    offset += lh.length + size;
  }

  const cdSize = central.reduce((a, b) => a + b.length, 0);
  const eocd = new Uint8Array(22);
  const ev = new DataView(eocd.buffer);
  ev.setUint32(0, 0x06054b50, true);
  ev.setUint16(8, files.length, true);
  ev.setUint16(10, files.length, true);
  ev.setUint32(12, cdSize, true);
  ev.setUint32(16, offset, true);

  return new Blob([...chunks, ...central, eocd], { type: 'application/zip' });
}

function dataURLToBytes(dataUrl) {
  const b64 = dataUrl.slice(dataUrl.indexOf(',') + 1);
  const bin = atob(b64);
  const out = new Uint8Array(bin.length);
  for (let i = 0; i < bin.length; i++) out[i] = bin.charCodeAt(i);
  return out;
}

/* ============================================================
   后端适配层：优先 Tauri，降级 localStorage
   ============================================================ */
const B = {
  async config() {
    if (IS_TAURI) return invoke('get_config');
    return {
      data_dir: '浏览器本地存储',
      image_dir: '—',
      export_dir: '—',
      db_path: 'localStorage',
    };
  },
  async stats() {
    if (IS_TAURI) return invoke('get_stats');
    const rs = LocalBackend.load();
    const archived = rs.filter((r) => r.archived_at).length;
    const exported = rs.filter((r) => r.exported_at && !r.archived_at).length;
    return {
      total: rs.length,
      archived,
      exported,
      pending: rs.length - archived - exported,
      syncable: rs.length - archived,
      with_images: rs.filter((r) => r.turns.some((t) => (t.images || []).length)).length,
      total_chars: rs.reduce((a, r) => a + r.turns.reduce((x, t) => x + t.content.length, 0), 0),
      turns: rs.reduce((a, r) => a + r.turns.length, 0),
    };
  },
  async list(query, bucket) {
    if (IS_TAURI) return invoke('list_records', { query, bucket: bucket || 'all' });
    let rs = LocalBackend.load();
    rs.sort((a, b) => (b.updated_at || '').localeCompare(a.updated_at || ''));
    const q = (query || '').trim().toLowerCase();
    if (bucket === 'pending') rs = rs.filter((r) => !r.exported_at && !r.archived_at);
    if (bucket === 'exported') rs = rs.filter((r) => !!r.exported_at && !r.archived_at);
    if (bucket === 'archived') rs = rs.filter((r) => !!r.archived_at);
    if (q) {
      rs = rs.filter(
        (r) =>
          (r.title || '').toLowerCase().includes(q) ||
          (r.source || '').toLowerCase().includes(q) ||
          (r.tags || []).some((t) => t.toLowerCase().includes(q)) ||
          r.turns.some((t) => t.content.toLowerCase().includes(q))
      );
    }
    return rs.map((r) => ({
      id: r.id,
      title: r.title,
      display_title: displayTitle(r),
      source: r.source,
      tags: r.tags,
      preview: previewOf(r),
      turn_count: r.turns.length,
      image_count: r.turns.reduce((a, t) => a + (t.images || []).length, 0),
      chars: r.turns.reduce((a, t) => a + t.content.length, 0),
      created_at: r.created_at,
      updated_at: r.updated_at,
      exported_at: r.exported_at || null,
      archived_at: r.archived_at || null,
      sync_batch: r.sync_batch || null,
    }));
  },
  async restore(ids) {
    if (IS_TAURI) return invoke('restore_records', { ids });
    const rs = LocalBackend.load();
    let n = 0;
    rs.forEach((r) => {
      if (ids.includes(r.id) && r.exported_at) {
        r.exported_at = null;
        n++;
      }
    });
    LocalBackend.save(rs);
    return n;
  },
  /** 回收站 → 待导出：清掉归档和已导出标记 */
  async restoreFromArchive(ids) {
    if (IS_TAURI) return invoke('restore_from_archive', { ids });
    const rs = LocalBackend.load();
    let n = 0;
    rs.forEach((r) => {
      if (ids.includes(r.id) && r.archived_at) {
        r.archived_at = null;
        r.exported_at = null;
        r.sync_batch = null;
        n++;
      }
    });
    LocalBackend.save(rs);
    return n;
  },
  /** 彻底删除（回收站专用） */
  async purge(ids) {
    if (IS_TAURI) return invoke('purge_records', { ids });
    LocalBackend.save(LocalBackend.load().filter((r) => !ids.includes(r.id)));
    return ids.length;
  },
  async get(id) {
    if (IS_TAURI) return invoke('get_record', { id });
    return LocalBackend.load().find((r) => r.id === id);
  },
  async save(rec) {
    if (IS_TAURI) return invoke('save_record', { record: rec });
    const rs = LocalBackend.load();
    const now = LocalBackend.now();
    if (!rec.id) {
      rec.id = LocalBackend.uid();
      rec.created_at = now;
    }
    rec.updated_at = now;
    rec.turns = rec.turns.filter((t) => t.content.trim() || (t.images || []).length);
    const i = rs.findIndex((r) => r.id === rec.id);
    if (i >= 0) rs[i] = rec;
    else rs.push(rec);
    LocalBackend.save(rs);
    return rec;
  },
  async remove(id) {
    if (IS_TAURI) return invoke('delete_record', { id });
    LocalBackend.save(LocalBackend.load().filter((r) => r.id !== id));
  },
  async saveImage(dataUrl) {
    const ext = (dataUrl.match(/data:image\/(\w+)/) || [, 'jpg'])[1];
    const b64 = dataUrl.slice(dataUrl.indexOf(',') + 1);
    if (IS_TAURI) return invoke('save_image', { ext, data_base64: b64 });
    const name = 'img_' + Math.random().toString(36).slice(2, 10) + '.' + ext;
    localStorage.setItem(IMG_PREFIX + name, dataUrl);
    return name;
  },
  imageUrl(name) {
    if (IS_TAURI) {
      const dir = state.config ? state.config.data_dir : '';
      return T.convertFileSrc(dir + '/images/' + name, 'asset');
    }
    return localStorage.getItem(IMG_PREFIX + name) || '';
  },
  async removeImage(name) {
    if (IS_TAURI) return invoke('delete_image', { name });
    localStorage.removeItem(IMG_PREFIX + name);
  },
  async resetExported() {
    if (IS_TAURI) return invoke('reset_exported');
    const rs = LocalBackend.load();
    rs.forEach((r) => (r.exported_at = null));
    LocalBackend.save(rs);
  },
  async export(opts) {
    if (IS_TAURI) return invoke('export_data', opts);
    // 浏览器兜底：直接生成文件下载
    let rs = LocalBackend.load();
    if (opts.scope === 'pending') rs = rs.filter((r) => !r.exported_at);
    if (!rs.length) throw new Error('没有符合条件的记录');
    const lines = rs.map((r) => buildLine(r, opts.format, opts.with_meta)).join('\n') + '\n';
    const enc = new TextEncoder();
    const base =
      opts.filename || (opts.format === 'pretrain' ? 'pretrain_t2t' : 'sft_t2t') + '_' + new Date().toISOString().slice(0, 10) + '.jsonl';
    const imgs = [];
    rs.forEach((r) => r.turns.forEach((t) => (t.images || []).forEach((n) => imgs.push(n))));
    if (imgs.length && opts.format !== 'pretrain') {
      const files = [{ name: base, data: enc.encode(lines) }];
      imgs.forEach((n) => {
        const du = localStorage.getItem(IMG_PREFIX + n);
        if (du) files.push({ name: 'images/' + n, data: dataURLToBytes(du) });
      });
      download(base.replace('.jsonl', '.zip'), makeZip(files));
    } else {
      download(base, new Blob([enc.encode(lines)], { type: 'application/jsonl' }));
    }
    const ids = rs.map((r) => r.id);
    const all = LocalBackend.load();
    if (opts.scope === 'pending') {
      const now = LocalBackend.now();
      all.forEach((r) => {
        if (ids.includes(r.id)) r.exported_at = now;
      });
      LocalBackend.save(all);
    }
    return { path: base, zip_path: null, count: rs.length, skipped: 0, has_images: imgs.length > 0 };
  },
  async mirror() {
    if (IS_TAURI) return invoke('mirror');
    return ['（浏览器模式下不支持镜像，请用导出）'];
  },
  async importText(text) {
    if (IS_TAURI) return invoke('import_text', { content: text });
    const rs = LocalBackend.load();
    const now = LocalBackend.now();
    let added = 0;
    let skipped = 0;
    for (const line of text.split('\n')) {
      const s = line.trim();
      if (!s) continue;
      let v;
      try {
        v = JSON.parse(s);
      } catch (e) {
        continue;
      }
      const rec = jsonToRecord(v);
      if (!rec) continue;
      rec.id = LocalBackend.uid();
      rec.created_at = now;
      rec.updated_at = now;
      rs.push(rec);
      added++;
    }
    LocalBackend.save(rs);
    return { added, skipped, images: 0 };
  },
  async importFile(path) {
    if (IS_TAURI) return invoke('import_file', { path });
    throw new Error('浏览器模式请选择文本导入');
  },
  async pickImportFile() {
    if (IS_TAURI) return invoke('pick_import_file');
    return null;
  },
  async pickExportDir() {
    if (IS_TAURI) return invoke('pick_export_dir');
    return null;
  },
  /* ---- GitHub 云端同步（需要 Rust 后端，浏览器兜底模式下不可用） ---- */
  ghGuard() {
    if (!IS_TAURI) throw new Error('云端同步需要装在电脑或手机上的 App，浏览器里用不了');
  },
  async ghConfig() {
    this.ghGuard();
    return invoke('github_get_config', {});
  },
  async ghSave(cfg) {
    this.ghGuard();
    return invoke('github_save_config', { cfg });
  },
  async ghDeviceStart(clientId) {
    this.ghGuard();
    return invoke('github_device_start', { clientId: clientId });
  },
  async ghDevicePoll(clientId, deviceCode) {
    this.ghGuard();
    return invoke('github_device_poll', { clientId: clientId, deviceCode: deviceCode });
  },
  async ghFinishLogin(token, clientId) {
    this.ghGuard();
    return invoke('github_finish_login', { token, clientId });
  },
  async ghLogout() {
    this.ghGuard();
    return invoke('github_logout', {});
  },
  async ghTest(cfg) {
    this.ghGuard();
    return invoke('github_test_connection', { cfg });
  },
  async ghEnsureRepo(cfg) {
    this.ghGuard();
    return invoke('github_ensure_repo', { cfg });
  },
  async ghVisibility(cfg, isPrivate) {
    this.ghGuard();
    return invoke('github_set_visibility', { cfg, private: isPrivate });
  },
  async ghSync(cfg, onProgress) {
    this.ghGuard();
    return invoke('github_sync', { cfg, channel: makeChannel(onProgress) });
  },
  async openUrl(url) {
    if (IS_TAURI) return invoke('open_url', { url });
    window.open(url, '_blank');
  },
  async reveal(path) {
    if (IS_TAURI) return invoke('reveal_in_finder', { path });
  },
  /* ---- 应用设置与分词器 ---- */
  async settings() {
    if (IS_TAURI) return invoke('get_app_settings');
    try {
      return JSON.parse(localStorage.getItem('coachsource.settings.v1')) || defaultSettings();
    } catch (e) {
      return defaultSettings();
    }
  },
  async saveSettings(s) {
    if (IS_TAURI) return invoke('save_app_settings', { settings: s });
    localStorage.setItem('coachsource.settings.v1', JSON.stringify(s));
  },
  async tokAnalyze(text) {
    if (IS_TAURI) return invoke('tokenizer_analyze', { text });
    return jsTokenize(text, jsVocabWords());
  },
  async tokVocab() {
    if (IS_TAURI) return invoke('tokenizer_vocab');
    return jsVocabWords()
      .sort((a, b) => a.word.localeCompare(b.word))
      .map((word) => ({ word, added_at: '' }));
  },
  async tokAddWords(words) {
    if (IS_TAURI) return invoke('tokenizer_add_words', { words });
    const vocab = jsVocabWords();
    const known = new Set(vocab.map((v) => v.word));
    let added = 0;
    for (const w of words) {
      const c = String(w).replace(/\s+/g, '');
      if (!c || c.length > 32 || known.has(c)) continue;
      known.add(c);
      vocab.push({ word: c, added_at: '' });
      added++;
    }
    localStorage.setItem('coachsource.vocab.v1', JSON.stringify(vocab));
    return added;
  },
  async tokRemoveWords(words) {
    if (IS_TAURI) return invoke('tokenizer_remove_words', { words });
    const rm = new Set(words.map((w) => String(w).replace(/\s+/g, '')));
    const vocab = jsVocabWords().filter((v) => !rm.has(v.word));
    localStorage.setItem('coachsource.vocab.v1', JSON.stringify(vocab));
    return words.length;
  },
  async tokImportTrie(content) {
    if (IS_TAURI) return invoke('tokenizer_import_trie', { content });
    const v = JSON.parse(content);
    if (!v || !v.root) throw new Error('不是字典树文件');
    const words = [];
    (function walk(node, prefix) {
      if (node.e === 1 && prefix) words.push(prefix);
      for (const [ch, child] of Object.entries(node.c || {})) walk(child, prefix + ch);
    })(v.root, '');
    if (!words.length) throw new Error('文件里一个词都没有');
    return B.tokAddWords(words);
  },
  async tokExportFile() {
    if (IS_TAURI) return invoke('tokenizer_export_file');
    // 浏览器兜底：直接生成下载
    const words = jsVocabWords().map((v) => v.word);
    if (!words.length) throw new Error('词汇表是空的');
    const trie = jsBuildTrieValue(words);
    download(
      'tokenizer_trie.json',
      new Blob([JSON.stringify(trie, null, 2)], { type: 'application/json' })
    );
    return 'tokenizer_trie.json（已开始下载）';
  },
};

/** Tauri 的进度通道：让 Rust 端能一边跑一边把进度推回来 */
function makeChannel(onMessage) {
  const id = T.transformCallback((msg) => {
    if (onMessage) onMessage(msg);
  }, true);
  return {
    __CHANNEL__: true,
    id,
    toJSON() {
      return `__CHANNEL__:${this.id}`;
    },
  };
}

/* ---------- 导出行构造（浏览器兜底用，逻辑与 Rust 端保持一致） ---------- */
function buildLine(rec, format, withMeta) {
  if (format === 'pretrain') {
    return JSON.stringify({ text: rec.turns.map((t) => t.content).join('\n') });
  }
  const conversations = [];
  const images = [];
  for (const t of rec.turns) {
    let content = '';
    for (const n of t.images || []) {
      content += '<image>\n';
      images.push('images/' + n);
    }
    content += t.content;
    const o = { role: t.role, content };
    if (t.reasoning_content) o.reasoning_content = t.reasoning_content;
    conversations.push(o);
  }
  const out = { conversations };
  if (images.length) out.images = images;
  if (withMeta) {
    out.id = rec.id;
    out.source = rec.source;
    out.tags = rec.tags;
    out.created_at = rec.created_at;
  }
  return JSON.stringify(out);
}

function jsonToRecord(v) {
  if (v && Array.isArray(v.conversations)) {
    const imgs = (v.images || []).map((s) => String(s).split('/').pop());
    const turns = v.conversations.map((c, i) => ({
      role: c.role || 'user',
      content: c.content || '',
      reasoning_content: c.reasoning_content || '',
      images: i === 0 && (c.role === 'user' || !c.role) ? imgs : [],
    }));
    return {
      id: '',
      title: '',
      source: v.source || 'import',
      tags: v.tags || [],
      turns,
      created_at: v.created_at || '',
      updated_at: '',
      exported_at: null,
    };
  }
  if (v && typeof v.text === 'string' && v.text.trim()) {
    return {
      id: '',
      title: '',
      source: 'pretrain',
      tags: [],
      turns: [{ role: 'assistant', content: v.text, reasoning_content: '', images: [] }],
      created_at: '',
      updated_at: '',
      exported_at: null,
    };
  }
  return null;
}

/* ============================================================
   状态与界面
   ============================================================ */
const state = {
  view: 'compose',
  config: null,
  editingId: null,
  turns: [],
  bucket: 'pending', // pending = 待导出；exported = 已导出；archived = 回收站
  selected: new Set(), // 列表里勾选的条目
  listIds: [],
  gh: null, // GitHub 配置（令牌、仓库…）
  ghReport: null, // 最近一次同步的结果
  tok: { view: false, full: false }, // 分词观察开关（view=逐轮观察，full=全开随机取色）
};

function blankTurn(role) {
  return { role, content: '', reasoning_content: '', images: [] };
}

function setView(v) {
  state.view = v;
  ['compose', 'library', 'export', 'sync', 'settings'].forEach((n) => {
    $('#view-' + n).classList.toggle('hidden', n !== v);
  });
  $$('.tabbar button').forEach((b) => b.classList.toggle('active', b.dataset.view === v));
  window.scrollTo(0, 0);
  if (v === 'library') refreshList();
  if (v === 'export') refreshStats();
  if (v === 'sync') refreshSyncPage();
  if (v === 'settings') refreshSettingsPage();
}

/* ---------- 轮次编辑区 ---------- */
function renderTurns() {
  const box = $('#turns');
  box.innerHTML = '';

  state.turns.forEach((turn, i) => {
    const el = document.createElement('div');
    el.className = 'turn';
    el.dataset.role = turn.role;

    const head = document.createElement('div');
    head.className = 'turn-head';

    const badge = document.createElement('button');
    badge.className = 'role-badge';
    badge.textContent = { user: '用户', assistant: '助手', system: '系统' }[turn.role];
    badge.title = '点击切换角色';
    badge.onclick = () => {
      const order = ['user', 'assistant', 'system'];
      turn.role = order[(order.indexOf(turn.role) + 1) % 3];
      saveDraft();
      renderTurns();
    };

    const spacer = document.createElement('div');
    spacer.className = 'spacer';

    const up = document.createElement('button');
    up.className = 'iconbtn';
    up.textContent = '↑';
    up.disabled = i === 0;
    up.onclick = () => {
      if (i === 0) return;
      const t = state.turns.splice(i, 1)[0];
      state.turns.splice(i - 1, 0, t);
      saveDraft();
      renderTurns();
    };

    const down = document.createElement('button');
    down.className = 'iconbtn';
    down.textContent = '↓';
    down.disabled = i === state.turns.length - 1;
    down.onclick = () => {
      if (i === state.turns.length - 1) return;
      const t = state.turns.splice(i, 1)[0];
      state.turns.splice(i + 1, 0, t);
      saveDraft();
      renderTurns();
    };

    const del = document.createElement('button');
    del.className = 'iconbtn';
    del.textContent = '删除';
    del.onclick = () => {
      state.turns.splice(i, 1);
      saveDraft();
      renderTurns();
    };

    head.append(badge, spacer, up, down, del);

    const ta = document.createElement('textarea');
    ta.placeholder = turn.role === 'assistant' ? '写下理想的回答…' : '写下问题…';
    ta.value = turn.content;
    ta.oninput = () => {
      turn.content = ta.value;
      updateChars();
      saveDraft();
      scheduleTokRender(); // 分词视图开着的时候，输入停下来就刷新着色
    };
    ta.onfocus = () => {
      state.lastTa = ta; // 「选中加词」要从最后聚焦的输入框取选区
    };

    const imgs = document.createElement('div');
    imgs.className = 'imgs';
    turn.images.forEach((name) => {
      const th = document.createElement('div');
      th.className = 'thumb';
      const im = document.createElement('img');
      im.src = B.imageUrl(name);
      im.alt = '';
      const x = document.createElement('button');
      x.className = 'del';
      x.textContent = '×';
      x.onclick = () => {
        turn.images = turn.images.filter((n) => n !== name);
        B.removeImage(name).catch(() => {});
        saveDraft();
        renderTurns();
      };
      th.append(im, x);
      imgs.appendChild(th);
    });

    // 手机上分「拍照」「图库」两个入口；桌面只有「+」走文件选择器
    const isMobileLayout = window.matchMedia('(max-width: 820px)').matches;
    if (isMobileLayout) {
      const camBtn = document.createElement('button');
      camBtn.className = 'ghost';
      camBtn.style.cssText = 'height:74px;padding:0 14px;font-size:13px;';
      camBtn.textContent = '拍照';
      camBtn.onclick = () => pickImage(turn, i, 'camera');
      const galBtn = document.createElement('button');
      galBtn.className = 'ghost';
      galBtn.style.cssText = 'height:74px;padding:0 14px;font-size:13px;';
      galBtn.textContent = '图库';
      galBtn.onclick = () => pickImage(turn, i, 'gallery');
      imgs.append(camBtn, galBtn);
    } else {
      const addImg = document.createElement('button');
      addImg.className = 'ghost';
      addImg.style.cssText = 'width:74px;height:74px;font-size:20px;';
      addImg.textContent = '+';
      addImg.title = '添加图片';
      addImg.onclick = () => pickImage(turn, i, 'gallery');
      imgs.appendChild(addImg);
    }

    const cotToggle = document.createElement('div');
    cotToggle.className = 'cot-toggle';
    cotToggle.textContent = '思维链（reasoning_content）▸ 点开填写';
    const cot = document.createElement('div');
    cot.className = 'cot hidden';
    const cotTa = document.createElement('textarea');
    cotTa.placeholder = '可选。训练带思维链的模型时才需要填。';
    cotTa.value = turn.reasoning_content;
    cotTa.oninput = () => {
      turn.reasoning_content = cotTa.value;
      saveDraft();
    };
    cot.appendChild(cotTa);
    cotToggle.onclick = () => {
      cot.classList.toggle('hidden');
      cotToggle.textContent = cot.classList.contains('hidden')
        ? '思维链（reasoning_content）▸ 点开填写'
        : '思维链（reasoning_content）▾ 收起';
      if (!cot.classList.contains('hidden')) cotTa.focus();
    };
    if (turn.reasoning_content) cotToggle.click();

    // 分词视图开着时，每个轮次正文下面挂一块只读的着色预览
    const tokView = document.createElement('div');
    tokView.className = 'tok-view hidden';
    el.append(head, ta, tokView, imgs, cotToggle, cot);
    box.appendChild(el);
  });

  updateChars();
  renderTokViews();
}

function pickImage(turn, index, mode) {
  // mode: 'camera' = 直接调系统相机拍一张；'gallery' = 系统文件选择器选图（可多选）。
  // Android 上由 WebView 的 onShowFileChooser 处理：带 capture 属性走相机 intent。
  const input = mode === 'camera' ? $('#file-camera') : $('#file-image');
  input.value = '';
  input.onchange = async () => {
    const files = Array.from(input.files || []);
    for (const f of files) {
      try {
        const du = await readFileAsDataURL(f);
        const name = await B.saveImage(du);
        state.turns[index].images.push(name);
      } catch (e) {
        toast('图片保存失败：' + e, false);
      }
    }
    saveDraft();
    renderTurns();
  };
  input.click();
}

/* ---------- 草稿：防止误关丢失 ---------- */
let draftTimer = null;
function saveDraft() {
  clearTimeout(draftTimer);
  draftTimer = setTimeout(() => {
    try {
      localStorage.setItem(
        LS_DRAFT,
        JSON.stringify({
          turns: state.turns,
          title: $('#f-title').value,
          tags: $('#f-tags').value,
          source: $('#f-source').value,
          editingId: state.editingId,
        })
      );
    } catch (e) {
      /* 忽略 */
    }
  }, 400);
}

function restoreDraft() {
  try {
    const raw = localStorage.getItem(LS_DRAFT);
    if (!raw) return false;
    const d = JSON.parse(raw);
    if (!d.turns || !d.turns.length) return false;
    state.turns = d.turns;
    state.editingId = d.editingId || null;
    $('#f-title').value = d.title || '';
    $('#f-tags').value = d.tags || '';
    $('#f-source').value = d.source || 'manual';
    return true;
  } catch (e) {
    return false;
  }
}

function clearDraft() {
  localStorage.removeItem(LS_DRAFT);
}

function resetCompose() {
  state.editingId = null;
  state.turns = [blankTurn('user'), blankTurn('assistant')];
  $('#f-title').value = '';
  $('#f-tags').value = '';
  $('#f-source').value = 'manual';
  clearDraft();
  renderTurns();
}

/* ---------- 保存 ---------- */
async function doSave(andNew) {
  const rec = {
    id: state.editingId || '',
    title: $('#f-title').value.trim(),
    source: $('#f-source').value,
    tags: $('#f-tags').value
      .split(/[,，]/)
      .map((s) => s.trim())
      .filter(Boolean),
    turns: state.turns,
    created_at: '',
    updated_at: '',
    exported_at: null,
  };
  try {
    const saved = await B.save(rec);
    toast(andNew ? '已保存，可以继续写下一条' : '已保存');
    if (andNew) resetCompose();
    else {
      state.editingId = saved.id;
      saveDraft();
    }
    refreshStats();
  } catch (e) {
    toast('保存失败：' + e, false);
  }
}

/* ---------- 列表 ---------- */
const BUCKET_TEXT = {
  pending: {
    empty: '待导出是空的。去「录入」记一条，或从「已导出」「回收站」恢复回来。',
    hint: '新记的对话都在这里。本地导出后移到「已导出」，同步上云后进入「回收站」。',
  },
  exported: {
    empty: '这里还没有数据。本地导出过的对话会出现在这里。',
    hint: '已经导出成本地文件、但还没同步上云的对话。勾选可恢复到待导出；下次同步也会带上它们。',
  },
  archived: {
    empty: '回收站是空的。同步上云之后的对话会移到这里。',
    hint: '已经同步到云端并归档的对话。正文完整保留在本地，可恢复、也可彻底删除。',
  },
};

async function refreshList() {
  const q = $('#f-search').value;
  const bucket = state.bucket;
  try {
    const items = await B.list(q, bucket);
    state.listIds = items.map((i) => i.id);
    const box = $('#list');
    box.innerHTML = '';

    const empty = $('#list-empty');
    empty.classList.toggle('hidden', items.length > 0);
    empty.textContent = (BUCKET_TEXT[bucket] || BUCKET_TEXT.pending).empty;

    $('#bucket-hint').textContent = (BUCKET_TEXT[bucket] || BUCKET_TEXT.pending).hint;

    items.forEach((it) => {
      const el = document.createElement('div');
      el.className = 'item';
      el.onclick = (ev) => {
        if (ev.target.closest('button') || ev.target.closest('input')) return;
        openRecord(it.id);
      };

      // 已导出和回收站里才出现勾选框
      if (bucket === 'exported' || bucket === 'archived') {
        const cb = document.createElement('input');
        cb.type = 'checkbox';
        cb.className = 'item-check';
        cb.checked = state.selected.has(it.id);
        cb.onclick = (ev) => ev.stopPropagation();
        cb.onchange = () => {
          if (cb.checked) state.selected.add(it.id);
          else state.selected.delete(it.id);
          updateTrashBar();
        };
        el.appendChild(cb);
      }

      const head = document.createElement('div');
      head.className = 'item-head';
      const title = document.createElement('div');
      title.className = 'item-title';
      title.textContent = it.display_title || displayTitle({ turns: [] });
      const time = document.createElement('div');
      time.className = 'item-time';
      time.textContent = (it.updated_at || '').slice(5, 16);
      head.append(title, time);

      const pv = document.createElement('p');
      pv.className = 'item-preview';
      pv.textContent = it.preview || '';
      if (!it.preview) pv.style.display = 'none';

      const meta = document.createElement('div');
      meta.className = 'item-meta';
      meta.appendChild(span(`${it.turn_count} 轮`));
      const ch = span(fmtChars(it.chars));
      ch.className = 'chars-tag';
      meta.appendChild(ch);
      if (it.image_count) meta.appendChild(span(`${it.image_count} 图`));
      (it.tags || []).forEach((t) => {
        const s = span(t);
        s.className = 'tag';
        meta.appendChild(s);
      });
      if (it.sync_batch) {
        const s = span('云端 ' + it.sync_batch);
        s.className = 'tag batch';
        meta.appendChild(s);
      }

      const body = document.createElement('div');
      body.className = 'item-body';
      body.append(head, pv, meta);
      el.appendChild(body);
      box.appendChild(el);
    });

    updateTrashBar();
  } catch (e) {
    toast('读取列表失败：' + e, false);
  }
}

function updateTrashBar() {
  const n = state.selected.size;
  const b = state.bucket;
  const show = n > 0 && (b === 'exported' || b === 'archived');
  $('#trash-bar').classList.toggle('hidden', !show);
  $('#sel-count').textContent = n;
  const btn = $('#btn-restore');
  if (btn) btn.textContent = b === 'archived' ? '恢复到待导出' : '恢复到待导出';
  const del = $('#btn-del-sel');
  if (del) del.textContent = b === 'archived' ? '彻底删除' : '删除';
}

/** 打开一条对话继续往下写：载入全部轮次，光标落在最后一轮 */
async function openRecord(id) {
  try {
    const r = await B.get(id);
    if (!r) {
      toast('这条记录已经不在了', false);
      return;
    }
    state.editingId = r.id;
    state.turns = r.turns.map((t) => ({
      role: t.role,
      content: t.content,
      reasoning_content: t.reasoning_content || '',
      images: t.images || [],
    }));
    $('#f-title').value = r.title || '';
    $('#f-tags').value = (r.tags || []).join(',');
    $('#f-source').value = r.source || 'manual';
    renderTurns();
    setView('compose');
    setTimeout(() => {
      const tas = $$('#turns textarea');
      if (tas.length) {
        const last = tas[tas.length - 1];
        last.scrollIntoView({ block: 'center', behavior: 'smooth' });
        last.focus({ preventScroll: true });
      }
    }, 80);
  } catch (e) {
    toast('打开失败：' + e, false);
  }
}

function span(text) {
  const s = document.createElement('span');
  s.textContent = text;
  return s;
}

/* ---------- 统计 ---------- */
async function refreshStats() {
  try {
    const s = await B.stats();
    $('#stats').innerHTML =
      `<span>共 <b>${s.total}</b> 条</span>` +
      `<span>待导出 <b>${s.pending}</b></span>` +
      `<span><b>${fmtChars(s.total_chars)}</b></span>`;
    const cp = $('#cnt-pending');
    const ce = $('#cnt-exported');
    const ca = $('#cnt-archived');
    if (cp) cp.textContent = s.pending;
    if (ce) ce.textContent = s.exported;
    if (ca) ca.textContent = s.archived || 0;
  } catch (e) {
    /* 忽略 */
  }
}

/** 录入区实时显示本条字数 */
function updateChars() {
  const el = $('#compose-chars');
  if (!el) return;
  const n = state.turns.reduce((a, t) => a + (t.content || '').length, 0);
  el.textContent = '本条 ' + fmtChars(n);
}

/* ---------- 云端同步 ---------- */

/** 把表单上的输入拼成一份完整配置（令牌只存在内存里，不进输入框） */
function currentGhConfig() {
  const base = state.gh || {};
  const mb = parseInt($('#gh-shard').value, 10);
  const shard = Number.isFinite(mb) ? Math.min(95, Math.max(1, mb)) : 50;
  return {
    client_id: ($('#gh-client-id').value || '').trim(),
    token: base.token || '',
    login: base.login || '',
    owner: ($('#gh-owner').value || '').trim() || base.login || '',
    repo: ($('#gh-repo').value || '').trim() || 'coach-source-data',
    branch: ($('#gh-branch').value || '').trim() || 'main',
    shard_mb: shard,
    include_images: $('#gh-images').checked,
    private: $('#gh-private').checked,
  };
}

async function refreshSyncPage() {
  if (!IS_TAURI) {
    $('#gh-syncable').textContent = '云端同步需要装在电脑或手机上的 App。';
    return;
  }
  try {
    state.gh = await B.ghConfig();
    const c = state.gh;
    $('#gh-client-id').value = c.client_id || '';
    $('#gh-repo').value = c.repo || 'coach-source-data';
    $('#gh-branch').value = c.branch || 'main';
    $('#gh-owner').value = c.owner || '';
    $('#gh-private').checked = !!c.private;
    $('#gh-shard').value = c.shard_mb || 50;
    $('#gh-images').checked = !!c.include_images;

    const logged = !!(c.token && c.login);
    $('#gh-logged-in').classList.toggle('hidden', !logged);
    $('#gh-logged-out').classList.toggle('hidden', logged);
    if (logged) $('#gh-login-name').textContent = c.login;

    const s = await B.stats();
    $('#gh-syncable').textContent = logged
      ? `这次会同步 ${s.syncable} 条（待导出 ${s.pending} + 已导出 ${s.exported}）。回收站里 ${s.archived} 条已经上过云，不会重复传。`
      : `本地有 ${s.syncable} 条待同步。先登录 GitHub。`;
    if (!logged) $('#gh-repo-result').textContent = '—';
  } catch (e) {
    $('#gh-syncable').textContent = '读取配置失败：' + e;
  }
}

let deviceTimer = null;
let deviceStopped = false;

/** 设备码登录：拿码 → 自动开浏览器 → 轮询等授权 */
async function startDeviceLogin() {
  const cid = ($('#gh-client-id').value || '').trim();
  deviceStopped = false;
  try {
    const d = await B.ghDeviceStart(cid);
    $('#gh-user-code').textContent = d.user_code;
    $('#gh-verify-uri').textContent = d.verification_uri;
    $('#gh-device').classList.remove('hidden');
    $('#gh-logged-out').classList.add('hidden');
    $('#gh-waiting').textContent = `等待你在网页上确认…（${Math.round(d.expires_in / 60)} 分钟内有效）`;
    try {
      await B.openUrl(d.verification_uri);
    } catch (e) {
      /* 打不开就让用户自己点链接 */
    }
    pollDevice(cid, d.device_code, d.interval, d.expires_in);
  } catch (e) {
    toast('登录失败：' + e, false);
  }
}

function pollDevice(cid, deviceCode, interval, expiresIn) {
  const started = Date.now();
  const tick = async () => {
    if (deviceStopped) return;
    if (Date.now() - started > expiresIn * 1000) {
      $('#gh-waiting').textContent = '授权码过期了，请重新点「用 GitHub 登录」。';
      return;
    }
    try {
      const r = await B.ghDevicePoll(cid, deviceCode);
      if (r.status === 'ok') {
        state.gh = await B.ghFinishLogin(r.token, cid);
        await B.ghSave(currentGhConfig());
        $('#gh-device').classList.add('hidden');
        toast('登录成功：' + state.gh.login);
        // GitHub App 的令牌不认 scope=repo（返回 scope 为空），缺 repo 就建不了仓库
        const scopes = (r.scope || '').split(/[,\s]+/).filter(Boolean);
        if (!scopes.includes('repo')) {
          setTimeout(() => toast('警告：这次授权没拿到 repo 权限，之后无法创建/推送仓库。'
            + '常见原因是 Client ID 来自「GitHub Apps」而不是「OAuth Apps」。', false), 1200);
        }
        refreshSyncPage();
        return;
      }
      if (r.status === 'expired') {
        $('#gh-waiting').textContent = '授权码过期了，请重新点「用 GitHub 登录」。';
        return;
      }
      if (r.status === 'denied') {
        $('#gh-waiting').textContent = '你在网页上取消了授权。';
        return;
      }
      const wait = r.status === 'slow_down' ? interval + 5 : interval;
      deviceTimer = setTimeout(tick, wait * 1000);
    } catch (e) {
      $('#gh-waiting').textContent = '轮询失败：' + e;
    }
  };
  tick();
}

function cancelDeviceLogin() {
  deviceStopped = true;
  clearTimeout(deviceTimer);
  $('#gh-device').classList.add('hidden');
  $('#gh-logged-out').classList.remove('hidden');
}

async function ghCheckRepo() {
  try {
    const cfg = currentGhConfig();
    await B.ghSave(cfg);
    const r = await B.ghTest(cfg);
    if (r.repo_exists) {
      if (r.default_branch && r.default_branch !== cfg.branch) {
        $('#gh-branch').value = r.default_branch;
      }
      $('#gh-private').checked = !!r.private;
      $('#gh-repo-result').textContent =
        `仓库 ${r.full_name} 存在（${r.private ? '私有' : '公开'}，默认分支 ${r.default_branch}）`;
      toast('仓库正常');
    } else {
      $('#gh-repo-result').textContent =
        `账号 ${r.login} 登录正常，但仓库还不存在：${r.repo_error}。点「创建仓库」建一个。`;
    }
  } catch (e) {
    $('#gh-repo-result').textContent = '检查失败：' + e;
    toast('检查失败：' + e, false);
  }
}

async function ghCreateRepo() {
  try {
    const cfg = currentGhConfig();
    await B.ghSave(cfg);
    const r = await B.ghEnsureRepo(cfg);
    if (r.default_branch) $('#gh-branch').value = r.default_branch;
    $('#gh-repo-result').textContent =
      (r.created ? '已创建：' : '已存在：') + r.full_name + '  ' + r.html_url;
    toast(r.created ? '仓库已创建' : '仓库已存在');
  } catch (e) {
    $('#gh-repo-result').textContent = '创建失败：' + e;
    toast('创建失败：' + e, false);
  }
}

async function ghApplyVisibility() {
  const cfg = currentGhConfig();
  const wantPrivate = $('#gh-private').checked;
  if (!wantPrivate) {
    const ok = confirm(
      '确定要把仓库设为公开吗？\n\n' +
        '任何人不需要登录就能看到并下载全部对话；内容可能被搜索引擎抓取、被别人 fork，' +
        '删除之后也不保证能清干净。这一步是不可逆的。'
    );
    if (!ok) {
      $('#gh-private').checked = true;
      return;
    }
  }
  try {
    const msg = await B.ghVisibility(cfg, wantPrivate);
    $('#gh-repo-result').textContent = msg;
    toast(msg);
  } catch (e) {
    toast('设置失败：' + e, false);
  }
}

async function runSync() {
  const cfg = currentGhConfig();
  if (!cfg.token) {
    toast('请先登录 GitHub', false);
    return;
  }
  const btn = $('#btn-gh-sync');
  btn.disabled = true;
  $('#gh-report').classList.add('hidden');
  $('#gh-progress').classList.remove('hidden');
  $('#gh-bar').style.width = '0%';
  $('#gh-phase').textContent = '准备中…';

  try {
    const rep = await B.ghSync(cfg, (p) => {
      const total = Math.max(1, p.total);
      $('#gh-bar').style.width = Math.min(100, (p.current / total) * 100) + '%';
      $('#gh-phase').textContent = p.text;
    });
    state.ghReport = rep;
    showSyncReport(rep);
    toast('同步完成，已归档进回收站');
    refreshStats();
  } catch (e) {
    $('#gh-phase').textContent = '同步失败：' + e;
    toast('同步失败：' + e, false);
  } finally {
    btn.disabled = false;
  }
}

function showSyncReport(rep) {
  const files = rep.shards.concat(rep.image_shards);
  const mb = (rep.total_bytes / 1024 / 1024).toFixed(1);
  $('#gh-report-sum').textContent =
    `批次 ${rep.batch} · ${rep.record_count} 条 · ${files.length} 个文件 · 共 ${mb} MB · ` +
    `云端校验${rep.verified ? '全部通过' : '未通过'} · 已归档 ${rep.archived} 条`;

  const box = $('#gh-files');
  box.innerHTML = '';
  files.forEach((s) => {
    const row = document.createElement('div');
    row.className = 'filerow';
    const name = document.createElement('span');
    name.className = 'fname';
    name.textContent = s.name;
    const size = document.createElement('span');
    size.className = 'fsize';
    size.textContent = (s.bytes / 1024 / 1024).toFixed(2) + ' MB';
    const a = document.createElement('a');
    a.className = 'flink';
    a.href = s.url;
    a.target = '_blank';
    a.rel = 'noreferrer';
    a.textContent = '下载';
    row.append(name, size, a);
    box.appendChild(row);
  });
  $('#gh-report').classList.remove('hidden');
}

async function copyLinks() {
  const rep = state.ghReport;
  if (!rep) return;
  const links = rep.shards
    .concat(rep.image_shards)
    .concat(rep.tokenizer_trie ? [rep.tokenizer_trie] : [])
    .map((s) => s.url)
    .join('\n');
  try {
    await navigator.clipboard.writeText(links);
    $('#gh-copy-result').textContent = '已复制全部下载链接到剪贴板';
  } catch (e) {
    const ta = document.createElement('textarea');
    ta.value = links;
    ta.rows = Math.min(12, links.split('\n').length + 1);
    ta.className = 'hint mono';
    const r = $('#gh-copy-result');
    r.textContent = '浏览器不让自动复制，请手动复制下面这些：';
    r.appendChild(ta);
  }
}

  /^\s*(#{1,6}\s*)?(assistant|ai|gpt|chatgpt|claude|助手|答|回答|模型)\s*[:：]|^\s*[【\[]\s*(assistant|ai|助手)\s*[】\]]/i;

const USER_RE =
  /^\s*(#{1,6}\s*)?(user|human|用户|我|问|提问)\s*[:：]|^\s*[【\[]\s*(user|human|用户|我)\s*[】\]]/i;
const ASSIST_RE =
  /^\s*(#{1,6}\s*)?(assistant|ai|gpt|chatgpt|claude|助手|答|回答|模型)\s*[:：]|^\s*[【\[]\s*(assistant|ai|助手)\s*[】\]]/i;

function splitConversation(text) {
  const lines = text.split(/\r?\n/);
  const out = [];
  let cur = null;

  const push = (role, firstLine) => {
    cur = { role, content: firstLine || '' };
    out.push(cur);
  };

  for (const line of lines) {
    if (USER_RE.test(line)) {
      push('user', line.replace(USER_RE, '').trim());
    } else if (ASSIST_RE.test(line)) {
      push('assistant', line.replace(ASSIST_RE, '').trim());
    } else if (/^\s*(system|系统提示|系统)\s*[:：]/i.test(line)) {
      push('system', line.replace(/^\s*(system|系统提示|系统)\s*[:：]/i, '').trim());
    } else if (cur) {
      cur.content += (cur.content ? '\n' : '') + line;
    } else {
      push('user', line);
    }
  }
  return out.map((t) => ({ ...blankTurn(t.role), content: t.content.trim() }));
}

/* ============================================================
   分词器
   默认关闭。开启后录入页出现「观察分词」：
   没被词汇覆盖的字用设置里的单一颜色着色（可点击加词），
   「全开」模式下每个已覆盖 token 着随机色（同一 token 同色）。
   ============================================================ */

function defaultSettings() {
  return { tokenizer_enabled: false, uncovered_color: '#e06c75' };
}

/* ---------- 浏览器兜底：与 Rust 端 tokenizer.rs 同一套算法 ---------- */

function jsVocabWords() {
  try {
    return JSON.parse(localStorage.getItem('coachsource.vocab.v1')) || [];
  } catch (e) {
    return [];
  }
}

function jsBuildTrie(words) {
  const root = { c: new Map(), w: false };
  for (const word of words) {
    let node = root;
    for (const ch of word) {
      if (!node.c.has(ch)) node.c.set(ch, { c: new Map(), w: false });
      node = node.c.get(ch);
    }
    node.w = true;
  }
  return root;
}

/** 最大前缀匹配，结果结构与 Rust 端 TokenizeResult 一致 */
function jsTokenize(text, vocab) {
  const root = jsBuildTrie(vocab.map((v) => v.word));
  const chars = Array.from(text);
  const spans = [];
  let covered = 0;
  let i = 0;
  while (i < chars.length) {
    let best = -1;
    let node = root.c.get(chars[i]);
    let j = i;
    while (node) {
      j++;
      if (node.w) best = j;
      if (j >= chars.length) break;
      node = node.c.get(chars[j]);
    }
    const end = best > 0 ? best : i + 1;
    const cov = best > 0;
    if (cov) covered += end - i;
    spans.push({ start: i, end, text: chars.slice(i, end).join(''), covered: cov });
    i = end;
  }
  return {
    spans,
    total_chars: chars.length,
    covered_chars: covered,
    ratio: chars.length ? covered / chars.length : 1,
  };
}

/** 生成与 Rust 端一致的字典树导出结构 */
function jsBuildTrieValue(words) {
  const root = jsBuildTrie(words);
  const conv = (node) => {
    const c = {};
    for (const [ch, child] of node.c) c[ch] = conv(child);
    return { e: node.w ? 1 : 0, c };
  };
  return {
    format: 'coachsource-tokenizer-trie',
    version: 1,
    created_at: new Date().toISOString().replace('T', ' ').slice(0, 19),
    word_count: words.length,
    root: conv(root),
  };
}

/* ---------- 着色 ---------- */

/** 同一 token 永远同色：把 token 文本哈希到色环上 */
function tokenColor(text) {
  let h = 0;
  for (let i = 0; i < text.length; i++) h = (h * 31 + text.charCodeAt(i)) >>> 0;
  return `hsl(${h % 360} 70% 64%)`;
}

function tokEnabled() {
  return !!(state.settings && state.settings.tokenizer_enabled);
}

function applyTokBar() {
  const on = tokEnabled();
  $('#tok-bar').classList.toggle('hidden', !on);
  if (!on) {
    state.tok.view = false;
    state.tok.full = false;
  }
  $('#btn-tok-view').classList.toggle('active', on && state.tok.view);
  $('#btn-tok-full').classList.toggle('hidden', !(on && state.tok.view));
  $('#btn-tok-addsel').classList.toggle('hidden', !(on && state.tok.view));
}

let tokTimer = null;
function scheduleTokRender() {
  if (!state.tok.view || !tokEnabled()) return;
  clearTimeout(tokTimer);
  tokTimer = setTimeout(renderTokViews, 350);
}

/** 把每个轮次的正文渲染成着色的 token 序列 */
async function renderTokViews() {
  const show = state.tok.view && tokEnabled();
  const views = $$('#turns .tok-view');
  views.forEach((v) => v.classList.add('hidden'));
  if (!show || !views.length) return;

  const color = (state.settings && state.settings.uncovered_color) || '#e06c75';
  let totalCov = 0;
  let totalChars = 0;

  for (let i = 0; i < views.length; i++) {
    const el = views[i];
    const text = ((state.turns || [])[i] || {}).content || '';
    el.classList.remove('hidden');
    el.textContent = '…';
    if (!text.trim()) {
      el.textContent = '（空，无内容可分词）';
      continue;
    }
    try {
      const r = await B.tokAnalyze(text);
      totalCov += r.covered_chars;
      totalChars += r.total_chars;
      el.textContent = '';
      for (const s of r.spans) {
        const sp = document.createElement('span');
        sp.textContent = s.text;
        if (!s.covered) {
          sp.className = 'tok-uncovered';
          sp.style.color = color;
          sp.title = `「${s.text}」不在词汇表里，点击加入`;
          sp.onclick = () => addVocabWords([s.text]);
        } else if (state.tok.full) {
          sp.style.color = tokenColor(s.text);
        }
        el.appendChild(sp);
      }
    } catch (e) {
      el.textContent = '分词失败：' + e;
    }
  }
  const pct = totalChars ? ((totalCov / totalChars) * 100).toFixed(1) : '0.0';
  $('#tok-ratio').textContent = `覆盖率 ${pct}%`;
}

async function addVocabWords(words) {
  const list = words.map((w) => String(w)).filter((w) => w.trim());
  if (!list.length) return;
  try {
    const n = await B.tokAddWords(list);
    toast(n ? `已加入 ${n} 个词` : '这些词已经在词汇表里了');
    if (n) {
      renderTokViews();
      if (state.view === 'settings') renderVocabList();
    }
  } catch (e) {
    toast('加词失败：' + e, false);
  }
}

/* ---------- 设置页 ---------- */

async function refreshSettingsPage() {
  if (!state.settings) state.settings = await B.settings().catch(() => defaultSettings());
  const s = state.settings;
  $('#s-tok-enabled').checked = !!s.tokenizer_enabled;
  const c = s.uncovered_color || '#e06c75';
  $('#s-tok-color').value = c;
  $('#s-tok-color-hex').value = c;
  renderVocabList();
}

async function renderVocabList() {
  try {
    const words = await B.tokVocab();
    $('#s-vocab-count').textContent = `共 ${words.length} 个词`;
    const box = $('#s-vocab-list');
    box.innerHTML = '';
    if (!words.length) {
      box.innerHTML = '<p class="hint">还没有词。可以在录入页选中一段文字加入，或在分词视图里点未覆盖的字。</p>';
      return;
    }
    words.forEach((w) => {
      const item = document.createElement('div');
      item.className = 'vocab-item';
      const name = document.createElement('span');
      name.className = 'vocab-word';
      name.textContent = w.word;
      const del = document.createElement('button');
      del.className = 'vocab-del';
      del.textContent = '×';
      del.title = '从词汇表删除';
      del.onclick = async () => {
        try {
          await B.tokRemoveWords([w.word]);
          toast(`已删除「${w.word}」`);
          renderVocabList();
          renderTokViews();
        } catch (e) {
          toast('删除失败：' + e, false);
        }
      };
      item.append(name, del);
      box.appendChild(item);
    });
  } catch (e) {
    $('#s-vocab-count').textContent = '词汇表读取失败：' + e;
  }
}

let settingsTimer = null;
function saveSettingsSoon(silent) {
  clearTimeout(settingsTimer);
  settingsTimer = setTimeout(() => saveSettingsNow(silent), 400);
}

async function saveSettingsNow(silent) {
  const s = {
    tokenizer_enabled: $('#s-tok-enabled').checked,
    uncovered_color: ($('#s-tok-color-hex').value.trim() || '#e06c75').toLowerCase(),
  };
  try {
    await B.saveSettings(s);
    state.settings = s;
    applyTokBar();
    renderTokViews();
    if (!silent) toast('设置已保存');
  } catch (e) {
    toast('保存失败：' + e, false);
  }
}

/* ============================================================
   事件绑定
   ============================================================ */
function bind() {
  $$('.tabbar button').forEach((b) => {
    b.onclick = () => setView(b.dataset.view);
  });

  $$('[data-add]').forEach((b) => {
    b.onclick = () => {
      state.turns.push(blankTurn(b.dataset.add));
      saveDraft();
      renderTurns();
      const tas = $$('#turns textarea');
      if (tas.length) tas[tas.length - 1].focus();
    };
  });

  $('#btn-save').onclick = () => doSave(false);
  $('#btn-save-new').onclick = () => doSave(true);
  $('#btn-clear').onclick = () => {
    if (!confirm('清空当前编辑内容？')) return;
    resetCompose();
  };

  $('#btn-toggle-paste').onclick = () => {
    const a = $('#paste-area');
    a.classList.toggle('hidden');
    if (!a.classList.contains('hidden')) $('#paste-text').focus();
  };
  $('#btn-paste-cancel').onclick = () => {
    $('#paste-area').classList.add('hidden');
    $('#paste-text').value = '';
  };
  $('#btn-paste').onclick = () => {
    const parsed = splitConversation($('#paste-text').value);
    if (!parsed.length) {
      toast('没解析出内容', false);
      return;
    }
    const keep = state.turns.filter((t) => t.content.trim() || t.images.length);
    state.turns = keep.concat(parsed);
    $('#paste-area').classList.add('hidden');
    $('#paste-text').value = '';
    saveDraft();
    renderTurns();
    toast(`解析出 ${parsed.length} 轮`);
  };

  $('#f-search').oninput = () => {
    clearTimeout(window._srch);
    window._srch = setTimeout(refreshList, 250);
  };

  $$('.seg button').forEach((b) => {
    b.onclick = () => {
      state.bucket = b.dataset.bucket;
      state.selected.clear();
      $$('.seg button').forEach((x) => x.classList.toggle('active', x === b));
      refreshList();
    };
  });

  $('#btn-restore').onclick = async () => {
    const ids = Array.from(state.selected);
    if (!ids.length) return;
    try {
      const n =
        state.bucket === 'archived' ? await B.restoreFromArchive(ids) : await B.restore(ids);
      state.selected.clear();
      toast(`已恢复 ${n} 条到待导出`);
      refreshList();
      refreshStats();
    } catch (e) {
      toast('恢复失败：' + e, false);
    }
  };

  $('#btn-sel-all').onclick = () => {
    state.listIds.forEach((id) => state.selected.add(id));
    refreshList();
  };

  $('#btn-sel-none').onclick = () => {
    state.selected.clear();
    refreshList();
  };

  $('#btn-del-sel').onclick = async () => {
    const ids = Array.from(state.selected);
    if (!ids.length) return;
    if (!confirm(`确定删除选中的 ${ids.length} 条？删掉就找不回来了。`)) return;
    for (const id of ids) {
      await B.remove(id);
    }
    state.selected.clear();
    toast('已删除');
    refreshList();
    refreshStats();
  };

  $('#btn-export').onclick = () => runExport(null);
  $('#btn-export-to').onclick = async () => {
    try {
      const dir = await B.pickExportDir();
      if (dir) runExport(dir);
    } catch (e) {
      toast('选择目录失败：' + e, false);
    }
  };

  $('#btn-mirror').onclick = async () => {
    try {
      const paths = await B.mirror();
      $('#e-result').textContent = '已写入：' + paths.join('  —  ');
      toast('镜像完成');
    } catch (e) {
      toast('镜像失败：' + e, false);
    }
  };

  $('#btn-open-dir').onclick = async () => {
    if (!state.config) return;
    await B.reveal(state.config.export_dir);
  };

  $('#btn-import-file').onclick = async () => {
    if (IS_TAURI) {
      try {
        const p = await B.pickImportFile();
        if (!p) return;
        const r = await B.importFile(p);
        $('#i-result').textContent = `新增 ${r.added} 条，跳过重复 ${r.skipped} 条，图片 ${r.images} 张`;
        toast('导入完成');
        refreshStats();
      } catch (e) {
        toast('导入失败：' + e, false);
      }
    } else {
      $('#file-import').click();
    }
  };

  $('#file-import').onchange = async () => {
    const f = $('#file-import').files[0];
    if (!f) return;
    try {
      const text = await readFileAsText(f);
      const r = await B.importText(text);
      $('#i-result').textContent = `新增 ${r.added} 条，跳过重复 ${r.skipped} 条`;
      toast('导入完成');
      refreshStats();
    } catch (e) {
      toast('导入失败：' + e, false);
    }
  };

  $('#btn-toggle-text').onclick = () => {
    $('#import-area').classList.toggle('hidden');
  };

  $('#btn-import-text').onclick = async () => {
    const text = $('#import-text').value;
    if (!text.trim()) return;
    try {
      const r = await B.importText(text);
      $('#i-result').textContent = `新增 ${r.added} 条，跳过重复 ${r.skipped} 条`;
      $('#import-text').value = '';
      toast('导入完成');
      refreshStats();
    } catch (e) {
      toast('导入失败：' + e, false);
    }
  };

  $('#btn-backup').onclick = async () => {
    try {
      const r = await B.export({
        format: 'sft',
        scope: 'all',
        with_meta: true,
        filename: 'coachsource_backup_' + new Date().toISOString().slice(0, 10) + '.jsonl',
        target_dir: null,
      });
      $('#e-result').textContent = `备份完成，共 ${r.count} 条${r.zip_path ? '（含图片，已打包 zip）' : ''}：${r.zip_path || r.path}`;
      toast('备份完成');
    } catch (e) {
      toast('备份失败：' + e, false);
    }
  };

  $('#btn-reset-exported').onclick = async () => {
    if (!confirm('把所有记录重新标成「未导出」？下次导出会全部再导一遍。')) return;
    await B.resetExported();
    toast('已重置');
    refreshStats();
  };

  /* ---- 云端同步 ---- */
  $('#btn-gh-login').onclick = startDeviceLogin;
  $('#btn-gh-cancel').onclick = cancelDeviceLogin;
  $('#btn-gh-open').onclick = () => B.openUrl($('#gh-verify-uri').textContent.trim());
  $('#btn-gh-logout').onclick = async () => {
    if (!confirm('退出登录？仓库设置会保留，下次同步需要重新登录。')) return;
    try {
      state.gh = await B.ghLogout();
      refreshSyncPage();
      toast('已退出登录');
    } catch (e) {
      toast('退出失败：' + e, false);
    }
  };
  $('#btn-gh-test').onclick = ghCheckRepo;
  $('#btn-gh-create').onclick = ghCreateRepo;
  $('#btn-gh-apply-vis').onclick = ghApplyVisibility;
  $('#btn-gh-sync').onclick = runSync;
  $('#btn-gh-open-repo').onclick = () => {
    if (state.ghReport) B.openUrl(state.ghReport.web_url);
  };
  $('#btn-gh-copy-links').onclick = copyLinks;

  /* ---- 分词器 ---- */
  $('#btn-tok-view').onclick = () => {
    state.tok.view = !state.tok.view;
    if (!state.tok.view) state.tok.full = false;
    applyTokBar();
    renderTokViews();
  };
  $('#btn-tok-full').onclick = () => {
    state.tok.full = !state.tok.full;
    $('#btn-tok-full').classList.toggle('active', state.tok.full);
    renderTokViews();
  };
  $('#btn-tok-addsel').onclick = () => {
    const ta = state.lastTa;
    if (!ta) {
      toast('先在一个输入框里选中一段文字', false);
      return;
    }
    const sel = ta.value.substring(ta.selectionStart, ta.selectionEnd).trim();
    if (!sel) {
      toast('先选中一段文字（拖蓝或双击选词）', false);
      return;
    }
    addVocabWords(sel.split(/\s+/));
  };

  /* ---- 设置页 ---- */
  $('#s-tok-enabled').onchange = () => saveSettingsNow(false);
  $('#s-tok-color').oninput = () => {
    $('#s-tok-color-hex').value = $('#s-tok-color').value;
    saveSettingsSoon(true);
  };
  $('#s-tok-color-hex').onchange = () => {
    let v = $('#s-tok-color-hex').value.trim();
    if (!/^#[0-9a-fA-F]{6}$/.test(v)) {
      toast('颜色格式不对，应该是 #e06c75 这样', false);
      $('#s-tok-color-hex').value = $('#s-tok-color').value;
      return;
    }
    $('#s-tok-color').value = v;
    saveSettingsNow(false);
  };
  const addFromInput = () => {
    const v = $('#s-vocab-new').value.trim();
    if (!v) return;
    addVocabWords(v.split(/[\s,，、]+/)).then(() => {
      $('#s-vocab-new').value = '';
    });
  };
  $('#s-vocab-add').onclick = addFromInput;
  $('#s-vocab-new').onkeydown = (e) => {
    if (e.key === 'Enter') addFromInput();
  };
  $('#s-vocab-import').onclick = () => $('#file-trie').click();
  $('#file-trie').onchange = async () => {
    const f = $('#file-trie').files[0];
    if (!f) return;
    try {
      const text = await readFileAsText(f);
      const n = await B.tokImportTrie(text);
      $('#s-vocab-result').textContent = `导入完成，新增 ${n} 个词`;
      toast('导入完成');
      renderVocabList();
    } catch (e) {
      $('#s-vocab-result').textContent = '导入失败：' + e;
      toast('导入失败：' + e, false);
    }
  };
  $('#s-vocab-export').onclick = async () => {
    try {
      const p = await B.tokExportFile();
      $('#s-vocab-result').textContent = '已导出：' + p;
      toast('字典树已导出');
    } catch (e) {
      $('#s-vocab-result').textContent = '';
      toast('导出失败：' + e, false);
    }
  };

  window.addEventListener('beforeunload', () => {
    if (state.turns.some((t) => t.content.trim())) saveDraft();
  });
}

async function runExport(dir) {
  const opts = {
    format: $('#e-format').value,
    scope: $('#e-scope').value,
    with_meta: $('#e-meta').checked,
    filename: null,
    target_dir: dir,
  };
  try {
    const r = await B.export(opts);
    let msg = `已导出 ${r.count} 条 → ${r.zip_path || r.path}`;
    if (r.skipped) msg += `（${r.skipped} 条含图片的记录在纯文本格式下被跳过）`;
    $('#e-result').textContent = msg;
    toast('导出完成');
    refreshStats();
    refreshList();
  } catch (e) {
    toast('导出失败：' + e, false);
  }
}

/* ---------- 启动 ---------- */
(async function init() {
  const style = document.createElement('style');
  style.textContent =
    '.toast{position:fixed;left:50%;transform:translateX(-50%) translateY(20px);bottom:88px;' +
    'background:#2a2f3a;color:#e6e9ef;border:1px solid #3a4354;border-radius:10px;padding:9px 16px;' +
    'font-size:13px;z-index:200;opacity:0;pointer-events:none;transition:all .18s;max-width:88vw}' +
    '.toast.show{opacity:1;transform:translateX(-50%) translateY(0)}' +
    '.toast.err{background:#4a1f1f;border-color:#7a3030;color:#ffd9d9}';
  document.head.appendChild(style);

  const t = document.createElement('div');
  t.id = 'toast';
  t.className = 'toast';
  document.body.appendChild(t);

  if (!IS_TAURI) {
    $('#banner').classList.remove('hidden');
    $('#banner').textContent =
      '当前在普通浏览器里运行：数据存在这个浏览器的本地存储中，清缓存会丢。要真正落盘保存，请用 Tauri 启动桌面版或安卓版。';
  }

  state.config = await B.config().catch(() => null);
  if (state.config) {
    $('#cfg-path').textContent =
      `数据目录 ${state.config.data_dir}\n导出目录 ${state.config.export_dir}\n数据库文件 ${state.config.db_path}`;
  }

  if (!restoreDraft()) {
    state.turns = [blankTurn('user'), blankTurn('assistant')];
  }
  renderTurns();
  bind();
  refreshStats();
  setView('compose');
})();

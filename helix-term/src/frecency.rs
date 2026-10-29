//! 按键频率统计（frecency）：给 which-key 排候选用。
//!
//! 为什么需要：截断谁都会做，关键是**截掉谁**。没有数据就只能按作者定义顺序截，
//! 于是"常用键被截掉、冷门键留着"。这里只做最小的一件事：记录**每次成功命中的
//! 键序列**（`space g b` / `n` …）的计数，落盘到 `~/.cache/helix/keys-frecency.json`。
//!
//! 设计取舍：
//! - 只记**计数**（不做时间衰减）：够用、实现简单、文件小（几百字节）；
//! - 写盘每 25 次增量一次（不是每次按键都写），退出时也不强制 flush —— 丢几次计数无所谓；
//! - 上限 512 条，超出按计数保留最高的那些（防止长跑之后文件膨胀）。

use std::collections::HashMap;
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{Mutex, OnceLock};

use helix_view::input::{KeyCode, KeyEvent, KeyModifiers};

const MAX_ENTRIES: usize = 512;
const FLUSH_EVERY: u64 = 25;

fn stats() -> &'static Mutex<HashMap<String, u64>> {
    static STATS: OnceLock<Mutex<HashMap<String, u64>>> = OnceLock::new();
    STATS.get_or_init(|| Mutex::new(HashMap::new()))
}

fn dirty() -> &'static AtomicU64 {
    static DIRTY: AtomicU64 = AtomicU64::new(0);
    &DIRTY
}

fn cache_path() -> Option<std::path::PathBuf> {
    let home = std::env::var_os("HOME").map(std::path::PathBuf::from)?;
    Some(home.join(".cache/helix/keys-frecency.json"))
}

/// 启动时读一次（文件不存在/坏掉都当空表，不报错）。
pub fn load() {
    let Some(path) = cache_path() else { return };
    let Ok(text) = std::fs::read_to_string(&path) else {
        return;
    };
    let Ok(map) = serde_json::from_str::<HashMap<String, u64>>(&text) else {
        log::warn!("keys-frecency.json 解析失败，忽略（按空表处理）");
        return;
    };
    let mut guard = stats().lock().unwrap_or_else(|e| e.into_inner());
    *guard = map;
    log::debug!("frecency: 载入 {} 条", guard.len());
}

fn flush() {
    let Some(path) = cache_path() else { return };
    let snapshot = {
        let guard = stats().lock().unwrap_or_else(|e| e.into_inner());
        // 超上限就只留计数最高的那些
        let mut items: Vec<(&String, &u64)> = guard.iter().collect();
        if items.len() > MAX_ENTRIES {
            items.sort_by(|a, b| b.1.cmp(a.1));
            items.truncate(MAX_ENTRIES);
        }
        items
            .into_iter()
            .map(|(k, v)| (k.clone(), *v))
            .collect::<HashMap<_, _>>()
    };
    if let Some(dir) = path.parent() {
        let _ = std::fs::create_dir_all(dir);
    }
    match serde_json::to_string(&snapshot) {
        Ok(text) => {
            if let Err(err) = std::fs::write(&path, text) {
                log::warn!("frecency: 写 {} 失败: {err}", path.display());
            }
        }
        Err(err) => log::warn!("frecency: 序列化失败: {err}"),
    }
}

/// 归一化后的键名：与 keymap 里写的一致（`C-c` / `space` / `g`）。
fn key_name(key: KeyEvent) -> String {
    let mods = key.modifiers;
    let mut out = String::new();
    if mods.contains(KeyModifiers::SHIFT) {
        // 大写字母/`G` 这种靠字符本身表达，别的才加前缀
        if !matches!(key.code, KeyCode::Char(_)) {
            out.push_str("S-");
        }
    }
    if mods.contains(KeyModifiers::CONTROL) {
        out.push_str("C-");
    }
    if mods.contains(KeyModifiers::ALT) {
        out.push_str("A-");
    }
    match key.code {
        KeyCode::Char(' ') => out.push_str("space"),
        KeyCode::Char(c) => out.push(c),
        KeyCode::Esc => out.push_str("esc"),
        KeyCode::Enter => out.push_str("ret"),
        KeyCode::Tab => out.push_str("tab"),
        KeyCode::Backspace => out.push_str("backspace"),
        KeyCode::Up => out.push_str("up"),
        KeyCode::Down => out.push_str("down"),
        KeyCode::Left => out.push_str("left"),
        KeyCode::Right => out.push_str("right"),
        KeyCode::Home => out.push_str("home"),
        KeyCode::End => out.push_str("end"),
        KeyCode::Delete => out.push_str("del"),
        KeyCode::Insert => out.push_str("ins"),
        KeyCode::PageUp => out.push_str("pageup"),
        KeyCode::PageDown => out.push_str("pagedown"),
        code => out.push_str(&format!("{code:?}").to_lowercase()),
    }
    out
}

/// 记录一次成功命中：`pending` 是这次按键前残留的前缀，`key` 是最后一次按键。
pub fn record_sequence(pending: &[KeyEvent], key: KeyEvent) {
    let mut seq: Vec<String> = pending.iter().map(|k| key_name(*k)).collect();
    seq.push(key_name(key));
    let seq = seq.join(" ");
    if seq.is_empty() {
        return;
    }
    {
        let mut guard = stats().lock().unwrap_or_else(|e| e.into_inner());
        *guard.entry(seq).or_insert(0) += 1;
    }
    let n = dirty().fetch_add(1, Ordering::Relaxed) + 1;
    if n.is_multiple_of(FLUSH_EVERY) {
        flush();
    }
}

/// 给 JS 的快照（`helix.key_frecency()`）：`{"space g b": 12, …}`。
pub fn snapshot_json() -> String {
    let guard = stats().lock().unwrap_or_else(|e| e.into_inner());
    serde_json::to_string(&*guard).unwrap_or_else(|_| "{}".to_string())
}

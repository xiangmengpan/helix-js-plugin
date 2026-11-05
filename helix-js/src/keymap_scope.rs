//! `helix.keymap.*` —— **作用域键位**：插件面板/弹窗的按键声明表（A 阶段）。
//!
//! ## 为什么要有（问题）
//!
//! 面板的键以前是插件自己 `switch (key.name)` 硬编码的（`plugins/filetree/plugin.js`
//! 的 `handle_key_with` 是典型）。后果：改键要改插件源码、`?` 帮助各写一份、
//! which-key/`:keys` 看不到、跟原生键位像两个世界。
//!
//! 这里只做**一张表**：
//! `作用域名 → { 规范化按键 → 绑定 }`，绑定要么是插件内的**动作名**（`"close"`），
//! 要么是**命令名**（用户 `[plugins.<插件名>.keys]` 覆盖时给的就是命令）。
//!
//! ## 职责边界（见 `docs/superpowers/plans/2026-11-04-panel-keybinding-scopes.md`）
//!
//! * **本模块**：键 → 绑定 的数据表 + 查询 + 提示文本。**不做分派**（不调插件代码）。
//! * **插件**：`resolve()` 拿动作名后自己做事；未命中时的"吞 / 穿透"仍由插件的
//!   `onKey` 返回值决定（B 阶段才收到 `input_router`）。
//! * **用户覆盖**：`[plugins.<插件名>.keys]`（辅助作用域 `[plugins.<名>.keys.<子名>]`）
//!   由 helix-term 解析成 `{ 键 → 命令名 }` 推给
//!   [`set_overrides`]，**在 `resolve` 时优先于插件声明**（不是注册时合并 —— 那样
//!   `:plugin-reload` 与 `:config-reload` 的先后会改变结果）。
//!
//! ## 键名的两种形状（已定案）
//!
//! `resolve(scope, key)` 既接受 `onKey` 原样的 `{name, shift, ctrl, alt}` 对象，
//! 也接受字符串（`"C-\\"`、`"<esc>"`、`"S-Tab"`）。两者都先规范化成 [`KeyId`]
//! 再查表 —— 这样声明表、TOML、运行期对象永远是同一把钥匙。

use std::collections::{BTreeMap, HashMap};

use boa_engine::object::ObjectInitializer;
use boa_engine::property::Attribute;
use boa_engine::{Context, JsResult, JsString, JsValue};

use crate::host::HostState;

// ────────────────────────── 键的规范化表示 ──────────────────────────

/// 规范化后的按键。`name` 对可打印字符原样保留（大小写有意义），对特殊键取固定写法
/// （`Esc`/`Enter`/`Tab`/…/`F1`），空格统一成 `" "`。
#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub struct KeyId {
    pub name: String,
    pub ctrl: bool,
    pub alt: bool,
    pub shift: bool,
}

impl KeyId {
    /// 规范化显示（`hints` 的输出形态，也是写 TOML/声明表时该用的写法）
    pub fn display(&self) -> String {
        let mut out = String::new();
        if self.ctrl {
            out.push_str("C-");
        }
        if self.alt {
            out.push_str("M-");
        }
        if self.shift {
            out.push_str("S-");
        }
        if self.name == " " {
            out.push_str("space");
        } else {
            out.push_str(&self.name);
        }
        out
    }

    /// 供外部（`popup.rs` 的 passthrough 检查）复用同一套键名归一
    pub fn normalize_for(raw: &str) -> String {
        Self::normalize_name(raw)
    }

    /// 特殊键名归一：`esc`/`<esc>`/`Esc` ⇒ `Esc`；`space` ⇒ `" "`；`f1` ⇒ `F1`；
    /// 其余（含所有可打印字符）原样返回。
    fn normalize_name(raw: &str) -> String {
        let lower = raw.to_ascii_lowercase();
        let canonical = match lower.as_str() {
            "esc" | "escape" => "Esc",
            "enter" | "return" | "cr" => "Enter",
            "tab" => "Tab",
            "backspace" | "bs" => "Backspace",
            "delete" | "del" => "Delete",
            "insert" | "ins" => "Insert",
            "up" => "Up",
            "down" => "Down",
            "left" => "Left",
            "right" => "Right",
            "home" => "Home",
            "end" => "End",
            "pageup" | "page-up" | "pgup" => "PageUp",
            "pagedown" | "page-down" | "pgdn" => "PageDown",
            "space" => " ",
            // `f1`..`f24`：只认数字尾巴，避免把普通字母 `f` 吃掉
            _ => {
                if lower.len() >= 2 && lower.starts_with('f') {
                    if let Ok(n) = lower[1..].parse::<u8>() {
                        if (1..=24).contains(&n) {
                            return format!("F{n}");
                        }
                    }
                }
                return raw.to_string();
            }
        };
        canonical.to_string()
    }

    /// 从字符串解析：`"j"` / `"C-\\"` / `"<esc>"` / `"S-Tab"` / `"C-M-x"`。
    ///
    /// 修饰键前缀 `C-` `M-`/`A-` `S-`，可重复、顺序任意；`<...>` 可省可留。
    pub fn parse(raw: &str) -> Result<KeyId, String> {
        let mut s = raw.trim();
        if s.is_empty() {
            return Err("空键名".into());
        }
        if s.starts_with('<') && s.ends_with('>') && s.len() > 2 {
            s = &s[1..s.len() - 1];
        }
        let (mut ctrl, mut alt, mut shift) = (false, false, false);
        loop {
            let b = s.as_bytes();
            // `X-` 前缀即可识别（`"C-"` 这种"只有修饰键"的写法随后会被判为空名报错）；
            // `"-"` 自己（长度 1）不会进这里
            if b.len() < 2 || b[1] != b'-' {
                break;
            }
            match b[0].to_ascii_lowercase() {
                b'c' => ctrl = true,
                b'm' | b'a' => alt = true,
                b's' => shift = true,
                _ => break,
            }
            s = &s[2..];
        }
        if s.is_empty() {
            return Err(format!("键名只有修饰键: {raw}"));
        }
        Ok(KeyId {
            name: Self::normalize_name(s),
            ctrl,
            alt,
            shift,
        })
    }

    /// 从 JS 的 `onKey` 对象取：`{name, shift, ctrl, alt}`（字段缺失即 false）
    fn from_js_object(obj: &boa_engine::JsObject, ctx: &mut Context) -> Result<KeyId, String> {
        let get = |key: &str, ctx: &mut Context| -> Result<JsValue, String> {
            obj.get(JsString::from(key), ctx)
                .map_err(|e| format!("读取 key.{key} 失败: {e}"))
        };
        let name: String = get("name", ctx)?
            .try_js_into(ctx)
            .map_err(|_| "key.name 必须是字符串".to_string())?;
        let flag = |v: JsValue| -> bool { v.as_boolean().unwrap_or(false) };
        Ok(KeyId {
            name: Self::normalize_name(&name),
            ctrl: flag(get("ctrl", ctx)?),
            alt: flag(get("alt", ctx)?),
            shift: flag(get("shift", ctx)?),
        })
    }
}

impl From<crate::types::PluginKey> for KeyId {
    /// `helix-term` 侧已经有 `key_to_plugin_key`（KeyEvent → PluginKey 的唯一映射）——
    /// 过期/覆盖键位直接走这条路，**不要再写第二份 KeyCode→名字的表**。
    fn from(k: crate::types::PluginKey) -> Self {
        KeyId {
            name: KeyId::normalize_name(&k.name),
            ctrl: k.ctrl,
            alt: k.alt,
            shift: k.shift,
        }
    }
}

// ────────────────────────── 表结构 ──────────────────────────

/// 一条绑定：**要么**是插件内动作名，**要么**是命令名（用户 TOML 覆盖来的）。
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Binding {
    pub action: Option<String>,
    pub command: Option<String>,
    pub desc: String,
}

impl Binding {
    fn action(name: &str, desc: &str) -> Self {
        Self {
            action: Some(name.to_string()),
            command: None,
            desc: desc.to_string(),
        }
    }
    fn command(name: &str) -> Self {
        Self {
            action: None,
            command: Some(name.to_string()),
            desc: format!(":{name}"),
        }
    }
}

/// 插件层对 Esc 的兜底动作（见 [`Scope::esc`]）
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum EscAction {
    /// 什么都不做（Esc 继续流向编辑器）—— 默认，零行为变化
    #[default]
    Ignore,
    /// 焦点回编辑器（叶子 0）；面板/弹层保留
    Blur,
}

/// 一个作用域（一个插件面板/弹窗的键位表）
#[derive(Debug, Clone, Default)]
pub struct Scope {
    pub keys: BTreeMap<KeyId, Binding>,
    pub extends: Option<String>,
    /// **永远交回编辑器**的键（B 阶段）：`C-w` / `C-p` / `:` 这类全局前缀。
    ///
    /// 为什么需要：popup 是模态层，`onKey` 一收就是全部 —— 用户按 `:` 打开命令行、
    /// 按 `C-w` 切窗口都会"莫名其妙没反应"。声明后由核心在**调 JS 之前**放行。
    pub passthrough: std::collections::BTreeSet<KeyId>,
    /// 插件层**没消费 Esc** 时核心怎么兜底（B 阶段，T4.4）：
    /// `Ignore`（默认）= 现状（Esc 继续往编辑器落）；
    /// `Blur` = 焦点回编辑器（面板/弹层保留，对应"退一层"语义）。
    pub esc: EscAction,
    /// 未命中的键怎么处理（B 阶段）：
    /// `All`（默认）= 与现在一致（未命中由 JS `onKey` 的返回值决定，缺省当消费）；
    /// `Unmapped` = JS 没给明确答复（`undefined`/认不出）时**交回编辑器**。
    pub swallow_unmapped_only: bool,
    /// 注册者（`:plugin-unload`/`:plugin-reload` 时按它清）
    pub owner: Option<String>,
}

/// `extends: "panel"` 的预设表（C 阶段的默认值；**只有默认值**，插件可整条覆盖）。
///
/// 口径（2026-11-04 定案）：`q` 关闭、`Esc` **回焦点**（不销毁面板，销毁用 `q`）、
/// `?` 帮助、`/` 过滤、`j`/`k` 上下、`g` 前缀起点。
pub fn panel_preset() -> Vec<(KeyId, Binding)> {
    let a = |k: &str, action: &str, desc: &str| {
        (
            KeyId::parse(k).expect("内置预设键名合法"),
            Binding::action(action, desc),
        )
    };
    vec![
        a("q", "close", "关闭面板"),
        a("<esc>", "blur", "回焦点（不关闭）"),
        a("?", "help", "显示键位"),
        a("/", "filter", "过滤"),
        a("j", "down", "下移"),
        a("k", "up", "上移"),
        a("g", "prefix", "前缀（gg 到顶…）"),
    ]
}

// ────────────────────────── 宿主内的存储 ──────────────────────────

fn with_scopes<T>(st: &HostState, f: impl FnOnce(&mut HashMap<String, Scope>) -> T) -> T {
    f(&mut st.key_scopes.borrow_mut())
}

fn with_overrides<T>(
    st: &HostState,
    f: impl FnOnce(&mut HashMap<String, BTreeMap<KeyId, Binding>>) -> T,
) -> T {
    f(&mut st.key_overrides.borrow_mut())
}

/// 用户 `[plugins.<插件名>.keys]` 的覆盖：`作用域名 → { 键 → 命令名 }`。
///
/// 由 helix-term 在 config 加载/重载后推入（helix-js 看不到 TOML）。**resolve 时优先**，
/// 因此插件 `:plugin-reload` 不会把用户覆盖弄丢。
pub fn set_overrides(
    st: &HostState,
    overrides: HashMap<String, Vec<(crate::types::PluginKey, String)>>,
) {
    with_overrides(st, |m| {
        m.clear();
        for (scope, entries) in overrides {
            let mut keys = BTreeMap::new();
            for (key, command) in entries {
                keys.insert(
                    KeyId::from(key),
                    Binding::command(command.trim_start_matches(':')),
                );
            }
            m.insert(scope, keys);
        }
    });
}

/// 注册/替换一个作用域（同名的旧表整体替换 —— "声明"语义，不是"追加"）。
pub fn register(st: &HostState, name: &str, mut scope: Scope) -> Result<(), String> {
    if name.is_empty() {
        return Err("scope 名不能为空".into());
    }
    // `extends` 展开：预设先铺，再让插件自己的键覆盖同名键
    if let Some(preset) = scope.extends.as_deref() {
        match preset {
            "panel" => {
                let mut merged: BTreeMap<KeyId, Binding> = panel_preset().into_iter().collect();
                for (k, v) in std::mem::take(&mut scope.keys) {
                    merged.insert(k, v);
                }
                scope.keys = merged;
            }
            other => return Err(format!("未知的键位预设 {other:?}（目前只有 \"panel\"）")),
        }
    }
    with_scopes(st, |m| {
        m.insert(name.to_string(), scope);
    });
    Ok(())
}

/// 注销（`:plugin-unload` 走 [`drop_owned`]；插件一般不用手调）
pub fn unregister(st: &HostState, name: &str) -> bool {
    with_scopes(st, |m| m.remove(name).is_some())
}

/// 按属主清理，返回清掉的作用域数（`:plugin-unload` 的报告里有一项）
pub fn drop_owned(st: &HostState, owner: &str) -> usize {
    with_scopes(st, |m| {
        let mine: Vec<String> = m
            .iter()
            .filter(|(_, s)| s.owner.as_deref() == Some(owner))
            .map(|(k, _)| k.clone())
            .collect();
        for k in &mine {
            m.remove(k);
        }
        mine.len()
    })
}

/// 这个键是否被声明为**永远交回编辑器**（B 阶段：核心在调 JS `onKey` 之前问一次）
pub fn is_passthrough(st: &HostState, name: &str, key: &KeyId) -> bool {
    with_scopes(st, |m| {
        m.get(name)
            .map(|s| s.passthrough.contains(key))
            .unwrap_or(false)
    })
}

/// 插件层没消费 Esc 时的兜底动作
pub fn esc_action(st: &HostState, name: &str) -> EscAction {
    with_scopes(st, |m| m.get(name).map(|s| s.esc).unwrap_or_default())
}

/// 未命中的键该不该交回编辑器（`swallow: "unmapped"`；默认 `false` = 现状）
pub fn gives_back_unmapped(st: &HostState, name: &str) -> bool {
    with_scopes(st, |m| {
        m.get(name)
            .map(|s| s.swallow_unmapped_only)
            .unwrap_or(false)
    })
}

/// 查一个键：**用户覆盖优先**，其次插件声明。命中返回绑定克隆。
pub fn resolve(st: &HostState, name: &str, key: &KeyId) -> Option<Binding> {
    let over = with_overrides(st, |m| m.get(name).and_then(|keys| keys.get(key)).cloned());
    if over.is_some() {
        return over;
    }
    with_scopes(st, |m| m.get(name).and_then(|s| s.keys.get(key)).cloned())
}

/// 已注册的作用域名（排序后返回）。`:keys` 用它列出"当前有哪些插件作用域"。
pub fn scope_names(st: &HostState) -> Vec<String> {
    let mut names: Vec<String> = with_scopes(st, |m| m.keys().cloned().collect());
    names.sort();
    names
}

/// 某作用域在 `:keys` 里的整段文本（含标题）；没有这个作用域 ⇒ `None`。
pub fn section(st: &HostState, name: &str) -> Option<String> {
    let body = hints(st, name)?;
    let owner = with_scopes(st, |m| m.get(name).and_then(|s| s.owner.clone()));
    // 改键表的**宿主插件名**：辅助作用域（`filetree.help`）也要指向 `[plugins.filetree.keys.…]`
    let host = owner
        .as_deref()
        .and_then(|o| o.split('.').next())
        .or_else(|| name.split('.').next())
        .unwrap_or(name);
    let who = owner
        .as_deref()
        .map(|o| format!("，注册者 `{o}`"))
        .unwrap_or_default();
    Some(format!(
        "// 插件作用域 `{name}`{who}（改键写 [plugins.{host}.keys]；入口 = 该插件的 onKey）\n\n{body}\n"
    ))
}

/// 提示文本（`?` / `:keys` / which-key 同一份数据）：按 `键    说明` 对齐。
pub fn hints(st: &HostState, name: &str) -> Option<String> {
    let mut rows: Vec<(String, String)> = Vec::new();
    // 覆盖项排前面（用户改的键最该被看见）
    with_overrides(st, |m| {
        if let Some(keys) = m.get(name) {
            for (k, b) in keys {
                rows.push((k.display(), b.desc.clone()));
            }
        }
    });
    let declared = with_scopes(st, |m| {
        m.get(name).map(|s| {
            s.keys
                .iter()
                .map(|(k, b)| (k.clone(), b.clone()))
                .collect::<Vec<_>>()
        })
    });
    if let Some(declared) = declared {
        for (k, b) in declared {
            if rows.iter().any(|(d, _)| d == &k.display()) {
                continue; // 已被用户覆盖，别列两遍
            }
            rows.push((k.display(), b.desc));
        }
    } else if rows.is_empty() {
        return None; // 既没声明也没覆盖 ⇒ 没有可提示的东西
    }
    let width = rows
        .iter()
        .map(|(k, _)| k.chars().count())
        .max()
        .unwrap_or(0);
    Some(
        rows.iter()
            .map(|(k, d)| format!("{k:<width$}  {d}", width = width))
            .collect::<Vec<_>>()
            .join("\n"),
    )
}

// ────────────────────────── JS 绑定 ──────────────────────────

fn key_arg(v: &JsValue, ctx: &mut Context) -> Result<KeyId, String> {
    if let Some(obj) = v.as_object() {
        return KeyId::from_js_object(&obj, ctx);
    }
    let s: String = v
        .clone()
        .try_js_into(ctx)
        .map_err(|_| "key 必须是 {name,ctrl,alt,shift} 对象或字符串".to_string())?;
    KeyId::parse(&s)
}

/// `helix.keymap.scope(name, spec)`；`spec = { extends?, keys: { "<键>": {action, desc?} | "<动作>" } }`
pub(crate) fn js_scope(_t: &JsValue, args: &[JsValue], ctx: &mut Context) -> JsResult<JsValue> {
    let name: String = args
        .first()
        .unwrap_or(&JsValue::undefined())
        .clone()
        .try_js_into(ctx)
        .map_err(|_| crate::js_err("keymap.scope: name 必须是字符串"))?;
    let spec = args.get(1).cloned().unwrap_or(JsValue::undefined());
    let Some(obj) = spec.as_object() else {
        return Err(crate::js_err(
            "keymap.scope: 第二参数必须是 {keys:{…}} 对象",
        ));
    };

    let mut scope = Scope::default();
    if let Ok(v) = obj.get(JsString::from("extends"), ctx) {
        if !v.is_null_or_undefined() {
            scope.extends = Some(v.try_js_into(ctx).map_err(|_| {
                crate::js_err("keymap.scope: extends 必须是字符串（目前只有 \"panel\"）")
            })?);
        }
    }
    // `swallow: "all" | "unmapped"`
    if let Ok(v) = obj.get(JsString::from("swallow"), ctx) {
        if !v.is_null_or_undefined() {
            let mode: String = v.try_js_into(ctx).map_err(|_| {
                crate::js_err("keymap.scope: swallow 必须是 \"all\" 或 \"unmapped\"")
            })?;
            match mode.as_str() {
                "all" => scope.swallow_unmapped_only = false,
                "unmapped" => scope.swallow_unmapped_only = true,
                other => {
                    return Err(crate::js_err(format!(
                        "keymap.scope: swallow 只接受 \"all\" / \"unmapped\"，收到 {other:?}"
                    )))
                }
            }
        }
    }
    // `esc: "blur" | "ignore"`
    if let Ok(v) = obj.get(JsString::from("esc"), ctx) {
        if !v.is_null_or_undefined() {
            let mode: String = v
                .try_js_into(ctx)
                .map_err(|_| crate::js_err("keymap.scope: esc 必须是 \"blur\" 或 \"ignore\""))?;
            scope.esc = match mode.as_str() {
                "blur" => EscAction::Blur,
                "ignore" => EscAction::Ignore,
                other => {
                    return Err(crate::js_err(format!(
                        "keymap.scope: esc 只接受 \"blur\" / \"ignore\"，收到 {other:?}"
                    )))
                }
            };
        }
    }
    // `passthrough: ["C-w", ":"]`
    if let Ok(v) = obj.get(JsString::from("passthrough"), ctx) {
        if let Some(arr) = v.as_object() {
            let len: usize = arr
                .get(JsString::from("length"), ctx)
                .ok()
                .and_then(|l| l.try_js_into::<usize>(ctx).ok())
                .unwrap_or(0);
            for i in 0..len {
                let item = arr.get(i, ctx).map_err(|e| {
                    crate::js_err(format!("keymap.scope: 读 passthrough[{i}] 失败: {e}"))
                })?;
                let raw: String = item.try_js_into(ctx).map_err(|_| {
                    crate::js_err("keymap.scope: passthrough 里每项都必须是键名字符串")
                })?;
                let id = KeyId::parse(&raw).map_err(|e| {
                    crate::js_err(format!(
                        "keymap.scope[{name}]: passthrough 键 {raw:?} 无法解析: {e}"
                    ))
                })?;
                scope.passthrough.insert(id);
            }
        }
    }

    let keys_val = obj
        .get(JsString::from("keys"), ctx)
        .map_err(|e| crate::js_err(format!("keymap.scope: 读取 keys 失败: {e}")))?;
    if let Some(keys_obj) = keys_val.as_object() {
        // 枚举对象自有的键（`Object.keys` 同款语义；只认字符串键 —— 与 `theme.rs` 同一写法）
        let names: Vec<String> = keys_obj
            .own_property_keys(ctx)
            .map_err(|e| crate::js_err(format!("keymap.scope: 枚举 keys 失败: {e}")))?
            .into_iter()
            .filter_map(|k| match k {
                boa_engine::property::PropertyKey::String(s) => Some(s.to_std_string_escaped()),
                _ => None,
            })
            .collect();
        for key_name in names {
            let id = match KeyId::parse(&key_name) {
                Ok(id) => id,
                // 拼错的键名**报错**（这里不是运行期热路径，写错就该立刻知道）
                Err(e) => {
                    return Err(crate::js_err(format!(
                        "keymap.scope[{name}]: 键 {key_name:?} 无法解析: {e}"
                    )))
                }
            };
            let val = keys_obj
                .get(JsString::from(key_name.as_str()), ctx)
                .map_err(|e| crate::js_err(format!("keymap.scope: 读取 {key_name} 失败: {e}")))?;
            let binding = if let Some(inner) = val.as_object() {
                let action: String = inner
                    .get(JsString::from("action"), ctx)
                    .ok()
                    .and_then(|v| v.try_js_into::<String>(ctx).ok())
                    .ok_or_else(|| {
                        crate::js_err(format!("keymap.scope[{name}]: {key_name} 缺少 action"))
                    })?;
                let desc: String = inner
                    .get(JsString::from("desc"), ctx)
                    .ok()
                    .and_then(|v| v.try_js_into::<String>(ctx).ok())
                    .unwrap_or_else(|| action.clone());
                Binding::action(&action, &desc)
            } else if let Ok(action) = val.try_js_into::<String>(ctx) {
                // 简写：`"q": "close"`（desc 默认 = action）
                Binding::action(&action, &action)
            } else {
                return Err(crate::js_err(format!(
                    "keymap.scope[{name}]: {key_name} 的值必须是 {{action, desc?}} 或字符串"
                )));
            };
            scope.keys.insert(id, binding);
        }
    }

    // 属主 = 当前回调/加载期的插件名（与 `register_command` 同一套判定）⇒ `:plugin-unload` 能按它清
    scope.owner = crate::permissions::current_owner().or_else(crate::permissions::current_declared);
    register(crate::host::host_state(ctx), &name, scope)
        .map_err(|e| crate::js_err(format!("keymap.scope: {e}")))?;
    Ok(JsValue::undefined())
}

/// `helix.keymap.resolve(name, key)` → `{action}` | `{command}` | `null`
pub(crate) fn js_resolve(_t: &JsValue, args: &[JsValue], ctx: &mut Context) -> JsResult<JsValue> {
    let name: String = args
        .first()
        .unwrap_or(&JsValue::undefined())
        .clone()
        .try_js_into(ctx)
        .map_err(|_| crate::js_err("keymap.resolve: name 必须是字符串"))?;
    let key = match key_arg(args.get(1).unwrap_or(&JsValue::undefined()), ctx) {
        Ok(k) => k,
        // 解析不出来 ⇒ 当作"没命中"，不抛错（onKey 每帧都在跑，别把面板整挂）
        Err(_) => return Ok(JsValue::null()),
    };
    let hit = resolve(crate::host::host_state(ctx), &name, &key);
    let Some(b) = hit else {
        return Ok(JsValue::null());
    };
    let mut builder = ObjectInitializer::new(ctx);
    if let Some(a) = b.action {
        builder.property(
            JsString::from("action"),
            JsString::from(a),
            Attribute::all(),
        );
    }
    if let Some(c) = b.command {
        builder.property(
            JsString::from("command"),
            JsString::from(c),
            Attribute::all(),
        );
    }
    builder.property(
        JsString::from("desc"),
        JsString::from(b.desc),
        Attribute::all(),
    );
    Ok(JsValue::from(builder.build()))
}

/// `helix.keymap.hints(name)` → 对齐好的文本；没有这个作用域 ⇒ `null`
pub(crate) fn js_hints(_t: &JsValue, _args: &[JsValue], ctx: &mut Context) -> JsResult<JsValue> {
    let name: String = _args
        .first()
        .unwrap_or(&JsValue::undefined())
        .clone()
        .try_js_into(ctx)
        .map_err(|_| crate::js_err("keymap.hints: name 必须是字符串"))?;
    let text = hints(crate::host::host_state(ctx), &name);
    Ok(match text {
        Some(t) => JsValue::from(JsString::from(t)),
        None => JsValue::null(),
    })
}

/// `helix.keymap.unregister(name)` → `true` = 删掉了
pub(crate) fn js_unregister(
    _t: &JsValue,
    args: &[JsValue],
    ctx: &mut Context,
) -> JsResult<JsValue> {
    let name: String = args
        .first()
        .unwrap_or(&JsValue::undefined())
        .clone()
        .try_js_into(ctx)
        .map_err(|_| crate::js_err("keymap.unregister: name 必须是字符串"))?;
    let removed = unregister(crate::host::host_state(ctx), &name);
    Ok(JsValue::from(removed))
}

#[cfg(test)]
mod tests {
    use super::*;

    /// 两种形状必须归一到**同一把** KeyId（这是"对象和字符串都收"的地基）
    #[test]
    fn parse_normalizes_strings_and_objects_identically() {
        let cases = [
            ("j", "j", false, false, false),
            ("<esc>", "Esc", false, false, false),
            ("Esc", "Esc", false, false, false),
            ("C-\\", "\\", true, false, false),
            ("S-Tab", "Tab", false, false, true),
            ("C-M-x", "x", true, true, false),
            ("space", " ", false, false, false),
            ("<space>", " ", false, false, false),
            ("f1", "F1", false, false, false),
            ("F12", "F12", false, false, false),
        ];
        for (raw, name, ctrl, alt, shift) in cases {
            let k = KeyId::parse(raw).unwrap_or_else(|e| panic!("{raw} 解析失败: {e}"));
            assert_eq!(
                (k.name.as_str(), k.ctrl, k.alt, k.shift),
                (name, ctrl, alt, shift),
                "解析 {raw:?}"
            );
            // 显示应能再解析回同一个键（TOML/声明表可以用它写）
            assert_eq!(
                KeyId::parse(&k.display()).unwrap(),
                k,
                "display 往返: {raw:?}"
            );
        }
        // `"-"` 单独作为键名不该被当成修饰键前缀
        assert_eq!(KeyId::parse("-").unwrap().name, "-");
        assert!(KeyId::parse("C-").is_err(), "只有修饰键应报错");
        assert!(KeyId::parse("").is_err());
    }

    #[test]
    fn register_resolve_and_preset_merge() {
        let host = crate::host::JsHost::new();
        let st = host.state();
        // 预设 + 覆盖：`q` 换成别的动作，`j` 用预设的
        register(
            st,
            "demo",
            Scope {
                extends: Some("panel".into()),
                keys: BTreeMap::from([
                    (
                        KeyId::parse("q").unwrap(),
                        Binding::action("quit-panel", "自定义关闭"),
                    ),
                    (
                        KeyId::parse("C-\\").unwrap(),
                        Binding::action("leave", "回编辑器"),
                    ),
                ]),
                owner: Some("demo".into()),
                ..Default::default()
            },
        )
        .unwrap();

        let q = resolve(st, "demo", &KeyId::parse("q").unwrap()).unwrap();
        assert_eq!(q.action.as_deref(), Some("quit-panel"), "插件声明覆盖预设");
        let j = resolve(st, "demo", &KeyId::parse("j").unwrap()).unwrap();
        assert_eq!(j.action.as_deref(), Some("down"), "预设项仍在");
        let esc = resolve(st, "demo", &KeyId::parse("<esc>").unwrap()).unwrap();
        assert_eq!(
            esc.action.as_deref(),
            Some("blur"),
            "Esc = 回焦点（2026-11-04 定案）"
        );
        assert!(resolve(st, "demo", &KeyId::parse("z").unwrap()).is_none());
        assert!(resolve(st, "nope", &KeyId::parse("q").unwrap()).is_none());
    }

    #[test]
    fn unknown_preset_is_an_error() {
        let host = crate::host::JsHost::new();
        let st = host.state();
        let err = register(
            st,
            "demo",
            Scope {
                extends: Some("nonsense".into()),
                ..Default::default()
            },
        )
        .unwrap_err();
        assert!(err.contains("nonsense"), "{err}");
    }

    /// 用户覆盖**优先于**插件声明，且 `hints` 不重复列两遍
    #[test]
    fn user_overrides_win_and_hints_dedupe() {
        let host = crate::host::JsHost::new();
        let st = host.state();
        register(
            st,
            "demo",
            Scope {
                keys: BTreeMap::from([
                    (KeyId::parse("q").unwrap(), Binding::action("close", "关闭")),
                    (KeyId::parse("j").unwrap(), Binding::action("down", "下移")),
                ]),
                ..Default::default()
            },
        )
        .unwrap();
        set_overrides(
            st,
            HashMap::from([(
                "demo".to_string(),
                vec![(
                    crate::types::PluginKey {
                        name: "q".into(),
                        shift: false,
                        ctrl: false,
                        alt: false,
                    },
                    "demo-toggle".to_string(),
                )],
            )]),
        );

        let q = resolve(st, "demo", &KeyId::parse("q").unwrap()).unwrap();
        assert_eq!(q.command.as_deref(), Some("demo-toggle"), "覆盖优先");
        assert!(q.action.is_none());

        let hints = hints(st, "demo").unwrap();
        assert_eq!(hints.matches("q").count(), 1, "同名键不该列两遍: {hints}");
        assert!(hints.contains(":demo-toggle"), "{hints}");
        assert!(hints.contains("下移"), "{hints}");
    }

    #[test]
    fn drop_owned_only_removes_that_owner() {
        let host = crate::host::JsHost::new();
        let st = host.state();
        let mk = |owner: &str| Scope {
            owner: Some(owner.into()),
            ..Default::default()
        };
        register(st, "a", mk("plug-a")).unwrap();
        register(st, "b", mk("plug-b")).unwrap();
        assert_eq!(drop_owned(st, "plug-a"), 1);
        assert!(
            resolve(st, "b", &KeyId::parse("q").unwrap()).is_none(),
            "b 仍存在（只是表空）"
        );
        assert!(hints(st, "a").is_none(), "a 已清");
    }

    /// `helix.keymap.active_scope()`：compositor 每帧推的"当前插件层作用域名"
    #[test]
    fn active_scope_cache_roundtrip() {
        crate::init();
        let st = crate::host::state_handle_or_init();
        crate::cache_active_scope(&st, "");
        crate::load_script(r#"helix.echo("none:" + String(helix.keymap.active_scope()));"#)
            .unwrap();
        crate::cache_active_scope(&st, "filetree");
        crate::load_script(r#"helix.echo("on:" + helix.keymap.active_scope());"#).unwrap();
        crate::cache_active_scope(&st, "");
        crate::load_script(r#"helix.echo("off:" + String(helix.keymap.active_scope()));"#).unwrap();
        let msgs = crate::take_messages();
        assert_eq!(msgs[0], "none:null", "没有插件层 ⇒ null");
        assert_eq!(
            msgs[1], "on:filetree",
            "推入后按名字返回（**不是**大写徽标）"
        );
        assert_eq!(msgs[2], "off:null", "清空后回到 null");
    }

    /// `:keys` 的"插件作用域"段：标题带注册者，正文是声明表（与 `hints` 同源）
    #[test]
    fn section_renders_for_keys_help() {
        let host = crate::host::JsHost::new();
        let st = host.state();
        register(
            st,
            "ks-help",
            Scope {
                keys: BTreeMap::from([
                    (
                        KeyId::parse("q").unwrap(),
                        Binding::action("close", "关闭探头"),
                    ),
                    (
                        KeyId::parse("g").unwrap(),
                        Binding::action("top", "跳到顶部"),
                    ),
                ]),
                owner: Some("ks-help".into()),
                ..Default::default()
            },
        )
        .unwrap();
        let sec = section(st, "ks-help").expect("应有一段文本");
        assert!(sec.contains("插件作用域"), "{sec}");
        assert!(sec.contains("ks-help"), "应含作用域名: {sec}");
        assert!(
            sec.contains("注册者"),
            "应标明注册者（`:plugin-unload` 会按它清）: {sec}"
        );
        assert!(
            sec.contains("关闭探头") && sec.contains("跳到顶部"),
            "{sec}"
        );
        assert!(
            sec.contains("[plugins.ks-help.keys]"),
            "应提示新写法（`[plugins.<插件名>.keys]`）: {sec}"
        );
        assert!(section(st, "nope").is_none());
    }

    /// **B 阶段**：`passthrough` / `swallow` 的声明与查询
    #[test]
    fn passthrough_and_swallow_declarations() {
        crate::init();
        crate::load_script(
            r#"
            helix.keymap.scope("pt", {
              passthrough: [":", "C-w", "<esc>"],
              swallow: "unmapped",
              keys: { "q": "close" },
            });
            "#,
        )
        .expect("声明应合法");
        let st = crate::host::state_handle_or_init();

        for (raw, want) in [
            (":", true),
            ("C-w", true),
            ("<esc>", true),
            ("q", false),
            ("x", false),
        ] {
            let kid = KeyId::parse(raw).unwrap();
            assert_eq!(
                is_passthrough(&st, "pt", &kid),
                want,
                "passthrough({raw}) 应为 {want}"
            );
        }
        // 对象形状（onKey 拿到的那个）也要命中同一条
        let obj = KeyId::from(crate::types::PluginKey {
            name: "w".into(),
            shift: false,
            ctrl: true,
            alt: false,
        });
        assert!(is_passthrough(&st, "pt", &obj), "对象形状应命中 C-w");

        assert!(
            gives_back_unmapped(&st, "pt"),
            "swallow:\"unmapped\" 应生效"
        );
        assert!(
            !gives_back_unmapped(&st, "nope"),
            "未声明的 scope 按默认（消费）"
        );

        // 默认不 passthrough（不声明就保持旧行为，零行为变化）
        crate::load_script(r#"helix.keymap.scope("plain", { keys: { "q": "close" } });"#).unwrap();
        assert!(!is_passthrough(&st, "plain", &KeyId::parse(":").unwrap()));
        assert!(!gives_back_unmapped(&st, "plain"));

        // 非法声明要报错（拼错不静默）
        assert!(
            crate::load_script(r#"helix.keymap.scope("bad", { swallow: "everything" });"#).is_err()
        );
        assert!(
            crate::load_script(r#"helix.keymap.scope("bad2", { passthrough: [42] });"#).is_err()
        );

        unregister(&st, "pt");
        unregister(&st, "plain");
    }

    /// JS 面：`scope()` 收对象/字符串两种键、`resolve()` 两种形状都查得到
    #[test]
    fn js_scope_and_resolve_accept_both_key_shapes() {
        crate::init();
        crate::load_script(
            r#"
            helix.keymap.scope("demo", {
              extends: "panel",
              keys: { "q": "close", "C-\\": { action: "leave", desc: "回编辑器" } },
            });
            const byStr = helix.keymap.resolve("demo", "C-\\");
            helix.echo("str:" + (byStr && byStr.action));
            const byObj = helix.keymap.resolve("demo", { name: "\\", ctrl: true, alt: false, shift: false });
            helix.echo("obj:" + (byObj && byObj.action));
            const preset = helix.keymap.resolve("demo", "j");
            helix.echo("preset:" + (preset && preset.action));
            helix.echo("hints:" + Boolean(helix.keymap.hints("demo")));
            helix.echo("gone:" + helix.keymap.unregister("demo"));
            helix.echo("after:" + helix.keymap.resolve("demo", "q"));
            "#,
        )
        .expect("脚本应能跑");
        let msgs = crate::take_messages();
        assert_eq!(msgs[0], "str:leave");
        assert_eq!(msgs[1], "obj:leave", "对象形状必须命中同一条绑定");
        assert_eq!(msgs[2], "preset:down");
        assert_eq!(msgs[3], "hints:true");
        assert_eq!(msgs[4], "gone:true");
        assert_eq!(msgs[5], "after:null");
    }
}

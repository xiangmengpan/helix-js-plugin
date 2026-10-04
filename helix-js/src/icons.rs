//! 诊断标记图标 API：JS 侧 `helix.set_diagnostic_icons` → 覆盖集，helix-term 同步到 gutter 渲染。
//! 统一图标映射见 plugins/icons.js（本 API 是方案 4 的 Rust 接线）。

use std::collections::HashMap;
use std::sync::OnceLock;

use boa_engine::{Context, JsError, JsString, JsValue};

/// 诊断图标覆盖集（severity 名 → 图标字符；空 = 渲染侧默认 ●）
static DIAGNOSTIC_ICONS: OnceLock<std::sync::Mutex<HashMap<String, String>>> = OnceLock::new();

fn icons() -> &'static std::sync::Mutex<HashMap<String, String>> {
    DIAGNOSTIC_ICONS.get_or_init(|| std::sync::Mutex::new(HashMap::new()))
}

/// 设置诊断标记图标：{ error?, warning?, info?, hint? }（非字符串/空串忽略；整体替换）。
pub(crate) fn js_set_diagnostic_icons(
    _this: &JsValue,
    args: &[JsValue],
    ctx: &mut Context,
) -> boa_engine::JsResult<JsValue> {
    let api = "helix.set_diagnostic_icons";
    let obj = args
        .first()
        .unwrap_or(&JsValue::undefined())
        .as_object()
        .ok_or_else(|| {
            JsError::from_opaque(JsValue::from(JsString::from(format!(
                "{api}: expected an object of {{ error, warning, info, hint }}"
            ))))
        })?;
    let mut map = HashMap::new();
    for key in ["error", "warning", "info", "hint"] {
        if let Ok(v) = obj.get(JsString::from(key), ctx) {
            if v.is_string() {
                if let Ok(s) = v.try_js_into::<String>(ctx) {
                    if !s.is_empty() {
                        map.insert(key.to_string(), s);
                    }
                }
            }
        }
    }
    *icons().lock().expect("diag icons lock") = map;
    Ok(JsValue::undefined())
}

/// helix-term 取走当前诊断图标集（render 泵时同步到 gutter；未 set 过返回空表）
pub fn take_diagnostic_icons() -> HashMap<String, String> {
    std::mem::take(&mut *icons().lock().expect("diag icons lock"))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn diagnostic_icons_roundtrip() {
        crate::init();
        // 空表默认
        assert!(take_diagnostic_icons().is_empty());

        // set 后 take 拿到完整映射；非字符串/空串忽略
        crate::load_script(
            r##"
            helix.set_diagnostic_icons({
                error: "\uf00d",
                warning: "\uf12a",
                info: "\uf129",
                hint: 42,          // 非字符串忽略
                unknown: "x",      // 未知键忽略
            });
            "##,
        )
        .unwrap();
        let icons = take_diagnostic_icons();
        assert_eq!(icons.get("error").map(String::as_str), Some("\u{f00d}"));
        assert_eq!(icons.get("warning").map(String::as_str), Some("\u{f12a}"));
        assert_eq!(icons.get("info").map(String::as_str), Some("\u{f129}"));
        assert_eq!(icons.len(), 3, "非字符串与未知键应忽略: {icons:?}");

        // take 后清空（幂等）
        assert!(take_diagnostic_icons().is_empty(), "take 取走后应为空");

        // 缺参/非对象 → JS 报错
        assert!(crate::load_script(r#"helix.set_diagnostic_icons("x");"#).is_err());
    }
}

// ═══════════════════════════════════════════════════════════════
// 图标表 —— **核心单一来源**(从 plugins/lib/icons.js 的 ICONS 表迁移而来)
//
// 原先这张表在 JS 侧(253 行)。问题是:图标是"看外观的东西"却硬编码在插件里
// (用户改不了、升级会被覆盖),而且"字体不支持怎么降级"的策略**被每个消费方各写一遍**。
// 收进核心后数据只有一份、降级策略只有一份、所有插件零依赖可用。
//
// 注:上面那个 `DIAGNOSTIC_ICONS`(JS 经 `helix.set_diagnostic_icons` 设置的覆盖集)
// 是**给 gutter 渲染用的另一条通道**(`take_diagnostic_icons` 一次性取走),与下表的
// "默认诊断字形"职责不同,故不合并 —— 合并会让"取走后表就空了"这类隐蔽 bug 出现。
// ═══════════════════════════════════════════════════════════════

static FILE_ICONS: &[(&str, char)] = &[
    ("rs", '\u{e7a8}'),
    ("js", '\u{e74e}'),
    ("mjs", '\u{e74e}'),
    ("cjs", '\u{e74e}'),
    ("ts", '\u{e628}'),
    ("tsx", '\u{e7ba}'),
    ("jsx", '\u{e7ba}'),
    ("json", '\u{e60b}'),
    ("toml", '\u{e615}'),
    ("yaml", '\u{e615}'),
    ("yml", '\u{e615}'),
    ("md", '\u{f48a}'),
    ("markdown", '\u{f48a}'),
    ("py", '\u{e606}'),
    ("c", '\u{e61e}'),
    ("h", '\u{f0fd}'),
    ("cpp", '\u{e61d}'),
    ("cc", '\u{e61d}'),
    ("hpp", '\u{f0fd}'),
    ("go", '\u{e626}'),
    ("java", '\u{e204}'),
    ("rb", '\u{e739}'),
    ("php", '\u{e73d}'),
    ("html", '\u{f13b}'),
    ("htm", '\u{f13b}'),
    ("css", '\u{e749}'),
    ("scss", '\u{e603}'),
    ("sass", '\u{e603}'),
    ("less", '\u{e758}'),
    ("sh", '\u{f489}'),
    ("bash", '\u{f489}'),
    ("zsh", '\u{f489}'),
    ("fish", '\u{f489}'),
    ("sql", '\u{f1c0}'),
    ("lua", '\u{e620}'),
    ("swift", '\u{e755}'),
    ("kt", '\u{e634}'),
    ("kts", '\u{e634}'),
    ("scala", '\u{e737}'),
    ("dart", '\u{e798}'),
    ("r", '\u{f25d}'),
    ("ex", '\u{e62d}'),
    ("exs", '\u{e62d}'),
    ("erl", '\u{e7b1}'),
    ("hs", '\u{e777}'),
    ("clj", '\u{e768}'),
    ("elm", '\u{e62c}'),
    ("zig", '\u{e6a9}'),
    ("vue", '\u{fd42}'),
    ("svelte", '\u{e697}'),
    ("sol", '\u{e73c}'),
    ("txt", '\u{f15c}'),
    ("log", '\u{f18d}'),
    ("pdf", '\u{f1c1}'),
    ("doc", '\u{f1c2}'),
    ("docx", '\u{f1c2}'),
    ("xls", '\u{f1c3}'),
    ("ppt", '\u{f1c4}'),
    ("zip", '\u{f410}'),
    ("tar", '\u{f410}'),
    ("gz", '\u{f410}'),
    ("rar", '\u{f410}'),
    ("png", '\u{f1c5}'),
    ("jpg", '\u{f1c5}'),
    ("jpeg", '\u{f1c5}'),
    ("gif", '\u{f1c5}'),
    ("svg", '\u{f1c5}'),
    ("ico", '\u{f1c5}'),
    ("webp", '\u{f1c5}'),
    ("mp3", '\u{f001}'),
    ("wav", '\u{f001}'),
    ("flac", '\u{f001}'),
    ("ogg", '\u{f001}'),
    ("mp4", '\u{f03d}'),
    ("avi", '\u{f03d}'),
    ("mkv", '\u{f03d}'),
    ("mov", '\u{f03d}'),
    ("exe", '\u{f17d}'),
    ("dll", '\u{f17d}'),
    ("deb", '\u{f306}'),
    ("rpm", '\u{f306}'),
    ("lock", '\u{f023}'),
    ("conf", '\u{e615}'),
    ("ini", '\u{e615}'),
    ("env", '\u{f462}'),
];

static DIR_ICONS: &[(&str, char)] = &[("closed", '\u{f07b}'), ("open", '\u{f07c}')];

static MODE_ICONS: &[(&str, char)] = &[
    ("normal", '\u{f04b}'),
    ("insert", '\u{f040}'),
    ("select", '\u{f0ca}'),
];

static DIAGNOSTIC_GLYPHS: &[(&str, char)] = &[
    ("error", '\u{f00d}'),
    ("warning", '\u{f12a}'),
    ("info", '\u{f129}'),
    ("hint", '\u{f0eb}'),
];

static GIT_ICONS: &[(&str, char)] = &[
    ("M", '\u{f403}'),
    ("A", '\u{f402}'),
    ("D", '\u{f41a}'),
    ("R", '\u{f417}'),
    ("C", '\u{f419}'),
    ("U", '\u{f404}'),
];

static COMPLETION_ICONS: &[(&str, char)] = &[
    ("1", '\u{f031}'),
    ("2", '\u{ea8c}'),
    ("3", '\u{f121}'),
    ("4", '\u{eb5b}'),
    ("5", '\u{eb5f}'),
    ("6", '\u{ea88}'),
    ("7", '\u{eb5b}'),
    ("8", '\u{eb61}'),
    ("9", '\u{ea8b}'),
    ("10", '\u{eb65}'),
    ("11", '\u{ea90}'),
    ("12", '\u{ea8f}'),
    ("13", '\u{ea95}'),
    ("14", '\u{eb62}'),
    ("15", '\u{eb66}'),
    ("16", '\u{eb5c}'),
    ("17", '\u{eb60}'),
    ("18", '\u{eb36}'),
    ("19", '\u{f07b}'),
    ("20", '\u{eb5e}'),
    ("21", '\u{eb5d}'),
    ("22", '\u{ea91}'),
    ("23", '\u{ea86}'),
    ("24", '\u{eb64}'),
    ("25", '\u{ea92}'),
];

/// 图标总开关:字体不支持 nerd font 时置 false → **一处**统一降级
static ICONS_ENABLED: std::sync::atomic::AtomicBool = std::sync::atomic::AtomicBool::new(true);

pub fn set_icons_enabled(on: bool) {
    ICONS_ENABLED.store(on, std::sync::atomic::Ordering::Relaxed);
}

pub fn icons_enabled() -> bool {
    ICONS_ENABLED.load(std::sync::atomic::Ordering::Relaxed)
}

fn lookup(table: &[(&str, char)], key: &str) -> Option<char> {
    table.iter().find(|(k, _)| *k == key).map(|(_, g)| *g)
}

/// 扩展名(小写,取最后一段);隐藏文件与无扩展名 → None
fn ext_of(path: &str) -> Option<String> {
    let base = path.rsplit(['/', '\\']).next().unwrap_or(path);
    let (stem, ext) = base.rsplit_once('.')?;
    if stem.is_empty() || ext.is_empty() {
        return None;
    }
    Some(ext.to_ascii_lowercase())
}

/// 文件类型图标。降级时返回空串 —— **不显示图标**,而不是显示错字形。
pub fn icon_file(path: &str) -> String {
    if !icons_enabled() {
        return String::new();
    }
    ext_of(path)
        .and_then(|e| lookup(FILE_ICONS, &e))
        .map(|c| c.to_string())
        .unwrap_or_default()
}

/// 目录图标。降级时用 `▸`/`▾` —— 这是**唯一**的 ASCII 回退(原先散在各消费方)。
pub fn icon_dir(expanded: bool) -> String {
    if icons_enabled() {
        if let Some(c) = lookup(DIR_ICONS, if expanded { "open" } else { "closed" }) {
            return c.to_string();
        }
    }
    if expanded {
        "▾".into()
    } else {
        "▸".into()
    }
}

macro_rules! icon_lookup_fn {
    ($fname:ident, $table:ident, $doc:literal) => {
        #[doc = $doc]
        pub fn $fname(key: &str) -> String {
            if !icons_enabled() {
                return String::new();
            }
            lookup($table, key)
                .map(|c| c.to_string())
                .unwrap_or_default()
        }
    };
}

icon_lookup_fn!(icon_mode, MODE_ICONS, "模式图标(normal/insert/select)");
icon_lookup_fn!(icon_git, GIT_ICONS, "git 状态标记");
icon_lookup_fn!(icon_completion, COMPLETION_ICONS, "补全类型图标(LSP kind)");

/// 诊断标记的**默认**字形(与上面的 JS 覆盖集是两条通道)
pub fn icon_diagnostic(key: &str) -> String {
    if !icons_enabled() {
        return String::new();
    }
    lookup(DIAGNOSTIC_GLYPHS, key)
        .map(|c| c.to_string())
        .unwrap_or_default()
}

// ── JS 绑定(直接返回值,不走 UiRequest:图标是核心数据,无需往返)──

fn icon_str_fn(
    args: &[JsValue],
    ctx: &mut Context,
    f: impl FnOnce(&str) -> String,
) -> boa_engine::JsResult<JsValue> {
    let k: String = args
        .first()
        .unwrap_or(&JsValue::undefined())
        .try_js_into(ctx)?;
    Ok(JsValue::from(JsString::from(f(&k))))
}

pub(crate) fn js_icons_file(
    _t: &JsValue,
    args: &[JsValue],
    ctx: &mut Context,
) -> boa_engine::JsResult<JsValue> {
    icon_str_fn(args, ctx, icon_file)
}
pub(crate) fn js_icons_mode(
    _t: &JsValue,
    args: &[JsValue],
    ctx: &mut Context,
) -> boa_engine::JsResult<JsValue> {
    icon_str_fn(args, ctx, icon_mode)
}
pub(crate) fn js_icons_diagnostic(
    _t: &JsValue,
    args: &[JsValue],
    ctx: &mut Context,
) -> boa_engine::JsResult<JsValue> {
    icon_str_fn(args, ctx, icon_diagnostic)
}
pub(crate) fn js_icons_git(
    _t: &JsValue,
    args: &[JsValue],
    ctx: &mut Context,
) -> boa_engine::JsResult<JsValue> {
    icon_str_fn(args, ctx, icon_git)
}
pub(crate) fn js_icons_completion(
    _t: &JsValue,
    args: &[JsValue],
    ctx: &mut Context,
) -> boa_engine::JsResult<JsValue> {
    icon_str_fn(args, ctx, icon_completion)
}
pub(crate) fn js_icons_dir(
    _t: &JsValue,
    args: &[JsValue],
    ctx: &mut Context,
) -> boa_engine::JsResult<JsValue> {
    let expanded: bool = args
        .first()
        .unwrap_or(&JsValue::undefined())
        .try_js_into(ctx)?;
    Ok(JsValue::from(JsString::from(icon_dir(expanded))))
}
/// `helix.icons.enabled(bool?)` —— 无参时**返回**当前状态(便于插件自检)
pub(crate) fn js_icons_enabled(
    _t: &JsValue,
    args: &[JsValue],
    ctx: &mut Context,
) -> boa_engine::JsResult<JsValue> {
    if let Some(v) = args.first().filter(|v| !v.is_undefined()) {
        set_icons_enabled(v.try_js_into(ctx)?);
    }
    Ok(JsValue::from(icons_enabled()))
}

/// `helix.icons` 命名空间的成员(名字, 函数, 元数)
pub fn native_fns() -> Vec<(&'static str, boa_engine::NativeFunction, usize)> {
    vec![
        (
            "file",
            boa_engine::NativeFunction::from_fn_ptr(js_icons_file),
            1,
        ),
        (
            "dir",
            boa_engine::NativeFunction::from_fn_ptr(js_icons_dir),
            1,
        ),
        (
            "mode",
            boa_engine::NativeFunction::from_fn_ptr(js_icons_mode),
            1,
        ),
        (
            "diagnostic",
            boa_engine::NativeFunction::from_fn_ptr(js_icons_diagnostic),
            1,
        ),
        (
            "git",
            boa_engine::NativeFunction::from_fn_ptr(js_icons_git),
            1,
        ),
        (
            "completion",
            boa_engine::NativeFunction::from_fn_ptr(js_icons_completion),
            1,
        ),
        (
            "enabled",
            boa_engine::NativeFunction::from_fn_ptr(js_icons_enabled),
            1,
        ),
    ]
}

#[cfg(test)]
mod table_tests {
    use super::*;

    /// 表非空且关键映射正确(防"生成脚本解析失败留下空表"这类静默问题)
    #[test]
    fn icon_tables_are_populated() {
        assert_eq!(FILE_ICONS.len(), 85, "file 组条目数");
        assert_eq!(MODE_ICONS.len(), 3);
        assert_eq!(GIT_ICONS.len(), 6);
        assert_eq!(COMPLETION_ICONS.len(), 25);
        assert_eq!(
            icon_file("src/main.rs"),
            "\u{e7a8}".to_string(),
            "rust 图标"
        );
        assert_eq!(
            icon_file("a/b/App.tsx"),
            "\u{e7ba}".to_string(),
            "取最后一段扩展名"
        );
        assert_eq!(icon_file("noext"), "", "无扩展名 → 空(不显示错字形)");
        assert_eq!(icon_file(".gitignore"), "", "隐藏文件不算扩展名");
        assert_eq!(icon_mode("normal"), "\u{f04b}".to_string());
    }

    /// 降级策略**只有一处**:关掉后除目录外一律空串,目录用 ▸/▾
    #[test]
    fn disabled_falls_back_in_one_place() {
        set_icons_enabled(false);
        assert_eq!(icon_file("a.rs"), "", "关闭后不显示文件图标");
        assert_eq!(icon_mode("normal"), "");
        assert_eq!(icon_dir(false), "▸", "目录用 ASCII 回退");
        assert_eq!(icon_dir(true), "▾");
        set_icons_enabled(true);
        assert_eq!(
            icon_dir(false),
            "\u{f07b}".to_string(),
            "恢复后回到 nerd font"
        );
        assert_eq!(icon_dir(true), "\u{f07c}".to_string());
    }

    /// `helix.icons.*` 直接返回(不走 UiRequest)
    #[test]
    fn js_icons_namespace_returns_values() {
        crate::init();
        crate::load_script(
            r##"
            helix.echo("f:" + helix.icons.file("x.py"));
            helix.echo("m:" + helix.icons.mode("insert"));
            helix.echo("d:" + helix.icons.dir(true));
            helix.echo("on:" + helix.icons.enabled());
            helix.icons.enabled(false);
            helix.echo("off:" + helix.icons.file("x.py"));
            helix.echo("dir:" + helix.icons.dir(false));
            helix.icons.enabled(true);
            "##,
        )
        .unwrap();
        let msgs = crate::take_messages();
        assert_eq!(msgs[0], "f:\u{e606}".to_string());
        assert_eq!(msgs[1], "m:\u{f040}".to_string());
        assert_eq!(msgs[2], "d:\u{f07c}".to_string());
        assert_eq!(msgs[3], "on:true");
        assert_eq!(msgs[4], "off:", "关闭后文件图标为空");
        assert_eq!(msgs[5], "dir:▸");
    }
}

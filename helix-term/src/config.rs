use crate::keymap;
use crate::keymap::{merge_keys, KeyTrie};
use helix_loader::merge_toml_values;
use helix_view::{document::Mode, theme};
use serde::{Deserialize, Serialize};
use std::collections::HashMap;
use std::fmt::Display;
use std::fs;
use std::io::Error as IOError;
use toml::de::Error as TomlError;

/// 插件作用域键位：`作用域名 → [(键名, 命令名)]`（命令在插件加载后才解析，见 [`Config::key_scopes`]）
pub type KeyScopes = HashMap<String, Vec<(String, String)>>;

#[derive(Debug, Clone, PartialEq)]
pub struct Config {
    pub theme: Option<theme::Config>,
    pub keys: HashMap<Mode, KeyTrie>,
    /// `[plugins.<插件名>.keys]` 的原始条目（作用域名 → `(键名, 命令名)`）。
    ///
    /// **为什么存字符串而不是 `KeyTrie`**：`KeyTrie` 解析会校验命令是否存在，而插件命令
    /// （`:filetree-find` 之类）要等插件加载后才注册 —— 早一步校验就会把合法配置判成错误
    /// （实测：`":filetree-find"` 让整段作用域被丢掉）。所以这里只校验**键名**，
    /// 命令留到 `apply_key_scope_overrides()`（插件加载之后）再用。
    ///
    /// 为什么作用域名单独一份而不是塞进 `keys`：它不是 `Mode`；且要一直留着给
    /// `:keys` / 无人认领提示用。
    pub key_scopes: KeyScopes,
    /// 调试键表（`dap_keymap()` 之上合并了 `[keys.debug]`；暂停时 `input_router` 用它）
    pub keys_debug: KeyTrie,
    /// 加载期产生的**用户可见**警告（如未知的 `[keys.X]`）：启动时显示一次
    pub warnings: Vec<String>,
    pub editor: helix_view::editor::Config,
    /// config.toml 的 `[plugins] / [plugins.<name>]` 段(插件配置;原始 TOML 表)
    pub plugins: Option<toml::Table>,
    /// `[logging]` —— 日志文件策略（**只认用户级 config**，见 `LoggingConfig`）
    pub logging: LoggingConfig,
}

/// 日志文件的默认上限：10 MiB。
///
/// 为什么要有：helix.log 是**追加写、从不轮转**的（`logging::init_file` 用 `append`），
/// 长期使用能长到几十 MB（实测 18 MB / 12.8 万行）—— 想查一次插件报错反而要先翻山。
pub const DEFAULT_LOG_MAX_SIZE: u64 = 10 * 1024 * 1024;

/// `[logging]` 段。
///
/// **全局作用域**：workspace 的 `.helix/config.toml` 里的 `[logging]` **不生效**
/// （与 `editor.workspace-trust` 同理 —— 不能让一个仓库决定你的日志怎么存）。
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case", deny_unknown_fields, default)]
pub struct LoggingConfig {
    /// 日志文件大小上限（字节）。写入会超过它时**清空重写**（不是轮转出多个文件）。
    /// `0` = 不限制。默认 10 MiB。
    pub max_size: u64,
}

impl Default for LoggingConfig {
    fn default() -> Self {
        Self {
            max_size: DEFAULT_LOG_MAX_SIZE,
        }
    }
}

/// `[keys.*]` 的容错解析：未知的模式名（`[keys.debug]`、写错的 `[keys.nromal]`）
/// **只警告并跳过**，不再让整份 config 解析失败。
///
/// 为什么重要:早先 `keys` 直接是 `HashMap<Mode, KeyTrie>` ⇒ 一个不认识的段名让
/// `Config::load` 整体报 "Configuration file malformed"，用户的主题/插件配置**全部**失效。
#[derive(Debug, Clone, PartialEq, Default)]
pub struct KeysConfig(
    pub HashMap<Mode, KeyTrie>,
    /// `[keys.debug]`：**调试键表的用户覆盖**（合并到 `dap_keymap()` 之上）
    pub Option<KeyTrie>,
    pub Vec<String>,
);

impl<'de> Deserialize<'de> for KeysConfig {
    fn deserialize<D: serde::Deserializer<'de>>(d: D) -> Result<Self, D::Error> {
        use serde::de::{MapAccess, Visitor};
        struct V;
        impl<'de> Visitor<'de> for V {
            type Value = KeysConfig;
            fn expecting(&self, f: &mut std::fmt::Formatter) -> std::fmt::Result {
                f.write_str("表：{normal|insert|select} = 键位表")
            }
            fn visit_map<A: MapAccess<'de>>(self, mut m: A) -> Result<Self::Value, A::Error> {
                let mut out = HashMap::new();
                let mut debug: Option<KeyTrie> = None;
                let mut warnings = Vec::new();
                while let Some(name) = m.next_key::<String>()? {
                    match name.as_str() {
                        "normal" => {
                            out.insert(Mode::Normal, m.next_value::<KeyTrie>()?);
                        }
                        "insert" => {
                            out.insert(Mode::Insert, m.next_value::<KeyTrie>()?);
                        }
                        "select" => {
                            out.insert(Mode::Select, m.next_value::<KeyTrie>()?);
                        }
                        // 调试键不是 `Mode`（暂停时的接管表）⇒ 单独收，合并到 `dap_keymap()`
                        "debug" => {
                            let tree = m.next_value::<KeyTrie>()?;
                            match debug.as_mut() {
                                Some(existing) => existing.merge_nodes(tree),
                                None => debug = Some(tree),
                            }
                        }
                        other => {
                            // 未知段名 = **拼错**（插件键位已改到 `[plugins.<name>.keys]`，
                            // 不再占用 `[keys]` 的命名空间）⇒ 解析期就警告，吃掉这个值免得整份作废。
                            let _: toml::Value = m.next_value()?;
                            let msg = format!(
                                "[keys.{other}] 不是已知的键位模式（只有 normal/insert/select/debug；\
                                 插件面板/弹层的键位写在 [plugins.<插件名>.keys]）⇒ 已忽略这一段"
                            );
                            log::warn!("{msg}");
                            warnings.push(msg);
                        }
                    }
                }
                Ok(KeysConfig(out, debug, warnings))
            }
        }
        d.deserialize_map(V)
    }
}

#[derive(Debug, Clone, PartialEq, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ConfigRaw {
    pub theme: Option<theme::Config>,
    pub keys: Option<KeysConfig>,
    pub editor: Option<toml::Value>,
    pub plugins: Option<toml::Table>,
    /// `[logging]`：直接按结构体解析（`deny-unknown-fields` ⇒ 拼错的键会明确报错）
    pub logging: Option<LoggingConfig>,
}

impl Default for Config {
    fn default() -> Config {
        Config {
            theme: None,
            keys: keymap::default(),
            key_scopes: HashMap::new(),
            keys_debug: keymap::default::dap_keymap(),
            warnings: Vec::new(),
            editor: helix_view::editor::Config::default(),
            plugins: None,
            logging: LoggingConfig::default(),
        }
    }
}

#[derive(Debug)]
pub enum ConfigLoadError {
    BadConfig(TomlError),
    Error(IOError),
}

impl Default for ConfigLoadError {
    fn default() -> Self {
        ConfigLoadError::Error(IOError::new(std::io::ErrorKind::NotFound, "place holder"))
    }
}

impl Display for ConfigLoadError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            ConfigLoadError::BadConfig(err) => err.fmt(f),
            ConfigLoadError::Error(err) => err.fmt(f),
        }
    }
}

/// 从 `[plugins]` 里抽出**插件作用域键位**，并把 `keys` 子表从插件配置里摘掉。
///
/// 形状（TOML 天然嵌套，不新增顶层表）：
///
/// ```toml
/// [plugins.filetree]
/// show_hidden = false
///
/// [plugins.filetree.keys]        # 主作用域（scope 名 = 插件名）
/// "q" = ":filetree-toggle"
///
/// [plugins.filetree.keys.help]   # 辅助作用域（scope 名 = `filetree.help`）
/// "esc" = ":filetree-help-close"
/// ```
///
/// 只做**键名**校验（`esc` / `C-s` / `j` …）；命令原样保留，等插件加载后再解析 ——
/// 插件命令在 config 解析时还不存在（见 `Config::key_scopes` 的注释）。
///
/// 为什么要摘掉：`[plugins.<name>]` 的每个键都会被 `define_config` 的 schema 校验，
/// 留着 `keys` 会被报成"未在 define_config 中声明（已忽略）"。
///
/// 为什么用**嵌套**而不是把键平铺进 `[plugins.<name>]`：配置键名与键名会撞（`size`/`filter`/`mode`），
/// 而且一个插件可能有多个作用域（面板 vs 弹层），平铺表达不了。
fn extract_key_scopes(plugins: &mut Option<toml::Table>) -> (KeyScopes, Vec<String>) {
    use std::str::FromStr;

    let mut out: KeyScopes = HashMap::new();
    let mut warnings: Vec<String> = Vec::new();
    let Some(plugins) = plugins.as_mut() else {
        return (out, warnings);
    };

    // 一张键位表 → `(键名, 命令)` 列表；只支持单键
    let collect = |scope: &str,
                   table: &toml::Table,
                   out: &mut KeyScopes,
                   warnings: &mut Vec<String>| {
        let mut entries = Vec::new();
        for (key, val) in table {
            if helix_view::input::KeyEvent::from_str(key).is_err() {
                warnings.push(format!(
                    "[plugins.{scope}] 键名 {key:?} 核心不认（写法同 `[keys.*]`：`esc`/`C-s`/`S-Tab`；\
                     不支持 `<esc>` 与多键序列）⇒ 已跳过"
                ));
                continue;
            }
            match val.as_str() {
                Some(cmd) => entries.push((key.clone(), cmd.to_string())),
                None => warnings.push(format!(
                    "[plugins.{scope}] 键 {key:?} 的值必须是命令名字符串（`:cmd` 或 `@:cmd<ret>`），\
                     拿到的是 `{val}` ⇒ 已跳过"
                )),
            }
        }
        out.insert(scope.to_string(), entries);
    };

    for (name, value) in plugins.iter_mut() {
        let Some(table) = value.as_table_mut() else {
            continue;
        };
        let Some(keys) = table.remove("keys") else {
            continue;
        };
        let Some(keys) = keys.as_table() else {
            warnings.push(format!(
                "[plugins.{name}.keys] 必须是一张键位表（键 = \":命令\"）⇒ 已忽略"
            ));
            continue;
        };
        // 一层：字符串值 ⇒ 主作用域；表值 ⇒ 辅助作用域 `<插件名>.<子名>`
        let mut flat = toml::Table::new();
        for (key, val) in keys.clone() {
            match val {
                toml::Value::Table(sub) => {
                    let scope = format!("{name}.{key}");
                    collect(&scope, &sub, &mut out, &mut warnings);
                }
                other => {
                    flat.insert(key, other);
                }
            }
        }
        if !flat.is_empty() {
            collect(name, &flat, &mut out, &mut warnings);
        }
    }
    (out, warnings)
}

impl Config {
    pub fn load(
        global: Result<&String, ConfigLoadError>,
        local: Result<String, ConfigLoadError>,
    ) -> Result<Config, ConfigLoadError> {
        let global_config: Result<ConfigRaw, ConfigLoadError> =
            global.and_then(|file| toml::from_str(file).map_err(ConfigLoadError::BadConfig));
        let local_config: Result<ConfigRaw, ConfigLoadError> =
            local.and_then(|file| toml::from_str(&file).map_err(ConfigLoadError::BadConfig));
        let res = match (global_config, local_config) {
            (Ok(global), Ok(local)) => {
                let mut keys = keymap::default();
                let mut warnings = Vec::new();
                let mut keys_debug = keymap::default::dap_keymap();
                if let Some(global_keys) = global.keys {
                    warnings.extend(global_keys.2);
                    merge_keys(&mut keys, global_keys.0);
                    if let Some(t) = global_keys.1 {
                        keys_debug.merge_nodes(t);
                    }
                }
                if let Some(local_keys) = local.keys {
                    warnings.extend(local_keys.2);
                    merge_keys(&mut keys, local_keys.0);
                    if let Some(t) = local_keys.1 {
                        keys_debug.merge_nodes(t);
                    }
                }

                let editor = match (global.editor, local.editor) {
                    (None, None) => helix_view::editor::Config::default(),
                    (None, Some(val)) | (Some(val), None) => {
                        val.try_into().map_err(ConfigLoadError::BadConfig)?
                    }
                    (Some(global), Some(local)) => merge_toml_values(global, local, 3)
                        .try_into()
                        .map_err(ConfigLoadError::BadConfig)?,
                };

                // `[plugins.<name>.keys]` → 插件作用域键位（并从表里摘掉，免得被 schema 校验
                // 当成"未声明的插件配置键"）
                let mut plugins = local.plugins.or(global.plugins);
                let (key_scopes, scope_warnings) = extract_key_scopes(&mut plugins);
                warnings.extend(scope_warnings);

                Config {
                    theme: local.theme.or(global.theme),
                    keys,
                    key_scopes,
                    keys_debug,
                    warnings,
                    editor,
                    plugins,
                    // `[logging]` 只认用户级 config：workspace 的 `.helix/config.toml` 不参与
                    // （与 `editor.workspace-trust` 同理 —— 别让一个仓库决定日志怎么存）
                    logging: global.logging.unwrap_or_default(),
                }
            }
            // if any configs are invalid return that first
            (_, Err(ConfigLoadError::BadConfig(err)))
            | (Err(ConfigLoadError::BadConfig(err)), _) => {
                return Err(ConfigLoadError::BadConfig(err))
            }
            (Ok(config), Err(_)) | (Err(_), Ok(config)) => {
                let mut keys = keymap::default();
                let mut warnings = Vec::new();
                let mut keys_debug = keymap::default::dap_keymap();
                if let Some(keymap) = config.keys {
                    warnings.extend(keymap.2);
                    if let Some(t) = keymap.1 {
                        keys_debug.merge_nodes(t);
                    }
                    merge_keys(&mut keys, keymap.0);
                }
                let mut plugins = config.plugins;
                let (key_scopes, scope_warnings) = extract_key_scopes(&mut plugins);
                warnings.extend(scope_warnings);
                Config {
                    theme: config.theme,
                    keys,
                    key_scopes,
                    keys_debug,
                    warnings,
                    editor: config.editor.map_or_else(
                        || Ok(helix_view::editor::Config::default()),
                        |val| val.try_into().map_err(ConfigLoadError::BadConfig),
                    )?,
                    plugins,
                    logging: config.logging.unwrap_or_default(),
                }
            }

            // these are just two io errors return the one for the global config
            (Err(err), Err(_)) => return Err(err),
        };

        // 让 `input_router` 立刻用上合并后的调试键表（含 `:config-reload`）
        keymap::default::set_debug_keymap(Some(res.keys_debug.clone()));
        Ok(res)
    }

    pub fn load_default() -> Result<Config, ConfigLoadError> {
        let global_config =
            fs::read_to_string(helix_loader::config_file()).map_err(ConfigLoadError::Error)?;
        let local_config = fs::read_to_string(helix_loader::workspace_config_file())
            .map_err(ConfigLoadError::Error);

        let phony_config = ConfigLoadError::Error(IOError::other("hacky placeholder"));
        let global_parsed = Config::load(Ok(&global_config), Err(phony_config))?;

        // We need to build a transient `WorkspaceTrust` just to ask whether the workspace is
        // trusted enough to load its `.helix/config.toml`. The persisted-trust file on disk is the
        // source of truth either way; this transient instance has an empty cache and is dropped
        // after the check.
        let trust = helix_loader::workspace_trust::WorkspaceTrust::new(
            (&global_parsed.editor.workspace_trust).into(),
        );
        if trust
            .query_current(helix_loader::workspace_trust::TrustQuery::LocalConfig)
            .is_trusted()
        {
            let mut merged = Config::load(Ok(&global_config), local_config)?;
            // editor.workspace-trust is global/user-scope only. Without this override, a
            // workspace's `.helix/config.toml` could set `level = "insecure"`; once the user trusted
            // *that* workspace, refresh_config would re-load with the override merged in and from
            // then on every subsequent workspace in the session would be implicitly trusted. Pin
            // the gate's own configuration to the global file.
            merged.editor.workspace_trust = global_parsed.editor.workspace_trust;
            Ok(merged)
        } else {
            Ok(global_parsed)
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    impl Config {
        fn load_test(config: &str) -> Config {
            Config::load(Ok(&config.to_owned()), Err(ConfigLoadError::default())).unwrap()
        }
    }

    #[test]
    fn keys_debug_merges_onto_dap_keymap() {
        let cfg = Config::load_test(
            r#"
            [keys.debug]
            "x" = "dap_next"
            "#,
        );
        use helix_view::input::KeyEvent;
        let x: KeyEvent = "x".parse().unwrap();
        match cfg.keys_debug.search(&[x]) {
            Some(crate::keymap::KeyTrie::MappableCommand(cmd)) => {
                assert_eq!(*cmd, crate::commands::MappableCommand::dap_next);
            }
            other => panic!("[keys.debug] 的 x 应绑到 dap_next，实际 {other:?}"),
        }
        // 内置键仍在（合并而非替换）
        let n: KeyEvent = "n".parse().unwrap();
        assert!(matches!(
            cfg.keys_debug.search(&[n]),
            Some(crate::keymap::KeyTrie::MappableCommand(_))
        ));
    }

    #[test]
    fn logging_max_size_from_config_and_defaults() {
        // 不写 `[logging]` ⇒ 默认 10 MiB
        assert_eq!(
            Config::load_test("").logging.max_size,
            DEFAULT_LOG_MAX_SIZE,
            "没有 [logging] 时要有默认上限"
        );
        // 显式配置（kebab-case，与其它段一致）
        let cfg = Config::load_test(
            r#"
            [logging]
            max-size = 4096
            "#,
        );
        assert_eq!(cfg.logging.max_size, 4096);
        // 0 = 不限制（合法值，不能当成"没配"而回退默认）
        let cfg = Config::load_test(
            r#"
            [logging]
            max-size = 0
            "#,
        );
        assert_eq!(cfg.logging.max_size, 0, "0 要原样保留（= 不限制）");
        // 拼错的键要明确报错（deny-unknown-fields），而不是静默忽略
        assert!(
            toml::from_str::<ConfigRaw>("[logging]\nmax_sise = 1\n").is_err(),
            "拼错的键应报错"
        );
    }

    #[test]
    fn logging_section_is_global_only() {
        // workspace 的 `.helix/config.toml` 不该决定日志怎么存（与 workspace-trust 同理）
        let cfg = Config::load(
            Ok(&"[logging]\nmax-size = 111\n".to_owned()),
            Ok("[logging]\nmax-size = 999\n".to_owned()),
        )
        .unwrap();
        assert_eq!(cfg.logging.max_size, 111, "只认用户级 config");
    }

    /// `[plugins.<name>.keys]` → 作用域键位；`[keys.X]` 回到**解析期**严格校验。
    ///
    /// 三种形状各来一条：主作用域、辅助作用域（`keys.help` ⇒ scope `filetree.help`）、
    /// 以及拼错的 `[keys.nromal]` 必须**立刻**警告（不再等到插件加载后）。
    #[test]
    fn plugin_keys_tables_and_strict_keys_sections() {
        let cfg = Config::load_test(
            r#"
            [plugins.filetree]
            show_hidden = false

            [plugins.filetree.keys]
            "q" = "wclose"

            [plugins.filetree.keys.help]
            "esc" = "wclose"

            [keys.nromal]
            "x" = "dap_next"

            [keys.normal]
            "C-s" = "wclose"
            "#,
        );
        // ① 主作用域 + ② 辅助作用域
        assert!(
            cfg.key_scopes.contains_key("filetree"),
            "主作用域应在: {:?}",
            cfg.key_scopes.keys().collect::<Vec<_>>()
        );
        assert_eq!(
            cfg.key_scopes.get("filetree"),
            Some(&vec![("q".to_string(), "wclose".to_string())]),
            "键/命令原样保留（命令不在这里校验）"
        );
        assert!(
            cfg.key_scopes.contains_key("filetree.help"),
            "辅助作用域应在（scope 名 = 插件名.子名）: {:?} / warnings={:?}",
            cfg.key_scopes.keys().collect::<Vec<_>>(),
            cfg.warnings
        );
        // `keys` 子表已从插件配置里摘掉（否则 schema 校验会报"未声明"）
        let plugins = cfg.plugins.as_ref().expect("plugins 表还在");
        let ft = plugins.get("filetree").and_then(|v| v.as_table()).unwrap();
        assert!(!ft.contains_key("keys"), "keys 应被摘掉: {ft:?}");
        assert_eq!(
            ft.get("show_hidden").and_then(|v| v.as_bool()),
            Some(false),
            "其它配置键原样保留"
        );
        // ③ 拼错的 `[keys.X]` 解析期就警告
        assert!(
            cfg.warnings.iter().any(|w| w.contains("keys.nromal")),
            "拼错应在解析期可见: {:?}",
            cfg.warnings
        );
        // ④ 认领检查：这里没有插件加载，`filetree` 无人认领 ⇒ 启动后还会提醒一次
        let warnings = crate::commands::typed::apply_key_scope_overrides(None, &cfg);
        assert!(
            warnings.iter().any(|w| w.contains("plugins.filetree.keys")),
            "无人认领的作用域要提示新写法: {warnings:?}"
        );
    }

    /// 认领之后覆盖真的生效（`resolve` 返回命令），且不再有"无人认领"的提示
    #[test]
    fn claimed_plugin_keys_scope_applies() {
        let cfg = Config::load_test(
            r#"
            [plugins.demo.keys]
            "q" = "wclose"
            "#,
        );
        helix_js::init();
        let st = helix_js::host::state_handle_or_init();
        helix_js::keymap_scope::register(
            &st,
            "demo",
            helix_js::keymap_scope::Scope {
                owner: Some("demo".into()),
                ..Default::default()
            },
        )
        .unwrap();
        let warnings = crate::commands::typed::apply_key_scope_overrides(None, &cfg);
        assert!(
            !warnings.iter().any(|w| w.contains("plugins.demo.keys")),
            "已认领不该提示: {warnings:?}"
        );
        let key = helix_js::PluginKey {
            name: "q".into(),
            shift: false,
            ctrl: false,
            alt: false,
        };
        let hit =
            helix_js::keymap_scope::resolve(&st, "demo", &helix_js::keymap_scope::KeyId::from(key))
                .expect("覆盖应命中");
        assert_eq!(hit.command.as_deref(), Some("wclose"), "{hit:?}");

        helix_js::keymap_scope::drop_owned(&st, "demo");
        helix_js::keymap_scope::set_overrides(&st, Default::default());
    }

    #[test]
    fn parsing_keymaps_config_file() {
        use crate::keymap;
        use helix_core::hashmap;
        use helix_view::document::Mode;

        let sample_keymaps = r#"
            [keys.insert]
            y = "move_line_down"
            S-C-a = "delete_selection"

            [keys.normal]
            A-F12 = "move_next_word_end"
        "#;

        let mut keys = keymap::default();
        merge_keys(
            &mut keys,
            hashmap! {
                Mode::Insert => keymap!({ "Insert mode"
                    "y" => move_line_down,
                    "S-C-a" => delete_selection,
                }),
                Mode::Normal => keymap!({ "Normal mode"
                    "A-F12" => move_next_word_end,
                }),
            },
        );

        assert_eq!(
            Config::load_test(sample_keymaps),
            Config {
                keys,
                ..Default::default()
            }
        );
    }

    #[test]
    fn keys_resolve_to_correct_defaults() {
        // From serde default
        let default_keys = Config::load_test("").keys;
        assert_eq!(default_keys, keymap::default());

        // From the Default trait
        let default_keys = Config::default().keys;
        assert_eq!(default_keys, keymap::default());
    }
}

#[cfg(test)]
mod plugin_keys_parse_tests {
    use super::*;

    fn load(cfg: &str) -> Config {
        Config::load(Ok(&cfg.to_owned()), Err(ConfigLoadError::default())).unwrap()
    }

    /// **插件命令在配置解析期还不存在** —— 这里正是那个坑的回归锁：
    /// 早先 `[plugins.x.keys]` 用 `KeyTrie` 解析，`:filetree-find` 会被判成
    /// "No TypableCommand" 并让**整段**作用域被丢掉（连合法的宏条目一起丢）。
    #[test]
    fn plugin_commands_are_not_validated_at_parse_time() {
        let cfg = load(
            r#"
            [plugins.filetree.keys]
            "C-s" = ":filetree-find"
            "C-y" = "@:filetree-find<ret>"
            "#,
        );
        assert!(
            cfg.warnings.is_empty(),
            "解析期不该因为'插件命令未注册'报警: {:?}",
            cfg.warnings
        );
        assert_eq!(
            cfg.key_scopes.get("filetree"),
            Some(&vec![
                ("C-s".to_string(), ":filetree-find".to_string()),
                ("C-y".to_string(), "@:filetree-find<ret>".to_string()),
            ]),
            "两条都该原样保留"
        );
    }

    /// 键名写错 / 多键序列 / 值不是字符串 ⇒ 解析期就警告（这些**不需要**插件在场就能判）
    #[test]
    fn bad_key_names_warn_at_parse_time() {
        let cfg = load(
            r#"
            [plugins.demo.keys]
            "<esc>" = ":wclose"
            "gg" = ":wclose"
            "x" = 42
            "q" = "wclose"
            "#,
        );
        let w = cfg.warnings.join(" | ");
        assert!(w.contains("<esc>"), "`<esc>` 这种 nvim 写法应被点出来: {w}");
        assert!(w.contains("gg"), "多键序列应被点出来: {w}");
        assert!(w.contains("42"), "值不是字符串应被点出来: {w}");
        assert_eq!(
            cfg.key_scopes.get("demo"),
            Some(&vec![("q".to_string(), "wclose".to_string())]),
            "合法的只剩 q: {:?}",
            cfg.key_scopes.get("demo")
        );
    }
}

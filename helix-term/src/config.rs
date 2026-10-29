use crate::keymap;
use crate::keymap::{merge_keys, KeyTrie};
use helix_loader::merge_toml_values;
use helix_view::{document::Mode, theme};
use serde::Deserialize;
use std::collections::HashMap;
use std::fmt::Display;
use std::fs;
use std::io::Error as IOError;
use toml::de::Error as TomlError;

#[derive(Debug, Clone, PartialEq)]
pub struct Config {
    pub theme: Option<theme::Config>,
    pub keys: HashMap<Mode, KeyTrie>,
    /// 调试键表（`dap_keymap()` 之上合并了 `[keys.debug]`；暂停时 `input_router` 用它）
    pub keys_debug: KeyTrie,
    /// 加载期产生的**用户可见**警告（如未知的 `[keys.X]`）：启动时显示一次
    pub warnings: Vec<String>,
    pub editor: helix_view::editor::Config,
    /// config.toml 的 `[plugins] / [plugins.<name>]` 段(插件配置;原始 TOML 表)
    pub plugins: Option<toml::Table>,
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
                            // 吃掉这个值，警告后跳过（不要让整份配置作废）
                            let _: toml::Value = m.next_value()?;
                            let msg = format!(
                                "[keys.{other}] 不是已知的键位模式（只有 normal/insert/select/debug）\
                                 ⇒ 已忽略这一段"
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
}

impl Default for Config {
    fn default() -> Config {
        Config {
            theme: None,
            keys: keymap::default(),
            keys_debug: keymap::default::dap_keymap(),
            warnings: Vec::new(),
            editor: helix_view::editor::Config::default(),
            plugins: None,
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

                Config {
                    theme: local.theme.or(global.theme),
                    keys,
                    keys_debug,
                    warnings,
                    editor,
                    plugins: local.plugins.or(global.plugins),
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
                Config {
                    theme: config.theme,
                    keys,
                    keys_debug,
                    warnings,
                    editor: config.editor.map_or_else(
                        || Ok(helix_view::editor::Config::default()),
                        |val| val.try_into().map_err(ConfigLoadError::BadConfig),
                    )?,
                    plugins: config.plugins,
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
    fn unknown_keys_section_only_warns() {
        // 写错段名不再让整份配置失败（早先会报 Invalid mode）
        let cfg = Config::load_test(
            r#"
            [keys.nromal]
            "x" = "dap_next"
            [keys.normal]
            "C-s" = "wclose"
            "#,
        );
        assert!(!cfg.warnings.is_empty(), "应有可见警告");
        assert!(cfg.keys_debug.search(&["n".parse().unwrap()]).is_some());
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

pub mod default;
pub mod macros;

pub use crate::commands::MappableCommand;
pub use default::default;

use arc_swap::{ArcSwap, Guard};
use helix_view::{document::Mode, info::Info, input::KeyEvent};
use indexmap::IndexMap;
use macros::key;
use serde::Deserialize;
use std::{
    borrow::Cow,
    collections::{BTreeSet, HashMap},
    ops::{Deref, DerefMut},
    sync::Arc,
};

#[derive(Debug, Clone, Default, Deserialize)]
pub struct KeyTrieNode {
    /// A label for keys coming under this node, like "Goto mode"
    #[serde(skip)]
    name: String,
    #[serde(flatten)]
    map: IndexMap<KeyEvent, KeyTrie>,
    #[serde(skip)]
    pub is_sticky: bool,
}

impl KeyTrieNode {
    pub fn new(name: &str, map: IndexMap<KeyEvent, KeyTrie>) -> Self {
        Self {
            name: name.to_string(),
            map,
            is_sticky: false,
        }
    }

    /// Merge another Node in. Leaves and subnodes from the other node replace
    /// corresponding keyevent in self, except when both other and self have
    /// subnodes for same key. In that case the merge is recursive.
    pub fn merge(&mut self, mut other: Self) {
        for (key, trie) in std::mem::take(&mut other.map) {
            if let Some(KeyTrie::Node(node)) = self.map.get_mut(&key) {
                if let KeyTrie::Node(other_node) = trie {
                    node.merge(other_node);
                    continue;
                }
            }
            self.map.insert(key, trie);
        }
    }

    pub fn infobox(&self) -> Info {
        Info::new(self.name.clone(), &self.entries())
    }

    /// 前缀提示:优先 JS set_keymap_hint 回调(文本 + 位置),无回调/返回 null → 内置 Info
    pub fn hint_info(&self) -> Info {
        if let Some((text, position)) = helix_js::keymap_hint(&self.name, &self.entries()) {
            let width = text.lines().map(|l| l.chars().count()).max().unwrap_or(0) as u16;
            let height = text.lines().count() as u16;
            let mut info = Info {
                title: Cow::Owned(self.name.clone()),
                text,
                width: width.saturating_add(2),
                height,
                position: Default::default(),
            };
            info.position = hint_position(&position);
            return info;
        }
        self.infobox()
    }

    /// 键位条目：(键组合, 说明)——JS keymap 提示(set_keymap_hint)与内置 Info 共用数据
    pub fn entries(&self) -> Vec<(String, String)> {
        let mut body: Vec<(BTreeSet<KeyEvent>, &str)> = Vec::with_capacity(self.len());
        for (&key, trie) in self.iter() {
            let desc = match trie {
                KeyTrie::MappableCommand(cmd) => {
                    if cmd.name() == "no_op" {
                        continue;
                    }
                    cmd.doc()
                }
                KeyTrie::Node(n) => &n.name,
                KeyTrie::Sequence(_) => "[Multiple commands]",
            };
            match body.iter().position(|(_, d)| d == &desc) {
                Some(pos) => {
                    body[pos].0.insert(key);
                }
                None => body.push((BTreeSet::from([key]), desc)),
            }
        }

        body.into_iter()
            .map(|(events, desc)| {
                let events = events.iter().map(ToString::to_string).collect::<Vec<_>>();
                (events.join(", "), desc.to_string())
            })
            .collect()
    }
}

/// JS 位置字符串 → InfoPosition(未知值回退右下角)
pub fn hint_position(pos: &str) -> helix_view::info::InfoPosition {
    match pos {
        "bottom-left" => helix_view::info::InfoPosition::BottomLeft,
        "top-right" => helix_view::info::InfoPosition::TopRight,
        "top-left" => helix_view::info::InfoPosition::TopLeft,
        "center" => helix_view::info::InfoPosition::Center,
        _ => helix_view::info::InfoPosition::BottomRight,
    }
}

impl PartialEq for KeyTrieNode {
    fn eq(&self, other: &Self) -> bool {
        self.map == other.map
    }
}

impl Deref for KeyTrieNode {
    type Target = IndexMap<KeyEvent, KeyTrie>;

    fn deref(&self) -> &Self::Target {
        &self.map
    }
}

impl DerefMut for KeyTrieNode {
    fn deref_mut(&mut self) -> &mut Self::Target {
        &mut self.map
    }
}

#[derive(Debug, Clone, PartialEq)]
pub enum KeyTrie {
    MappableCommand(MappableCommand),
    Sequence(Vec<MappableCommand>),
    Node(KeyTrieNode),
}

impl<'de> Deserialize<'de> for KeyTrie {
    fn deserialize<D>(deserializer: D) -> Result<Self, D::Error>
    where
        D: serde::Deserializer<'de>,
    {
        deserializer.deserialize_any(KeyTrieVisitor)
    }
}

struct KeyTrieVisitor;

impl<'de> serde::de::Visitor<'de> for KeyTrieVisitor {
    type Value = KeyTrie;

    fn expecting(&self, formatter: &mut std::fmt::Formatter) -> std::fmt::Result {
        write!(formatter, "a command, list of commands, or sub-keymap")
    }

    fn visit_str<E>(self, command: &str) -> Result<Self::Value, E>
    where
        E: serde::de::Error,
    {
        command
            .parse::<MappableCommand>()
            .map(KeyTrie::MappableCommand)
            .map_err(E::custom)
    }

    fn visit_seq<S>(self, mut seq: S) -> Result<Self::Value, S::Error>
    where
        S: serde::de::SeqAccess<'de>,
    {
        let mut commands = Vec::new();
        while let Some(command) = seq.next_element::<String>()? {
            commands.push(
                command
                    .parse::<MappableCommand>()
                    .map_err(serde::de::Error::custom)?,
            )
        }

        // Prevent macro keybindings from being used in command sequences.
        // This is meant to be a temporary restriction pending a larger
        // refactor of how command sequences are executed.
        if commands
            .iter()
            .any(|cmd| matches!(cmd, MappableCommand::Macro { .. }))
        {
            return Err(serde::de::Error::custom(
                "macro keybindings may not be used in command sequences",
            ));
        }

        Ok(KeyTrie::Sequence(commands))
    }

    fn visit_map<M>(self, mut map: M) -> Result<Self::Value, M::Error>
    where
        M: serde::de::MapAccess<'de>,
    {
        let mut mapping = IndexMap::new();
        while let Some((key, value)) = map.next_entry::<KeyEvent, KeyTrie>()? {
            mapping.insert(key, value);
        }
        Ok(KeyTrie::Node(KeyTrieNode::new("", mapping)))
    }
}

impl KeyTrie {
    pub fn reverse_map(&self) -> ReverseKeymap {
        // recursively visit all nodes in keymap
        fn map_node(cmd_map: &mut ReverseKeymap, node: &KeyTrie, keys: &mut Vec<KeyEvent>) {
            match node {
                KeyTrie::MappableCommand(MappableCommand::Macro { .. }) => {}
                KeyTrie::MappableCommand(cmd) => {
                    let name = cmd.name();
                    if name != "no_op" {
                        cmd_map.entry(name.into()).or_default().push(keys.clone())
                    }
                }
                KeyTrie::Node(next) => {
                    for (key, trie) in &next.map {
                        keys.push(*key);
                        map_node(cmd_map, trie, keys);
                        keys.pop();
                    }
                }
                KeyTrie::Sequence(_) => {}
            };
        }

        let mut res = HashMap::new();
        map_node(&mut res, self, &mut Vec::new());
        res
    }

    pub fn node(&self) -> Option<&KeyTrieNode> {
        match *self {
            KeyTrie::Node(ref node) => Some(node),
            KeyTrie::MappableCommand(_) | KeyTrie::Sequence(_) => None,
        }
    }

    pub fn node_mut(&mut self) -> Option<&mut KeyTrieNode> {
        match *self {
            KeyTrie::Node(ref mut node) => Some(node),
            KeyTrie::MappableCommand(_) | KeyTrie::Sequence(_) => None,
        }
    }

    /// Merge another KeyTrie in, assuming that this KeyTrie and the other
    /// are both Nodes. Panics otherwise.
    pub fn merge_nodes(&mut self, mut other: Self) {
        let node = std::mem::take(other.node_mut().unwrap());
        self.node_mut().unwrap().merge(node);
    }

    /// Descend a trie following the given path of keys
    pub fn search(&self, keys: &[KeyEvent]) -> Option<&KeyTrie> {
        let mut trie = self;
        for key in keys {
            trie = match trie {
                KeyTrie::Node(map) => map.get(key),
                // leaf encountered while keys left to process
                KeyTrie::MappableCommand(_) | KeyTrie::Sequence(_) => None,
            }?
        }
        Some(trie)
    }
}

#[derive(Debug, Clone, PartialEq)]
pub enum KeymapResult {
    /// Needs more keys to execute a command. Contains valid keys for next keystroke.
    Pending(KeyTrieNode),
    Matched(MappableCommand),
    /// Matched a sequence of commands to execute.
    MatchedSequence(Vec<MappableCommand>),
    /// Key was not found in the root keymap
    NotFound,
    /// Key is invalid in combination with previous keys. Contains keys leading upto
    /// and including current (invalid) key.
    Cancelled(Vec<KeyEvent>),
}

/// A map of command names to keybinds that will execute the command.
pub type ReverseKeymap = HashMap<String, Vec<Vec<KeyEvent>>>;

pub struct Keymaps {
    /// 具体 ArcSwap：既能 load（按键查询）也能 store（插件键位注入）。
    /// ponytail: 启动快照——config-reload 不再热更新键位（需重启才能重载插件绑定）。
    pub map: ArcSwap<HashMap<Mode, KeyTrie>>,
    /// Stores pending keys waiting for the next key. This is relative to a
    /// sticky node if one is in use.
    state: Vec<KeyEvent>,
    /// Stores the sticky node if one is activated.
    pub sticky: Option<KeyTrieNode>,
}

impl Keymaps {
    pub fn new(map: HashMap<Mode, KeyTrie>) -> Self {
        Self {
            map: ArcSwap::from_pointee(map),
            state: Vec::new(),
            sticky: None,
        }
    }

    pub fn map(&self) -> Guard<Arc<HashMap<Mode, KeyTrie>>> {
        self.map.load()
    }

    /// 插件键位注入：已解析的键序列 → 链式 KeyTrieNode（叶子 MappableCommand）→ merge_keys → store。
    /// 覆盖语义：同键已有绑定（含内置）被插件绑定替换（merge_nodes 叶子替换）。
    pub fn insert_binding(&self, mode: Mode, keys: &[KeyEvent], command: MappableCommand) {
        let mut trie = KeyTrie::MappableCommand(command);
        for key in keys.iter().rev() {
            let mut map = IndexMap::new();
            map.insert(*key, trie);
            trie = KeyTrie::Node(KeyTrieNode::new("plugin-map", map));
        }
        let mut current = (**self.map.load()).clone();
        merge_keys(&mut current, HashMap::from([(mode, trie)]));
        self.map.store(Arc::new(current));
    }

    /// 摘掉一条绑定（`insert_binding` 的**逆操作**）。
    ///
    /// 为何需要：键位是**全局表**（`view.keymaps`）✗ ——“只在这个 buffer 生效”无法直接表达。
    /// 替代做法：**开屏时绑、关屏时解绑**（dashboard 用）⇒ 需要真正的“摘除”，
    /// 而不是把键改成 `no_op` ✗（那会**吞掉**按键，让 `f` 变死键）。
    ///
    /// 语义：沿键序列走到叶并摘除；若中间节点因此**变空**，向上回收 ✓；
    /// 键序列不存在 → **幂等**（不报错）✓
    pub fn remove_binding(&self, mode: Mode, keys: &[KeyEvent]) {
        let mut current = (**self.map.load()).clone();
        if let Some(trie) = current.get_mut(&mode) {
            Self::remove_from_trie(trie, keys);
        }
        self.map.store(Arc::new(current));
    }

    /// 在 trie 里沿 `keys` 摘除叶；若节点因此变空，返回 `true`（交由调用者摘掉该节点）✓
    fn remove_from_trie(trie: &mut KeyTrie, keys: &[KeyEvent]) -> bool {
        let (Some((first, rest)), KeyTrie::Node(node)) = (keys.split_first(), &mut *trie) else {
            return false;
        };
        if rest.is_empty() {
            node.map.shift_remove(first);
        } else if let Some(child) = node.map.get_mut(first) {
            if Self::remove_from_trie(child, rest) {
                node.map.shift_remove(first);
            }
        }
        node.map.is_empty()
    }

    /// Returns list of keys waiting to be disambiguated in current mode.
    pub fn pending(&self) -> &[KeyEvent] {
        &self.state
    }

    pub fn sticky(&self) -> Option<&KeyTrieNode> {
        self.sticky.as_ref()
    }

    pub fn contains_key(&self, mode: Mode, key: KeyEvent) -> bool {
        let keymaps = &*self.map();
        let keymap = &keymaps[&mode];
        keymap
            .search(self.pending())
            .and_then(KeyTrie::node)
            .is_some_and(|node| node.contains_key(&key))
    }

    /// Lookup `key` in the keymap to try and find a command to execute. Escape
    /// key cancels pending keystrokes. If there are no pending keystrokes but a
    /// sticky node is in use, it will be cleared.
    pub fn get(&mut self, mode: Mode, key: KeyEvent) -> KeymapResult {
        // TODO: remove the sticky part and look up manually
        let keymaps = &*self.map();
        let keymap = &keymaps[&mode];

        if key!(Esc) == key {
            if !self.state.is_empty() {
                // Note that Esc is not included here
                return KeymapResult::Cancelled(self.state.drain(..).collect());
            }
            self.sticky = None;
        }

        let first = self.state.first().unwrap_or(&key);
        let trie_node = match self.sticky {
            Some(ref trie) => Cow::Owned(KeyTrie::Node(trie.clone())),
            None => Cow::Borrowed(keymap),
        };

        let trie = match trie_node.search(&[*first]) {
            Some(KeyTrie::MappableCommand(ref cmd)) => {
                return KeymapResult::Matched(cmd.clone());
            }
            Some(KeyTrie::Sequence(ref cmds)) => {
                return KeymapResult::MatchedSequence(cmds.clone());
            }
            None => return KeymapResult::NotFound,
            Some(t) => t,
        };

        self.state.push(key);
        match trie.search(&self.state[1..]) {
            Some(KeyTrie::Node(map)) => {
                if map.is_sticky {
                    self.state.clear();
                    self.sticky = Some(map.clone());
                }
                KeymapResult::Pending(map.clone())
            }
            Some(KeyTrie::MappableCommand(cmd)) => {
                self.state.clear();
                KeymapResult::Matched(cmd.clone())
            }
            Some(KeyTrie::Sequence(cmds)) => {
                self.state.clear();
                KeymapResult::MatchedSequence(cmds.clone())
            }
            None => KeymapResult::Cancelled(self.state.drain(..).collect()),
        }
    }
}

impl Default for Keymaps {
    fn default() -> Self {
        Self::new(default())
    }
}

/// Merge default config keys with user overwritten keys for custom user config.
pub fn merge_keys(dst: &mut HashMap<Mode, KeyTrie>, mut delta: HashMap<Mode, KeyTrie>) {
    for (mode, keys) in dst {
        keys.merge_nodes(
            delta
                .remove(mode)
                .unwrap_or_else(|| KeyTrie::Node(KeyTrieNode::default())),
        )
    }
}

#[cfg(test)]
mod tests {
    /// `remove_binding` 是 `insert_binding` 的逆操作:摘叶 + 空节点回收 + 幂等 ✓
    ///
    /// 两点踩坑记录(都是本测试前几版失败的原因 ✗):
    ///  1. 构造必须**从默认键表起步** —— `insert_binding` 只 merge 进**已有模式**,
    ///     空表(`Keymaps::new(HashMap::new())`)里永远合不进去 ✗
    ///  2. 探针必须用 **`get`** —— `contains_key` 的语义是"给定 **pending 前缀**后该节点是否含此键",
    ///     前缀为空时**恒为 false** ✗,并且在该模式无条目时还会索引 panic ✗
    ///   还有:别假设任何键"默认未绑"(如 `Q` 默认就是绑的 ✗)⇒ 只断言"是否命中**我们绑的那条**" ✓
    #[test]
    fn remove_binding_is_inverse_of_insert() {
        // 注:`get` 取 `&mut self`(要记录 pending 状态)✗ ⇒ 必须 `mut` ✓
        let mut keymaps = Keymaps::new(default());
        let k: KeyEvent = "Q".parse().unwrap();

        keymaps.insert_binding(Mode::Normal, &[k], MappableCommand::no_op);
        // 注:`MappableCommand::no_op` 是**函数指针** ✗ ⇒ 不能用 `matches!` 模式匹配;
        // 照既有测试用 `assert_eq!` 比较**值** ✓
        assert_eq!(
            keymaps.get(Mode::Normal, k),
            KeymapResult::Matched(MappableCommand::no_op),
            "插入后应命中我们绑的命令"
        );

        keymaps.remove_binding(Mode::Normal, &[k]);
        assert_ne!(
            keymaps.get(Mode::Normal, k),
            KeymapResult::Matched(MappableCommand::no_op),
            "摘除后不应再命中那条命令"
        );

        // 幂等:再摘一次不 panic;不存在的键序列也不 panic ✓
        keymaps.remove_binding(Mode::Normal, &[k]);
        keymaps.remove_binding(Mode::Normal, &["F12".parse().unwrap()]);
    }

    use super::macros::keymap;
    use super::*;
    use crate::commands::MappableCommand;
    use helix_core::hashmap;
    use helix_view::input::{KeyCode, KeyEvent, KeyModifiers};
    use indexmap::indexmap;

    #[test]
    #[should_panic]
    fn duplicate_keys_should_panic() {
        keymap!({ "Normal mode"
            "i" => normal_mode,
            "i" => goto_definition,
        });
    }

    #[test]
    fn check_duplicate_keys_in_default_keymap() {
        // will panic on duplicate keys, assumes that `Keymaps` uses keymap! macro
        Keymaps::default();
    }

    #[test]
    fn merge_partial_keys() {
        let keymap = hashmap! {
            Mode::Normal => keymap!({ "Normal mode"
                "i" => normal_mode,
                "无" => insert_mode,
                "z" => jump_backward,
                "g" => { "Merge into goto mode"
                    "$" => goto_line_end,
                    "g" => delete_char_forward,
                },
            })
        };
        let mut merged_keyamp = default();
        merge_keys(&mut merged_keyamp, keymap.clone());
        assert_ne!(keymap, merged_keyamp);

        let mut keymap = Keymaps::new(merged_keyamp.clone());
        assert_eq!(
            keymap.get(Mode::Normal, key!('i')),
            KeymapResult::Matched(MappableCommand::normal_mode),
            "Leaf should replace leaf"
        );
        assert_eq!(
            keymap.get(Mode::Normal, key!('无')),
            KeymapResult::Matched(MappableCommand::insert_mode),
            "New leaf should be present in merged keymap"
        );
        // Assumes that z is a node in the default keymap
        assert_eq!(
            keymap.get(Mode::Normal, key!('z')),
            KeymapResult::Matched(MappableCommand::jump_backward),
            "Leaf should replace node"
        );

        let keymap = merged_keyamp.get_mut(&Mode::Normal).unwrap();
        // Assumes that `g` is a node in default keymap
        assert_eq!(
            keymap.search(&[key!('g'), key!('$')]).unwrap(),
            &KeyTrie::MappableCommand(MappableCommand::goto_line_end),
            "Leaf should be present in merged subnode"
        );
        // Assumes that `gg` is in default keymap
        assert_eq!(
            keymap.search(&[key!('g'), key!('g')]).unwrap(),
            &KeyTrie::MappableCommand(MappableCommand::delete_char_forward),
            "Leaf should replace old leaf in merged subnode"
        );
        // Assumes that `ge` is in default keymap
        assert_eq!(
            keymap.search(&[key!('g'), key!('e')]).unwrap(),
            &KeyTrie::MappableCommand(MappableCommand::goto_last_line),
            "Old leaves in subnode should be present in merged node"
        );

        assert!(
            merged_keyamp
                .get(&Mode::Normal)
                .and_then(|key_trie| key_trie.node())
                .unwrap()
                .len()
                > 1
        );
        assert!(!merged_keyamp
            .get(&Mode::Insert)
            .and_then(|key_trie| key_trie.node())
            .unwrap()
            .is_empty());
    }

    #[test]
    fn order_should_be_set() {
        let keymap = hashmap! {
            Mode::Normal => keymap!({ "Normal mode"
                "space" => { ""
                    "s" => { ""
                        "v" => vsplit,
                        "c" => hsplit,
                    },
                },
            })
        };
        let mut merged_keymap = default();
        merge_keys(&mut merged_keymap, keymap.clone());
        assert_ne!(keymap, merged_keymap);
        let keymap = merged_keymap.get_mut(&Mode::Normal).unwrap();
        // Make sure mapping works
        assert_eq!(
            keymap.search(&[key!(' '), key!('s'), key!('v')]).unwrap(),
            &KeyTrie::MappableCommand(MappableCommand::vsplit),
            "Leaf should be present in merged subnode"
        );
        // Merged nodes were ordered at the end
        let node = keymap.search(&[key!(' '), key!('s')]).unwrap();
        assert_eq!(
            node.node().unwrap().keys().copied().collect::<Vec<_>>(),
            vec![key!('v'), key!('c')]
        );
    }

    #[test]
    fn aliased_modes_are_same_in_default_keymap() {
        let keymaps = Keymaps::default().map();
        let root = keymaps.get(&Mode::Normal).unwrap();
        // `C-w` 别名已删(阶段①:窗口操作改走 compositor 层的平级模式 `C-p`),
        // 上游的 `Space-w` 窗口子树仍在,窗口命令从这里可达。
        assert!(
            root.search(&[key!(' '), key!('w')]).is_some(),
            "Space-w 窗口子树应保留"
        );
        assert!(
            root.search(&["C-w".parse::<KeyEvent>().unwrap()]).is_none(),
            "C-w 别名已删(阶段①)"
        );
        assert_eq!(
            root.search(&[key!('z')]).unwrap(),
            root.search(&[key!('Z')]).unwrap(),
            "Mismatch for view mode on `z` and `Z`"
        );
    }

    #[test]
    fn reverse_map() {
        let normal_mode = keymap!({ "Normal mode"
            "i" => insert_mode,
            "g" => { "Goto"
                "g" => goto_file_start,
                "e" => goto_file_end,
            },
            "j" | "k" => move_line_down,
        });
        let keymap = normal_mode;
        let mut reverse_map = keymap.reverse_map();

        // sort keybindings in order to have consistent tests
        // HashMaps can be compared but we can still get different ordering of bindings
        // for commands that have multiple bindings assigned
        for v in reverse_map.values_mut() {
            v.sort()
        }

        assert_eq!(
            reverse_map,
            HashMap::from([
                ("insert_mode".to_string(), vec![vec![key!('i')]]),
                (
                    "goto_file_start".to_string(),
                    vec![vec![key!('g'), key!('g')]]
                ),
                (
                    "goto_file_end".to_string(),
                    vec![vec![key!('g'), key!('e')]]
                ),
                (
                    "move_line_down".to_string(),
                    vec![vec![key!('j')], vec![key!('k')]]
                ),
            ]),
            "Mismatch"
        )
    }

    /// Deserialize into KeyTrieNode
    #[test]
    fn deserialize_node() {
        let keys = r#"
"+" = "select_all"
a = "append_mode"
        "#;
        let expectation = KeyTrie::Node(KeyTrieNode::new(
            "",
            indexmap! {
                key!('+') => KeyTrie::MappableCommand(
                    MappableCommand::select_all
                ),
                key!('a') => KeyTrie::MappableCommand(
                    MappableCommand::append_mode
                ),
            },
        ));

        assert_eq!(toml::from_str(keys), Ok(expectation));

        // Other fields in KeyTrieNode CANNOT be deserialized
        let invalid = r#"
name = "name"
is_sticky = false
        "#;
        let result = toml::from_str::<KeyTrieNode>(invalid);
        assert!(result.is_err_and(|error| error.message().contains("Invalid key code 'is_sticky'")));
    }

    #[test]
    fn escaped_keymap() {
        let keys = r#"
"+" = [
    "select_all",
    ":pipe sed -E 's/\\s+$//g'",
]
        "#;

        let key = KeyEvent {
            code: KeyCode::Char('+'),
            modifiers: KeyModifiers::NONE,
        };

        let expectation = KeyTrie::Node(KeyTrieNode::new(
            "",
            indexmap! {
                key => KeyTrie::Sequence(vec!{
                    MappableCommand::select_all,
                    MappableCommand::Typable {
                        name: "pipe".to_string(),
                        args: "sed -E 's/\\s+$//g'".to_string(),
                        doc: "".to_string(),
                    },
                })
            },
        ));

        assert_eq!(toml::from_str(keys), Ok(expectation));
    }
}

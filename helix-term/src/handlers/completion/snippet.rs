use std::{borrow::Cow, collections::HashMap, sync::Arc};

use helix_core::completion::CompletionProvider;
use helix_event::TaskHandle;
use helix_view::{document::SavePoint, handlers::completion::ResponseContext, Editor};

use serde::Deserialize;

use super::{
    request::Trigger, CompletionItem, CompletionItems, CompletionResponse, SnippetCompletionItem,
};

const SNIPPETS_DIR: &str = "snippets";

#[derive(Debug, serde::Deserialize)]
struct SnippetDef {
    #[serde(default, deserialize_with = "one_or_many")]
    prefix: Vec<String>,
    body: Vec<String>,
    description: Option<String>,
}

/// friendly-snippets 允许 prefix 为字符串或字符串数组。
fn one_or_many<'de, D>(deserializer: D) -> Result<Vec<String>, D::Error>
where
    D: serde::Deserializer<'de>,
{
    #[derive(serde::Deserialize)]
    #[serde(untagged)]
    enum OneOrMany {
        One(String),
        Many(Vec<String>),
    }
    Ok(match OneOrMany::deserialize(deserializer)? {
        OneOrMany::One(s) => vec![s],
        OneOrMany::Many(v) => v,
    })
}

fn build_items(raw: &str) -> Option<Vec<CompletionItem>> {
    // 外层整体反序列化:坏 JSON(非对象)直接 None;好 JSON 逐条解析,单条失败 log + 跳过,不崩整文件
    let parsed: HashMap<String, serde_json::Value> = serde_json::from_str(raw).ok()?;
    Some(
        parsed
            .into_iter()
            .filter_map(|(name, value)| {
                let def: SnippetDef = match serde_json::from_value(value) {
                    Ok(def) => def,
                    Err(e) => {
                        log::warn!("snippet {name} parse failed: {e}");
                        return None;
                    }
                };
                let label: Cow<'static, str> = def.prefix.into_iter().next()?.into();
                let body = def.body.join("\n");
                Some(CompletionItem::Snippet(SnippetCompletionItem {
                    label,
                    body,
                    description: def.description,
                    provider_priority: 0,
                    match_indices: Vec::new(),
                }))
            })
            .collect(),
    )
}

pub(super) fn completion(
    editor: &Editor,
    _trigger: Trigger,
    handle: TaskHandle,
    savepoint: Arc<SavePoint>,
) -> Option<impl FnOnce() -> CompletionResponse> {
    let (_, doc) = current_ref!(editor);
    let language = doc.language_name()?;
    let dir = helix_loader::config_dir().join(SNIPPETS_DIR);
    let path = dir.join(format!("{language}.json"));
    let path = if path.exists() {
        path
    } else {
        dir.join("all.json")
    };
    if !path.exists() {
        return None;
    }
    Some(move || {
        if handle.is_canceled() {
            return CompletionResponse {
                items: CompletionItems::Other(Vec::new()),
                provider: CompletionProvider::Snippet,
                context: ResponseContext {
                    is_incomplete: false,
                    priority: 0,
                    savepoint,
                },
            };
        }
        let raw = std::fs::read_to_string(&path).unwrap_or_default();
        let items = build_items(&raw).unwrap_or_default();
        CompletionResponse {
            items: CompletionItems::Other(items),
            provider: CompletionProvider::Snippet,
            context: ResponseContext {
                is_incomplete: false,
                priority: 0,
                savepoint,
            },
        }
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::collections::HashMap;

    const GOOD: &str = r#"{
      "fn": { "prefix": "fn", "body": ["function ${1:name}() {", "\t${0}", "}"], "description": "Function" },
      "for": { "prefix": ["for", "forof"], "body": ["for (const ${1:x} of ${2:iter}) {", "\t${0}", "}"] }
    }"#;

    #[test]
    fn parse_snippets_json() {
        let parsed: HashMap<String, SnippetDef> = serde_json::from_str(GOOD).unwrap();
        assert_eq!(parsed.len(), 2);
        let f = &parsed["fn"];
        assert_eq!(f.prefix, vec!["fn".to_string()]);
        assert_eq!(f.body.join("\n"), "function ${1:name}() {\n\t${0}\n}");
        assert_eq!(f.description.as_deref(), Some("Function"));
        // 多 prefix:首 prefix 为 label
        let fo = &parsed["for"];
        assert_eq!(fo.prefix, vec!["for".to_string(), "forof".to_string()]);
    }

    #[test]
    fn parse_bad_json_does_not_panic() {
        // 非法 JSON → Err(不是 panic);构建候选返回空
        let items = build_items(r#"{ "broken" "#).unwrap_or_default();
        assert!(items.is_empty());
    }

    #[test]
    fn build_items_uses_first_prefix_and_body() {
        let items = build_items(GOOD).unwrap();
        assert_eq!(items.len(), 2);
        // HashMap 迭代顺序不确定:按 label 找,不依赖 items[0]
        let item = items
            .iter()
            .find(|item| matches!(item, CompletionItem::Snippet(s) if s.label.as_ref() == "fn"))
            .expect("fn snippet present");
        match item {
            CompletionItem::Snippet(s) => {
                assert_eq!(s.label.as_ref(), "fn");
                assert!(s.body.contains("${1:name}"));
                assert_eq!(s.description.as_deref(), Some("Function"));
                assert_eq!(s.provider_priority, 0);
            }
            _ => panic!("expected snippet item"),
        }
    }

    #[test]
    fn build_items_skips_bad_entry_keeps_good() {
        // 混入坏条目(body 类型错 + prefix 缺失):好条目保留,坏条目跳过并 log,不崩整文件
        let raw = r#"{
      "fn": { "prefix": "fn", "body": ["function ${1:name}() {", "\t${0}", "}"] },
      "broken": { "prefix": "br", "body": "not an array" },
      "no-prefix": { "body": ["x"] }
    }"#;
        let items = build_items(raw).unwrap();
        assert_eq!(items.len(), 1);
        match &items[0] {
            CompletionItem::Snippet(s) => assert_eq!(s.label.as_ref(), "fn"),
            _ => panic!("expected snippet item"),
        }
    }
}

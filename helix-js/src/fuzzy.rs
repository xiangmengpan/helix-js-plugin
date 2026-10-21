//! `helix.fuzzy.*` —— **进程内**模糊匹配（nucleo，与核心 picker 同一套）。
//!
//! ## 为什么在核心（而不是让插件自己写打分）
//!
//! 2026-10-21 的引擎横评实测：5000 行在手写 JS 打分器里是 **7.1 ms/次**（boa）。
//! picker 的 `refilter` 是**每次按键**都跑的，7.1ms 会直接吃掉半个帧预算；5 万行时 71ms。
//! 这是 `docs/plugin-vs-core.md` 的 G1/G2：**机制**（怎么匹配、怎么打分）留核心，
//! **策略**（匹配谁、取多少、怎么展示）留插件。
//!
//! ## 为什么是"会话句柄"而不是 `filter(query, items)`
//!
//! 先做过一版一次性 `filter(query, items)`，实测**比 JS 打分还慢**（boa 11.0ms vs 7.1ms）：
//! 把 5000 个字符串逐项搬过 JS↔Rust 边界的成本（每项一次分配 + 拷贝）比打分本身贵一个数量级。
//! 所以数据**只搬一次**：`session(items)` 建快照，之后每次按键只传 `(id, query)`，
//! 回来的是 `limit` 条 `{index, score}`（默认 200 条）。
//!
//! ## 语义（定死，避免调用方猜）
//!
//! * `session(items)` ⇒ id；`items` 必须全是字符串（非字符串项**报错**，不静默跳过）；
//! * `run(id, query, opts)`：
//!   - `query` 为空 ⇒ `[]`（"不过滤"是调用方的决定，这里不隐含）；
//!   - 返回 `[{index, score}]`，按 score 降序，同分按下标升序（**稳定、可复现**）；
//!   - `opts.limit` 排序**之后**截断（默认 200，`0` = 不限）；
//!   - `opts.case` = `"smart"`（默认）/ `"insensitive"` / `"sensitive"`；
//!   - 未知 id ⇒ 报错（静默返回空会让"忘了建 session"变成难查的空列表）；
//! * `drop(id)`：释放快照（picker 关闭 / 条目集变化时调）；未知 id 是 no-op。
//!
//! 会话**按宿主隔离**（`JsHost.fuzzy_sessions`），随 `:plugin-reload` 一起清空。

use std::collections::HashMap;

use boa_engine::{
    object::builtins::JsArray, object::ObjectInitializer, property::Attribute, Context, JsObject,
    JsString, JsValue,
};
use nucleo::pattern::{CaseMatching, Normalization, Pattern};
use nucleo::{Config, Matcher, Utf32Str};

/// `helix.fuzzy.session(items)` → id
pub(crate) fn js_fuzzy_session(
    _this: &JsValue,
    args: &[JsValue],
    ctx: &mut Context,
) -> boa_engine::JsResult<JsValue> {
    let items = args
        .first()
        .and_then(|v| v.as_object())
        .ok_or_else(|| crate::js_err("helix.fuzzy.session: items must be an array of strings"))?;
    let len = items
        .get(JsString::from("length"), ctx)
        .ok()
        .and_then(|v| v.as_number())
        .unwrap_or(0.0) as usize;

    let mut values: Vec<String> = Vec::with_capacity(len);
    for i in 0..len {
        let v = items
            .get(i as u32, ctx)
            .map_err(|e| crate::js_err(format!("helix.fuzzy.session: items[{i}]: {e}")))?;
        let s: String = v.try_js_into(ctx).map_err(|_| {
            crate::js_err(format!(
                "helix.fuzzy.session: items[{i}] is not a string（只接受字符串数组）"
            ))
        })?;
        values.push(s);
    }

    let id = crate::state::next_fuzzy_session_id();
    crate::state::with_fuzzy_sessions(|m| {
        m.insert(id, values);
    });
    Ok(JsValue::from(id as f64))
}

/// `helix.fuzzy.run(id, query, opts?)` → `[{index, score}]`
pub(crate) fn js_fuzzy_run(
    _this: &JsValue,
    args: &[JsValue],
    ctx: &mut Context,
) -> boa_engine::JsResult<JsValue> {
    let id = args
        .first()
        .and_then(|v| v.as_number())
        .ok_or_else(|| crate::js_err("helix.fuzzy.run: id must be a number"))?
        as u64;
    let query: String = args
        .get(1)
        .unwrap_or(&JsValue::undefined())
        .try_js_into(ctx)
        .map_err(|_| crate::js_err("helix.fuzzy.run: query must be a string"))?;

    let (mut limit, mut case) = (200usize, CaseMatching::Smart);
    if let Some(opts) = args.get(2).and_then(|v| v.as_object()) {
        if let Ok(v) = opts.get(JsString::from("limit"), ctx) {
            if let Some(n) = v.as_number() {
                limit = n.max(0.0) as usize;
            }
        }
        if let Ok(v) = opts.get(JsString::from("case"), ctx) {
            if let Ok(s) = v.try_js_into::<String>(ctx) {
                case = match s.as_str() {
                    "sensitive" => CaseMatching::Respect,
                    "insensitive" => CaseMatching::Ignore,
                    "smart" => CaseMatching::Smart,
                    other => {
                        return Err(crate::js_err(format!(
                            "helix.fuzzy.run: opts.case must be smart|insensitive|sensitive, got '{other}'"
                        )))
                    }
                };
            }
        }
    }

    let out = JsArray::new(ctx)?;
    if query.is_empty() {
        return Ok(JsValue::from(out));
    }

    // ⚠ **借着打分，不要克隆**：5000 条 String 的克隆本身是 0.2ms 量级（实测），
    // 而打分函数不碰宿主状态 ⇒ 可以在借用里跑完。
    let hits = crate::state::with_fuzzy_sessions(|m| {
        let Some(items) = m.get(&id) else {
            return None;
        };
        let mut hits = score(&query, items, case);
        hits.sort_by(|a, b| b.1.cmp(&a.1).then(a.0.cmp(&b.0)));
        if limit > 0 {
            hits.truncate(limit);
        }
        Some(hits)
    })
    .ok_or_else(|| {
        crate::js_err(format!(
            "helix.fuzzy.run: 未知 session {id}（先 helix.fuzzy.session(items)，或它已被 drop）"
        ))
    })?;
    for (idx, score) in hits {
        let obj = ObjectInitializer::new(ctx)
            .property(JsString::from("index"), idx as f64, Attribute::all())
            .property(JsString::from("score"), f64::from(score), Attribute::all())
            .build();
        out.push(JsValue::from(obj), ctx)?;
    }
    Ok(JsValue::from(out))
}

/// `helix.fuzzy.drop(id)`
pub(crate) fn js_fuzzy_drop(
    _this: &JsValue,
    args: &[JsValue],
    _ctx: &mut Context,
) -> boa_engine::JsResult<JsValue> {
    if let Some(id) = args.first().and_then(|v| v.as_number()) {
        crate::state::with_fuzzy_sessions(|m| {
            m.remove(&(id as u64));
        });
    }
    Ok(JsValue::undefined())
}

/// 纯函数（可单测，不需要引擎）：返回 `(下标, 分数)`，未命中不出现。
fn score(query: &str, items: &[String], case: CaseMatching) -> Vec<(usize, u32)> {
    // 空 query：nucleo 的空 Pattern 会给**每个**条目 0 分，这里显式改成"不匹配任何人"
    // （"不过滤"是调用方的决定，本 API 不隐含它）
    if query.is_empty() {
        return Vec::new();
    }
    let pattern = Pattern::parse(query, case, Normalization::Smart);
    let mut matcher = Matcher::new(Config::DEFAULT);
    let mut buf = Vec::new();
    let mut out = Vec::new();
    for (i, s) in items.iter().enumerate() {
        if let Some(score) = pattern.score(Utf32Str::new(s, &mut buf), &mut matcher) {
            out.push((i, score));
        }
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    fn ask(q: &str, items: &[&str]) -> Vec<usize> {
        let owned: Vec<String> = items.iter().map(|s| s.to_string()).collect();
        let mut v = score(q, &owned, CaseMatching::Smart);
        v.sort_by(|a, b| b.1.cmp(&a.1).then(a.0.cmp(&b.0)));
        v.into_iter().map(|(i, _)| i).collect()
    }

    #[test]
    fn subsequence_hits_in_order() {
        // "abc" 是子序列 ⇒ 命中；顺序不对/缺字符 ⇒ 不命中
        assert_eq!(ask("abc", &["a_b_c", "abc", "acb", "nope"]), vec![1, 0]);
    }

    #[test]
    fn smart_case_ignores_lowercase_query() {
        assert_eq!(ask("abc", &["ABC"]), vec![0]);
        // 查询里带大写 ⇒ 变敏感
        assert!(ask("Abc", &["abc"]).is_empty());
    }

    #[test]
    fn empty_query_scores_nothing() {
        assert!(score("", &["x".to_string()], CaseMatching::Smart).is_empty());
    }

    /// 会话句柄：建 → 查 → drop 后再查报错（不静默返回空）
    #[test]
    fn session_roundtrip_and_drop() {
        crate::init();
        let ctx = &mut boa_engine::Context::default();
        let items = JsArray::new(ctx).unwrap();
        for s in ["a_b_c", "abc", "zzz"] {
            items.push(JsString::from(s), ctx).unwrap();
        }
        let id = js_fuzzy_session(
            &JsValue::undefined(),
            &[JsValue::from(items)],
            ctx,
        )
        .unwrap()
        .as_number()
        .unwrap() as u64;
        assert!(crate::state::with_fuzzy_sessions(|m| m.contains_key(&id)));

        let hits = js_fuzzy_run(
            &JsValue::undefined(),
            &[JsValue::from(id as f64), JsValue::from(JsString::from("abc"))],
            ctx,
        )
        .unwrap();
        let arr = hits.as_object().unwrap();
        assert_eq!(
            arr.get(JsString::from("length"), ctx)
                .unwrap()
                .as_number()
                .unwrap(),
            2.0,
            "abc 命中 a_b_c 与 abc"
        );

        js_fuzzy_drop(&JsValue::undefined(), &[JsValue::from(id as f64)], ctx).unwrap();
        let err = js_fuzzy_run(
            &JsValue::undefined(),
            &[JsValue::from(id as f64), JsValue::from(JsString::from("a"))],
            ctx,
        )
        .unwrap_err();
        assert!(format!("{err}").contains("未知 session"), "{err}");
    }
}

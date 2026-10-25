//! 断点的**唯一真相**（`BreakpointStore`）。
//!
//! 为什么从 `Editor.breakpoints` 搬到这里：
//!
//! - 断点是**跨会话**状态：launch 之前就能设（`dap_toggle_breakpoint` 在没有 active client 时
//!   只往表里写），attach 之后要按 adapter 的响应回填 `verified/id/message`；
//! - 现在"谁推 `setBreakpoints`"散在 `helix-view/src/handlers/dap.rs`（`breakpoints_changed`）
//!   与 `helix-term/src/commands/dap.rs`（toggle）两处，回填只在其中一处 —— 两处真相必然漂移；
//! - 多会话需要对**同一份**断点表按会话重放（`Initialized` 时），表必须是共享的。
//!
//! 字段与 `helix_view::editor::Breakpoint` 一一对应（那条路径在后续任务里换成这里），
//! 其中 `verified/id/message` 是 **adapter 回填** 的三个字段。
//!
//! 行号口径：**内部一律 0 起始**（与编辑器/JS API 一致），只在 `source_breakpoints()`
//! 发请求时 +1。换算收在这一个函数里 —— 以前散在两处，是最容易出 off-by-one 的地方。

use helix_dap_types::{Breakpoint as DapBreakpoint, SourceBreakpoint};
use std::collections::BTreeMap;
use std::path::{Path, PathBuf};

/// 一条断点。`line` 0 起始。
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct Breakpoint {
    /// adapter 分配的 id（回填）
    pub id: Option<usize>,
    /// adapter 是否已确认（回填）
    pub verified: bool,
    /// adapter 给的说明（回填，如 "no executable"）
    pub message: Option<String>,

    pub line: usize,
    pub column: Option<usize>,
    pub condition: Option<String>,
    pub hit_condition: Option<String>,
    pub log_message: Option<String>,
}

impl Breakpoint {
    pub fn new(line: usize) -> Self {
        Self {
            line,
            ..Default::default()
        }
    }

    /// 是否带"条件类"设置（adapter 能力检查用：条件断点 / 命中条件 / 日志断点）
    pub fn is_conditional(&self) -> bool {
        self.condition.is_some()
    }
    pub fn has_hit_condition(&self) -> bool {
        self.hit_condition.is_some()
    }
    pub fn is_log_point(&self) -> bool {
        self.log_message.is_some()
    }
}

/// 断点表：`路径 → (行 → 断点)`。
///
/// 用 `BTreeMap` 而不是 `HashMap`：UI 需要**稳定顺序**（断点列表按 path 字典序、行号升序），
/// 而 `BTreeMap` 的迭代顺序天然就是这个，省掉"每次列一遍就 sort 一遍"。
#[derive(Debug, Default)]
pub struct BreakpointStore {
    inner: BTreeMap<PathBuf, BTreeMap<usize, Breakpoint>>,
}

impl BreakpointStore {
    pub fn new() -> Self {
        Self::default()
    }

    /// 切换某行（有就删、没有就加默认断点）。返回 `true` = 当前已设置。
    ///
    /// 语义与 `:debug-toggle-breakpoint` 一致：删掉就彻底忘掉（不保留条件）。
    pub fn toggle(&mut self, path: impl Into<PathBuf>, line: usize) -> bool {
        let path = path.into();
        let map = self.inner.entry(path.clone()).or_default();
        if map.remove(&line).is_some() {
            // 删到空就把路径也摘掉:留空条目会让 `paths()` 多出一个"有断点"的假文件,
            // 而 `Initialized` 重放时会对它发空 `setBreakpoints`。
            if map.is_empty() {
                self.inner.remove(&path);
            }
            return false;
        }
        map.insert(line, Breakpoint::new(line));
        true
    }

    /// 覆盖式设置（带条件/日志时用）。`bp.line` 决定落点。
    pub fn set(&mut self, path: impl Into<PathBuf>, bp: Breakpoint) {
        self.inner
            .entry(path.into())
            .or_default()
            .insert(bp.line, bp);
    }

    pub fn remove(&mut self, path: &Path, line: usize) -> bool {
        let Some(map) = self.inner.get_mut(path) else {
            return false;
        };
        let removed = map.remove(&line).is_some();
        if map.is_empty() {
            self.inner.remove(path);
        }
        removed
    }

    /// 清空：`Some(path)` 只清那个文件，`None` 清全部。返回删掉的条数。
    pub fn clear(&mut self, path: Option<&Path>) -> usize {
        match path {
            Some(p) => self.inner.remove(p).map(|m| m.len()).unwrap_or(0),
            None => {
                let n = self.inner.values().map(|m| m.len()).sum();
                self.inner.clear();
                n
            }
        }
    }

    pub fn get(&self, path: &Path, line: usize) -> Option<&Breakpoint> {
        self.inner.get(path)?.get(&line)
    }

    /// 当前行是否已有断点（`:debug-toggle-breakpoint` / gutter 渲染用）
    pub fn has(&self, path: &Path, line: usize) -> bool {
        self.get(path, line).is_some()
    }

    fn find_by_id(&self, id: usize) -> Option<(PathBuf, usize)> {
        for (path, map) in &self.inner {
            if let Some((line, _)) = map.iter().find(|(_, bp)| bp.id == Some(id)) {
                return Some((path.clone(), *line));
            }
        }
        None
    }

    /// 按 adapter 的 id 找断点并改它（返回命中的 path 与行号）。**不改行号**，改行号用
    /// [`BreakpointStore::apply_changed`]（那里要重排 map 的键）。
    pub fn update_by_id(
        &mut self,
        id: usize,
        f: impl FnOnce(&mut Breakpoint),
    ) -> Option<(PathBuf, usize)> {
        let (path, line) = self.find_by_id(id)?;
        let bp = self.inner.get_mut(&path)?.get_mut(&line)?;
        f(bp);
        Some((path, line))
    }

    /// `breakpoint` 事件（`reason: "changed"`）的回填：按 id 找到、改字段、**必要时按新行号重排**。
    ///
    /// 单独一个方法（而不是让调用方 `update_by_id` 顺手改 `line`）：直接改 `bp.line` 会让
    /// map 的键与值不一致 —— 之后 `has(path, line)` 全是错的。这类"键值不同步"的 bug
    /// 在 gutter 上表现为"断点点不中"，极难查。
    pub fn apply_changed(
        &mut self,
        id: usize,
        verified: bool,
        message: Option<String>,
        line: Option<usize>,
        column: Option<usize>,
    ) -> Option<(PathBuf, usize)> {
        let (path, old_line) = self.find_by_id(id)?;
        let map = self.inner.get_mut(&path)?;
        let mut bp = map.remove(&old_line)?;
        bp.verified = verified;
        if message.is_some() {
            bp.message = message;
        }
        if let Some(line) = line {
            bp.line = line;
        }
        if column.is_some() {
            bp.column = column;
        }
        let new_line = bp.line;
        map.insert(new_line, bp);
        Some((path, new_line))
    }

    /// 按 adapter 的 id 删除（`removed` 事件用）。返回删掉的 (path, line)。
    pub fn remove_by_id(&mut self, id: usize) -> Option<(PathBuf, usize)> {
        let (path, line) = self.find_by_id(id)?;
        let map = self.inner.get_mut(&path)?;
        map.remove(&line);
        if map.is_empty() {
            self.inner.remove(&path);
        }
        Some((path, line))
    }

    /// 把所有断点标记为"未确认"（会话结束/断开时用：UI 上应从 ● 变 ◯）。
    ///
    /// 返回被改动的条数（0 时调用方不必重绘）。
    pub fn invalidate_all(&mut self) -> usize {
        let mut n = 0;
        for map in self.inner.values_mut() {
            for bp in map.values_mut() {
                if bp.verified {
                    bp.verified = false;
                    n += 1;
                }
            }
        }
        n
    }

    /// 某个文件的断点（按行号升序），UI/能力检查用。
    pub fn for_path(&self, path: &Path) -> Vec<Breakpoint> {
        self.inner
            .get(path)
            .map(|m| m.values().cloned().collect())
            .unwrap_or_default()
    }

    /// 全表快照，**顺序稳定**：路径字典序 → 行号升序。
    pub fn list(&self, path: Option<&Path>) -> Vec<(PathBuf, Breakpoint)> {
        let mut out = Vec::new();
        for (p, map) in &self.inner {
            if path.is_some_and(|want| want != p.as_path()) {
                continue;
            }
            for bp in map.values() {
                out.push((p.clone(), bp.clone()));
            }
        }
        out
    }

    /// 有断点的文件列表（`Initialized` 时要按文件逐个重放）。
    pub fn paths(&self) -> Vec<PathBuf> {
        self.inner.keys().cloned().collect()
    }

    pub fn len(&self) -> usize {
        self.inner.values().map(|m| m.len()).sum()
    }

    pub fn is_empty(&self) -> bool {
        self.len() == 0
    }

    /// 发 `setBreakpoints` 用的载荷（**这里**做 0 → 1 起始换算）。
    pub fn source_breakpoints(&self, path: &Path) -> Vec<SourceBreakpoint> {
        let Some(map) = self.inner.get(path) else {
            return Vec::new();
        };
        map.values()
            .map(|bp| SourceBreakpoint {
                line: bp.line + 1,
                column: bp.column,
                condition: bp.condition.clone(),
                hit_condition: bp.hit_condition.clone(),
                log_message: bp.log_message.clone(),
            })
            .collect()
    }

    /// 把 adapter 的响应回填到表里（`verified/id/message`/`line`）。
    ///
    /// 按**顺序** zip（DAP 规定响应里的 breakpoints 与请求顺序对应），长度不一致时按短的来
    /// —— 不能 panic：adapter 乱回是它的 bug，不是我们的崩溃点。
    pub fn apply_response(&mut self, path: &Path, breakpoints: &[DapBreakpoint]) {
        let Some(map) = self.inner.get_mut(path) else {
            return;
        };
        let mut entries: Vec<(usize, Breakpoint)> =
            map.iter().map(|(l, b)| (*l, b.clone())).collect();
        for ((old_line, bp), dap_bp) in entries.iter_mut().zip(breakpoints.iter()) {
            bp.id = dap_bp.id;
            bp.verified = dap_bp.verified;
            bp.message = dap_bp.message.clone();
            if let Some(line) = dap_bp.line {
                // adapter 回的是 1 起始（我们 initialize 时声明了 lines_start_at_one）
                bp.line = line.saturating_sub(1);
            }
            if let Some(column) = dap_bp.column {
                bp.column = Some(column);
            }
            // 行号可能被 adapter 挪动 ⇒ 用新行号重排（旧键删掉）
            if bp.line != *old_line {
                map.remove(old_line);
                map.insert(bp.line, bp.clone());
            } else {
                map.insert(*old_line, bp.clone());
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn p(s: &str) -> PathBuf {
        PathBuf::from(s)
    }

    #[test]
    fn toggle_flips_and_keeps_map_clean() {
        let mut st = BreakpointStore::new();
        assert_eq!(st.len(), 0);
        assert!(st.toggle("/a.rs", 3), "第一次 = 设上");
        assert!(st.has(&p("/a.rs"), 3));
        assert_eq!(st.len(), 1);
        assert!(!st.toggle("/a.rs", 3), "第二次 = 删掉");
        assert!(!st.has(&p("/a.rs"), 3));
        assert_eq!(st.len(), 0);
        assert!(st.paths().is_empty(), "空了就不该留空路径");
    }

    #[test]
    fn list_is_stable_path_then_line() {
        let mut st = BreakpointStore::new();
        st.toggle("/b.rs", 10);
        st.toggle("/a.rs", 9);
        st.toggle("/a.rs", 2);
        let got: Vec<(String, usize)> = st
            .list(None)
            .into_iter()
            .map(|(p, b)| (p.display().to_string(), b.line))
            .collect();
        assert_eq!(
            got,
            vec![
                ("/a.rs".to_string(), 2),
                ("/a.rs".to_string(), 9),
                ("/b.rs".to_string(), 10)
            ],
            "路径字典序 → 行号升序（UI 直接按这个渲染）"
        );
        // 只看一个文件
        let one = st.list(Some(&p("/a.rs")));
        assert_eq!(one.len(), 2);
    }

    #[test]
    fn set_keeps_conditions_and_clear_scoped() {
        let mut st = BreakpointStore::new();
        st.set(
            "/a.rs",
            Breakpoint {
                line: 4,
                condition: Some("x > 1".into()),
                log_message: Some("hit {x}".into()),
                ..Default::default()
            },
        );
        st.toggle("/other.rs", 1);
        let bp = st.get(&p("/a.rs"), 4).expect("有断点");
        assert!(bp.is_conditional() && bp.is_log_point());
        assert_eq!(st.clear(Some(&p("/a.rs"))), 1, "只清这个文件");
        assert_eq!(st.len(), 1, "别的文件还在");
        assert_eq!(st.clear(None), 1, "全清");
        assert!(st.is_empty());
    }

    #[test]
    fn source_breakpoints_convert_to_one_based() {
        let mut st = BreakpointStore::new();
        st.set(
            "/a.rs",
            Breakpoint {
                line: 0,
                hit_condition: Some("5".into()),
                ..Default::default()
            },
        );
        st.toggle("/a.rs", 41);
        let sent = st.source_breakpoints(&p("/a.rs"));
        assert_eq!(sent.len(), 2);
        assert_eq!(sent[0].line, 1, "0 起始 → 1 起始");
        assert_eq!(sent[0].hit_condition.as_deref(), Some("5"));
        assert_eq!(sent[1].line, 42);
        assert!(
            st.source_breakpoints(&p("/nope.rs")).is_empty(),
            "没断点的文件发空列表（用于清掉 adapter 侧残留）"
        );
    }

    #[test]
    fn adapter_events_update_and_remove_by_id() {
        let mut st = BreakpointStore::new();
        st.set(
            "/a.rs",
            Breakpoint {
                line: 3,
                id: Some(11),
                ..Default::default()
            },
        );
        st.set(
            "/b.rs",
            Breakpoint {
                line: 5,
                id: Some(12),
                ..Default::default()
            },
        );
        // `changed` 事件：按 id 找到并改
        let hit = st
            .update_by_id(12, |bp| {
                bp.verified = true;
                bp.message = Some("ok".into());
            })
            .expect("id 12 在表里");
        assert_eq!(hit.0, p("/b.rs"));
        assert!(st.get(&p("/b.rs"), 5).unwrap().verified);
        // 不存在的 id 不炸
        assert!(st.update_by_id(99, |_| {}).is_none());
        // `removed` 事件：按 id 删；空了不留空路径
        assert_eq!(st.remove_by_id(11).unwrap().0, p("/a.rs"));
        assert!(!st.has(&p("/a.rs"), 3));
        assert!(st.paths().iter().all(|path| path == &p("/b.rs")));
    }

    #[test]
    fn apply_response_backfills_and_renumbers() {
        let mut st = BreakpointStore::new();
        st.toggle("/a.rs", 3);
        st.toggle("/a.rs", 7);
        // adapter 把第 7 行挪到第 8 行，并确认了第一条
        st.apply_response(
            &p("/a.rs"),
            &[
                DapBreakpoint {
                    id: Some(11),
                    verified: true,
                    message: None,
                    source: None,
                    line: Some(4),
                    column: None,
                    end_line: None,
                    end_column: None,
                    instruction_reference: None,
                    offset: None,
                },
                DapBreakpoint {
                    id: Some(12),
                    verified: false,
                    message: Some("no executable".into()),
                    source: None,
                    line: Some(8),
                    column: Some(5),
                    end_line: None,
                    end_column: None,
                    instruction_reference: None,
                    offset: None,
                },
            ],
        );
        let first = st.get(&p("/a.rs"), 3).expect("行号没变的那条还在原位");
        assert_eq!(first.id, Some(11));
        assert!(first.verified);
        let moved = st.get(&p("/a.rs"), 7).expect("被挪到第 8 行（0 起始 7）");
        assert_eq!(moved.column, Some(5));
        assert!(!moved.verified);
        assert_eq!(moved.message.as_deref(), Some("no executable"));
        assert_eq!(st.len(), 2, "重排不能变成 3 条");
    }

    #[test]
    fn apply_response_shorter_than_request_does_not_panic() {
        let mut st = BreakpointStore::new();
        st.toggle("/a.rs", 1);
        st.toggle("/a.rs", 2);
        // adapter 只回一条：按短的来，不 panic
        st.apply_response(
            &p("/a.rs"),
            &[DapBreakpoint {
                id: Some(1),
                verified: true,
                message: None,
                source: None,
                line: Some(2),
                column: None,
                end_line: None,
                end_column: None,
                instruction_reference: None,
                offset: None,
            }],
        );
        assert_eq!(st.len(), 2);
        // adapter 回 line=2（1 起始）⇒ 0 起始 1，正好是第一条原本的位置
        assert!(st.get(&p("/a.rs"), 1).unwrap().verified, "第一条被回填");
        assert!(!st.get(&p("/a.rs"), 2).unwrap().verified, "第二条没被碰");
    }
}

//! 插件 picker 的 term 侧驱动（任务 3）：OpenPicker 请求 → 原生 Picker 层；
//! Enter/选中 经 source 名查 JS 侧 define 的配置回调（action/preview 桥）。

use std::cell::RefCell;
use std::path::{Path, PathBuf};

use crate::compositor::Compositor;
use crate::ui::overlay::overlaid;
use crate::ui::picker::{Column, ColumnFormatFn, PathOrId, Picker};
use helix_js::picker::RowSpec;
use tui::widgets::Cell;

/// 插件 picker 的候选行（T = menu 行数据）：cells 各列文本；payload 原样数据
#[derive(Debug, Clone)]
pub struct PickerRow {
    pub cells: Vec<String>,
    pub payload: Vec<String>,
}

/// 列格式：取 row.cells[I]（const 泛型把列索引烘进 fn 指针——ColumnFormatFn 是 fn 指针，闭包不能捕获）
fn column_format<const I: usize>() -> ColumnFormatFn<PickerRow, ()> {
    |row: &PickerRow, _: &()| Cell::from(row.cells.get(I).cloned().unwrap_or_default())
}

/// 列构建：第一列参与过滤（filter），其余列仅展示（without_filtering）。
/// format 按列索引取 row.cells[i]，越界 → 空串（行间列数不一致不 panic）。
fn build_columns(names: &[String]) -> Vec<Column<PickerRow, ()>> {
    names
        .iter()
        .enumerate()
        .map(|(i, name)| {
            // 各臂是同一 fn 指针类型,无需标注 ColumnFormatFn(其为 picker.rs 私有别名)
            let format = match i {
                0 => column_format::<0>(),
                1 => column_format::<1>(),
                2 => column_format::<2>(),
                3 => column_format::<3>(),
                4 => column_format::<4>(),
                5 => column_format::<5>(),
                6 => column_format::<6>(),
                7 => column_format::<7>(),
                // ponytail: 上限 8 列(插件 picker 常见 1-3 列);超列回退空串,需要时改固定上限 + 循环
                _ => column_format::<{ usize::MAX }>(),
            };
            let mut col = Column::new(name.clone(), format);
            if i > 0 {
                col = col.without_filtering();
            }
            col
        })
        .collect()
}

// JS preview 返回的路径是运行时 owned String，而 with_preview 的 file_fn 只给 &'a Path
// （返回数据必须活得比调用长）。方案：线程局部槽缓存最近一次路径，Box::leak 出 'static
// 引用（同路径复用不重复泄漏，泄漏有界：本次 picker 会话浏览过的不同文件数）。
// 安全性：get_preview 拿到 path 后同步拷进 preview_cache（Arc<Path>），单线程渲染内
// 无覆盖窗口；leak 的引用永不释放，生命周期恒有效。
thread_local! {
    static PREVIEW_PATH: RefCell<Option<(String, &'static Path)>> = const { RefCell::new(None) };
}

/// 从 rows + 源名构建原生 Picker 并推入 compositor（overlaid 覆盖层）。
/// action/preview 回调经 source 名查 JS 侧 define 的配置；未定义/抛错 → 回退无操作。
pub(crate) fn open_picker(
    compositor: &mut Compositor,
    source: &str,
    rows: Vec<RowSpec>,
    columns: Vec<String>,
) {
    // 无列（空 rows 或空 cells 数组）→ 核心 Picker 断言非空列，跳过并提示
    if columns.is_empty() {
        log::warn!("picker '{source}': no columns, skipping");
        return;
    }
    let options: Vec<PickerRow> = rows
        .into_iter()
        .map(|r| PickerRow {
            cells: r.cells,
            payload: r.payload,
        })
        .collect();
    let action_src = source.to_string();
    let preview_src = source.to_string();
    let picker = Picker::new(
        build_columns(&columns),
        0,
        options,
        (),
        move |_cx: &mut crate::compositor::Context, row, _action| {
            // Enter：action 回调（payload 数组）；未定义/抛错 → 忽略
            helix_js::picker::invoke_action(&action_src, &row.payload);
        },
    );
    let picker = picker.with_preview(move |_editor: &helix_view::Editor, row: &PickerRow| {
        // 选中行：preview 回调 → {path, line}；未定义/返回 null → None（核心不显示预览）
        let (path, line) = helix_js::picker::invoke_preview(&preview_src, &row.payload)?;
        let path: &'static Path = PREVIEW_PATH.with(|slot| {
            let mut slot = slot.borrow_mut();
            if slot.as_ref().is_none_or(|(last, _)| last != &path) {
                *slot = Some((
                    path.clone(),
                    Box::leak(PathBuf::from(&path).into_boxed_path()),
                ));
            }
            slot.as_ref().unwrap().1
        });
        Some((PathOrId::Path(path), Some((line, 0))))
    });
    compositor.push(Box::new(overlaid(picker)));
}

#[cfg(test)]
mod tests {
    use super::*;
    use tui::text::Text;

    #[test]
    fn js_picker_build_columns_and_filter_first() {
        // 列:第一列 filter,其余 without_filtering
        let cols = build_columns(&["name".to_string(), "path".to_string()]);
        assert!(cols[0].filter_enabled());
        assert!(!cols[1].filter_enabled());
        // format 从 cells 取
        let row = PickerRow {
            cells: vec!["a.rs".into(), "src/a.rs".into()],
            payload: vec!["a.rs".into(), "src/a.rs".into()],
        };
        let cell = cols[0].format(&row, &());
        assert_eq!(cell.content, Text::from("a.rs"));
        let cell = cols[1].format(&row, &());
        assert_eq!(cell.content, Text::from("src/a.rs"));
        // 越界列 → 空串(不 panic)
        let empty = PickerRow {
            cells: vec![],
            payload: vec![],
        };
        let cell = cols[0].format(&empty, &());
        assert_eq!(cell.content, Text::from(""));
    }
}

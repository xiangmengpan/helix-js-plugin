# 树状组件模型实现计划（方案二-B）

> 自主执行（方案二窗口内）。TDD → 通过 → 提交；控制者自动合并（并行 wave）。

### 任务 1：组件模型（单任务）

**文件：** `helix-js/src/lib.rs`、`helix-term/src/ui/comp_layout.rs`（新）、`helix-term/src/ui/plugin_popup.rs`

- [ ] **步骤 1：失败单测（helix-js）**

```rust
#[test]
fn component_nodes() {
    let _guard = TEST_LOCK.lock().unwrap();
    init();
    load_script(
        r#"
        helix.open_popup({
            render: () => helix.el("col", [
                helix.el("text", "title", { style: "error" }),
                helix.el("row", [
                    helix.el("text", "left"),
                    helix.el("text", "right", { width: 5 }),
                ], { gap: 1 }),
                helix.el("scroll", [helix.el("text", "s1"), helix.el("text", "s2")], { height: 1 }),
            ]),
        });
        helix.register_command("tree", (ctx) => {
            const n = helix.el("text", "hello", { width: 10 });
            helix.echo("type:" + n.type + " text:" + n.text + " w:" + n.width);
        });
        "#,
    )
    .unwrap();
    let ctx = CommandContext { path: None, text: String::new(), cursor: (0, 0), selection: ((0, 0), (0, 0)) };
    assert!(run_command("tree", &ctx).unwrap());
    assert_eq!(take_messages(), vec!["type:text text:hello w:10"]);

    let reqs = take_ui_requests();
    let id = match &reqs[0] { UiRequest::OpenPopup { id, .. } => *id };
    match render_popup(id, 40, 10).unwrap() {
        Content::Tree(root) => {
            assert!(matches!(&root, CompNode::Col { .. }));
            match &root {
                CompNode::Col { children, .. } => {
                    assert_eq!(children.len(), 3);
                    assert!(matches!(&children[0], CompNode::Text { text, style: Some(s), .. } if text == "title" && s == "error"));
                    assert!(matches!(&children[1], CompNode::Row { .. }));
                    assert!(matches!(&children[2], CompNode::Scroll { .. }));
                }
                _ => panic!(),
            }
        }
        _ => panic!("expected tree"),
    }
    close_popup(id).unwrap();
    // 非法节点类型 → Err
    load_script(r#"helix.open_popup({ render: () => ({ type: "bogus" }) });"#).unwrap();
    let id = match take_ui_requests()[0] { UiRequest::OpenPopup { id, .. } => id };
    assert!(render_popup(id, 40, 10).is_err());
    close_popup(id).unwrap();
}
```

- [ ] **步骤 2：运行失败** → 编译错误（Content/CompNode/js_el 未定义）。
- [ ] **步骤 3：实现 helix-js**
  - `CompNode` 枚举（Text{text,style,width}/Row{children,gap}/Col{children,gap}/Scroll{children,height}）+ `Content` 枚举（Lines(Vec<StyledLine>)/Tree(CompNode)）
  - `render_popup`/面板 render 返回 `Content`：数组 → Lines（旧兼容）；单对象有 type → Tree（递归解析：type 白名单 + 字段校验）
  - `js_el`（arity 2-3）：type 字符串 + 参数（text 节点：text 字符串 + opts；容器：children 数组 + opts）→ 构造节点对象（JS 对象，不是 Rust——el 只是便捷构造器，实际解析在 render 时）——即 js_el 返回 `{type, ...}` JS 对象；render 解析时读它。**简化**：不做 js_el 原生函数——helix.el 可以在 JS 里构造（无原生需要）？——但 helix 没有 JS 运行时基础库……`helix.el` 必须由宿主提供（JS 侧没有）。做法：init 时注入一个 JS 函数定义（eval 一段定义 el 的脚本注册到全局）——或原生 js_el 返回对象。以原生 js_el 为准（最简单可靠）
- [ ] **步骤 4：helix-js 通过**（32 个，clippy 0）
- [ ] **步骤 5：helix-term 布局引擎**（ui/comp_layout.rs）
  - `pub fn layout(node: &CompNode, viewport: (u16, u16)) -> Vec<StyledLine>`
  - text → 一行（style 解析 + width 截断）；col → 堆叠（子节点自上而下，viewport 高度限制）；row → 并排（每个子节点布局成若干行，水平拼接：第 i 行 = 各子第 i 行拼接；宽度求和，超 viewport 截断）；scroll → 布局后保留最后 height 行
  - 单测：row 并排/col 堆叠/scroll 裁剪/嵌套/text 截断/宽度求和
- [ ] **步骤 6：弹窗/面板渲染分支**：Content::Tree → layout → 行 → 既有 Span 渲染；Lines → 原路径
- [ ] **步骤 7：集成测试**（tests/test/plugin_components.rs，临时 mod）：弹窗 render 返回组件树 → 渲染 Buffer → 断言布局（row 两列文本并排出现）
- [ ] **步骤 8：回归**（plugin 组、command_line、cargo check/clippy）
- [ ] **步骤 9：Commit** `feat(js): component tree rendering (el row/col/scroll/text)`

---

## 自检
- 风险：js_el 原生 vs JS 注入（原生为准）；Content 枚举对既有弹窗测试的波及（render_popup 返回类型变化——既有测试 `assert_eq!(lines, vec![StyledLine...])` 需适配 `Content::Lines(...)`）；布局引擎 row 的多行拼接细节（子节点高度不一致时的处理——补空行）。

; 折叠候选（`@fold`）。
;
; 原文件用的是 `@start`/`@end` + `#set! role block` 的另一种约定，且引用了本仓
; 语法版本里不存在的节点（`bracket_start`/`bracket_end`）⇒ 编译期即报错。
; 这里改成与仓内其余 46 个语言一致的 `@fold` 约定。
[
  (directive)
  (element)
] @fold

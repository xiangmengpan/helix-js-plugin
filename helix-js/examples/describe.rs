//! 打印 `helix.*` 的机器可读声明面（`cargo xtask api-describe` 用的数据源）。
//!
//! 为什么用 example 而不是测试：它要能被**不编译测试**的路径调用（xtask 在自己的
//! 进程里 spawn 它），且失败要能直接反映到退出码。
fn main() {
    println!("{}", helix_js::api::describe());
}

//! 插件能力清单与检查点(C-1 字段扩展 / C-2 三个入口)。
//!
//! 由来与策略(2026-10-14 计划 §T3):`helix.plugin(name, { deps, version })` 里
//! 没有 `permissions`,于是"插件能不能起进程 / 读写任意文件"这件事**没有任何声明**;
//! 但一步到位地"拦截未声明的操作"会让现有一切插件当场全挂 ⇒ 必须**渐进**:
//!
//! 1. **C-1 只解析、只记录**:manifest 的新字段进 `PluginManifest`,不影响行为。
//! 2. **C-2 三个入口做检查**:`helix.run` / `read_file` / `write_plugin_file`。
//! 3. **grandfathered**(本仓 17 个内置插件与所有"没声明 permissions 的插件"):
//!    - 内置插件(提供根是**用户插件目录以外**的**已知**根 ⇒ 随软件分发的 runtime)
//!      一律放行。注意判据是"已知根":绝对路径加载 / `load_script_named`(如
//!      `:plugin-load /abs/x.js`)没有提供根 ⇒ 按**第三方**对待(早先把这种情况判成
//!      内置 ⇒ 检查点对 `:plugin-load` 整体失效,见 `state::is_builtin_root`);
//!    - **没声明 `permissions`** 的 manifest = legacy 语义 ⇒ 放行(否则 369 个集成
//!      测试与所有存量插件立刻红;这也符合"先声明、再执行"的迁移顺序);
//!    - **声明了 `permissions`** 的插件才进入检查:声明的能力放行,未声明的:
//!      有记住的决定就用它,否则**问一次**(记入 pending,并**拒绝本次**)。
//!
//! "拒绝本次"而不是"静默放行"是刻意的:静默放行会让 `permissions` 变成装饰,
//! 而失败可见(错误消息 + pending 列表)才让人有机会授权。
//!
//! 端到端证据:`helix-term/tests/test/plugin_permissions.rs`(询问 → allow 放行 /
//! deny 长期生效 / 未声明 permissions 不受影响)。
//!
//! # 归属(哪个插件在调?)
//!
//! JS API 是全局的(`helix.run(...)`),运行时并不知道"这是谁在调"。归属按
//! **回调注册时**绑定(这是唯一可靠的时刻):
//!
//! * `helix.register_command` ⇒ 记下命令的属主;命令执行时用它;
//! * `helix.on(event, fn)` ⇒ 逐处理器记属主;事件分发时按处理器切换。
//!
//! 两者之外的路径(裸脚本顶层代码)归属为"最后声明的插件名"(加载期语义,
//! 与 `helix.plugin` 的调用顺序一致)。

use std::cell::RefCell;
use std::collections::HashMap;

/// 把版本串拆成数字段(`"25.07.1 (c4831272)"` → `[25, 7, 1]`)。
///
/// 宽松但有边界:每段取**前导数字**(`"1-beta"` → 1),没有数字的段记 0;
/// 整串一个数字都没有 ⇒ `None`(调用方据此决定"拒绝"还是"跳过")。
fn version_parts(v: &str) -> Option<Vec<u64>> {
    let mut out = Vec::new();
    let mut any = false;
    for seg in v.split('.') {
        let digits: String = seg.chars().take_while(|c| c.is_ascii_digit()).collect();
        let n: u64 = digits.parse().unwrap_or(0);
        if !digits.is_empty() {
            any = true;
        }
        out.push(n);
    }
    any.then_some(out)
}

/// `min_editor` 检查:`declared`(插件要求)是否**高于** `actual`(编辑器版本)。
///
/// 返回 `Ok(())` = 可以加载;`Err(消息)` = 不该加载(缺段按 0 补,`25.7` == `25.7.0`)。
pub(crate) fn min_editor_ok(declared: &str, actual: &str) -> Result<(), String> {
    let (Some(d), Some(a)) = (version_parts(declared), version_parts(actual)) else {
        return Err(format!(
            "min_editor {declared:?} 不是版本号(应是如 \"25.7\" 的形态)"
        ));
    };
    let n = d.len().max(a.len());
    for i in 0..n {
        let dv = d.get(i).copied().unwrap_or(0);
        let av = a.get(i).copied().unwrap_or(0);
        if dv > av {
            return Err(format!(
                "需要编辑器 >= {declared},当前 {actual}(升级编辑器或换用旧版插件)"
            ));
        }
        if dv < av {
            return Ok(());
        }
    }
    Ok(())
}

use crate::types::{PluginManifest, PluginPermissions};

/// 能力名:起子进程 / shell(`helix.run`、`helix.spawn`、`run_async`)
pub const CAP_PROCESS: &str = "process";
/// 能力名:读任意文件(`read_file`/`read_dir`/异步同族)
pub const CAP_FS_READ: &str = "fs_read";
/// 能力名:写文件(`write_file` 与插件自身文件的写入)
pub const CAP_FS_WRITE: &str = "fs_write";

/// 授权状态(C-2):记住的决定 + 待询问列表。按宿主隔离。
#[derive(Debug, Default)]
pub(crate) struct PermissionState {
    /// `(插件名, 能力名)` → 允许?
    pub(crate) decisions: HashMap<(String, String), bool>,
    /// 已问过、还没决定的(去重,保持发现顺序)
    pub(crate) pending: Vec<(String, String)>,
    /// 内置插件(来自用户插件目录以外的根)——永不询问
    pub(crate) builtin: HashMap<String, bool>,
    /// 加载期归属:最后一次 `helix.plugin(name, …)` 声明的名字
    pub(crate) last_declared: Option<String>,
    /// 运行时归属:当前正在执行的回调的属主
    pub(crate) current: Option<String>,
    /// 已声明过能力清单的插件(供 `:js-permission list` 与文档)
    pub(crate) manifests: HashMap<String, PluginManifest>,
    /// 注册期绑定:命令名 → 属主插件
    pub(crate) command_owners: HashMap<String, String>,
    /// 注册期绑定:事件名 → 各处理器的属主插件(下标与处理器列表对齐)
    pub(crate) handler_owners: HashMap<String, Vec<String>>,
}

thread_local! {
    /// 宿主不可用时的兜底(纯单测路径):检查点在这些线程上直接放行,不建宿主。
    static FALLBACK: RefCell<PermissionState> = RefCell::new(PermissionState::default());
}

fn with_state<T>(f: impl FnOnce(&mut PermissionState) -> T) -> T {
    // 先试宿主里的那份(正常路径);没有宿主(纯单测/无 JS)才落到线程兜底表。
    // 用 Option<FnOnce> 让闭包只在一条分支上被消费。
    let mut f = Some(f);
    let host = crate::host::try_with_permission_state(|s| (f.take().expect("只调一次"))(s));
    match host {
        Some(v) => v,
        None => FALLBACK.with(|s| (f.take().expect("上面没消费过"))(&mut s.borrow_mut())),
    }
}

/// C-1:`helix.plugin(name, {...})` 声明的 manifest 记下来(只记录,不拦截)
pub(crate) fn record_manifest(manifest: PluginManifest) {
    with_state(|s| {
        s.last_declared = Some(manifest.name.clone());
        s.manifests.insert(manifest.name.clone(), manifest);
    });
}

/// 加载完成后标注"这个插件是不是内置的"(由 `load_script_checked` 在知道
/// **提供根**时调用 —— 归属判据是根,不是名字)
pub(crate) fn mark_builtin(name: &str, builtin: bool) {
    with_state(|s| {
        s.builtin.insert(name.to_string(), builtin);
    });
}

/// 加载期归属:最后一次 `helix.plugin(name, …)` 声明的名字
pub(crate) fn current_declared() -> Option<String> {
    with_state(|s| s.last_declared.clone())
}

/// 进入新脚本 eval 前清标记(用来回答"**本次** eval 有没有声明 manifest")
pub(crate) fn begin_manifest() {
    with_state(|s| s.last_declared = None);
}

/// 设当前属主,返回上一个(供恢复)
pub(crate) fn set_current_owner(name: Option<&str>) -> Option<String> {
    with_state(|s| {
        let prev = s.current.clone();
        s.current = name.map(str::to_string);
        prev
    })
}

/// 恢复属主
pub(crate) fn restore_owner(prev: Option<String>) {
    with_state(|s| s.current = prev);
}

/// 加载期归属:该脚本声明了哪个插件名(供 `load_script_checked` 打内置标记)
pub(crate) fn last_declared() -> Option<String> {
    with_state(|s| s.last_declared.clone())
}

/// 注册期绑定:命令名 → 属主插件
pub(crate) fn bind_command_owner(command: &str, owner: &str) {
    with_state(|s| {
        s.command_owners
            .insert(command.to_string(), owner.to_string());
    })
}

/// 注册期绑定:事件处理器(按注册序号)→ 属主插件
pub(crate) fn bind_handler_owner(event: &str, owner: &str) {
    with_state(|s| {
        s.handler_owners
            .entry(event.to_string())
            .or_default()
            .push(owner.to_string());
    })
}

/// 命令的属主(运行时归属用)
pub(crate) fn command_owner(command: &str) -> Option<String> {
    with_state(|s| s.command_owners.get(command).cloned())
}

/// 事件第 `idx` 个处理器的属主
pub(crate) fn handler_owner(event: &str, idx: usize) -> Option<String> {
    with_state(|s| {
        s.handler_owners
            .get(event)
            .and_then(|v| v.get(idx).cloned())
    })
}

impl PermissionState {
    // (无方法:提供者是 `with_state` — 内部可变性收敛在一处)
}

/// 当前归属插件(运行时优先,其次加载期)
fn current_plugin() -> Option<String> {
    with_state(|s| s.current.clone().or_else(|| s.last_declared.clone()))
}

/// 放行/拒绝一次的判定(纯函数,便于单测直接打表)
///
/// 返回:`None` = 不需要询问(放行或不需要问);`Some(plugin)` = 需要问该插件
fn verdict(ctx: &RequestCtx, cap: &str) -> Verdict {
    if ctx.builtin {
        return Verdict::Allow;
    }
    match &ctx.permissions {
        // legacy:没声明 permissions ⇒ 不拦截(grandfathered)
        None => Verdict::Allow,
        Some(p) => {
            if p.allows(cap) {
                return Verdict::Allow;
            }
            match ctx.decision {
                Some(true) => Verdict::Allow,
                Some(false) => Verdict::Denied,
                None => Verdict::Ask,
            }
        }
    }
}

/// 判定所需的一切(把"读状态"与"判定"分开 ⇒ 判定可单测,不必搭引擎)
pub(crate) struct RequestCtx {
    pub(crate) plugin: String,
    pub(crate) builtin: bool,
    pub(crate) permissions: Option<PluginPermissions>,
    pub(crate) decision: Option<bool>,
}

#[derive(Debug, PartialEq, Eq)]
enum Verdict {
    Allow,
    Denied,
    Ask,
}

/// **检查点**(C-2):几个入口在真正做事之前调它。
///
/// 现在接上的入口(**同一能力覆盖同族 API**,否则"声明了 process"只拦 `run` 就是误导):
/// * `process` —— `helix.run` / `helix.run_async` / `helix.spawn`;
/// * `fs_read` —— `helix.read_file` / `helix.read_file_async` / `helix.read_dir` /
///   `helix.stat_async` / `helix.glob_async` / `helix.read_tree` / `helix.watch`;
/// * `fs_write` —— `helix.write_file` / `helix.write_file_async` / `helix.write_plugin_file` /
///   `helix.remove_plugin_file`。
///
/// * `Ok(())` = 放行;
/// * `Err(_)` = 拒绝,且消息足够让人知道下一步怎么做(谁、哪个能力、怎么授权)。
pub(crate) fn check(cap: &str) -> anyhow::Result<()> {
    // 没有宿主(纯单测 / 无 JS 场景)⇒ 不建宿主、不拦
    if !crate::host::engine_exists() {
        return Ok(());
    }
    let Some(plugin) = current_plugin() else {
        // 归属未知(裸脚本、加载期)= legacy 语义 ⇒ 放行,与"没声明 permissions"一致
        return Ok(());
    };
    let (builtin, permissions, decision) = with_state(|s| {
        (
            *s.builtin.get(&plugin).unwrap_or(&false),
            // 没有 manifest 或 manifest 里没写 `permissions` ⇒ legacy(grandfathered)
            s.manifests.get(&plugin).and_then(|m| m.permissions),
            s.decisions.get(&(plugin.clone(), cap.to_string())).copied(),
        )
    });
    let ctx = RequestCtx {
        plugin: plugin.clone(),
        builtin,
        permissions,
        decision,
    };
    match verdict(&ctx, cap) {
        Verdict::Allow => Ok(()),
        Verdict::Denied => Err(anyhow::anyhow!(
            "插件 '{}' 的能力 '{cap}' 已被拒绝(:js-permission allow {} {cap} 可改)",
            ctx.plugin,
            ctx.plugin
        )),
        Verdict::Ask => {
            let first = with_state(|s| {
                let kv = (ctx.plugin.clone(), cap.to_string());
                if s.pending.contains(&kv) {
                    false
                } else {
                    s.pending.push(kv);
                    true
                }
            });
            if first {
                // 让失败可见:状态栏也会看到这条(与 check 的 Err 一起)
                crate::host::with_messages(|m| {
                    m.push(format!(
                        "插件 '{}' 请求能力 '{cap}'(未在 manifest 声明):\
                         用 :js-permission allow {} {cap} 授权,或 deny 拒绝",
                        ctx.plugin, ctx.plugin
                    ))
                });
            }
            Err(anyhow::anyhow!(
                "插件 '{}' 未声明能力 '{cap}' —— 已记入待授权列表(首次询问)",
                ctx.plugin
            ))
        }
    }
}

/// 检查点的 **JS 绑定侧**包装:失败转成 JS 异常(带 API 名定位,插件作者一眼知道是哪个调用)。
pub(crate) fn check_js(api: &str, cap: &str) -> boa_engine::JsResult<()> {
    match check(cap) {
        Ok(()) => Ok(()),
        Err(e) => Err(boa_engine::JsError::from_opaque(boa_engine::JsValue::from(
            boa_engine::JsString::from(format!("{api}: {e}")),
        ))),
    }
}

// ── helix-term 侧接口(:js-permission 命令)─────────────────────────────

/// 记住一个决定(允许/拒绝)。返回是否真的改变了什么。
pub fn decide(plugin: &str, cap: &str, allow: bool) -> bool {
    with_state(|s| {
        s.pending.retain(|(p, c)| !(p == plugin && c == cap));
        s.decisions
            .insert((plugin.to_string(), cap.to_string()), allow)
            .is_none()
    })
}

/// **已记住的决定**(插件, 能力, 是否允许),按插件名+能力名排序 ⇒ `:js-permission list`
/// 的输出稳定可断言。
pub fn decisions() -> Vec<(String, String, bool)> {
    with_state(|s| {
        let mut v: Vec<(String, String, bool)> = s
            .decisions
            .iter()
            .map(|((p, c), allow)| (p.clone(), c.clone(), *allow))
            .collect();
        v.sort();
        v
    })
}

/// 待询问列表(供状态栏/`:js-permission list` 展示)
pub fn pending() -> Vec<(String, String)> {
    with_state(|s| s.pending.clone())
}

/// 已知的 manifest(名 → manifest);供 `:js-permission list` 展示声明面
pub fn manifests() -> Vec<PluginManifest> {
    with_state(|s| {
        let mut v: Vec<_> = s.manifests.values().cloned().collect();
        v.sort_by(|a, b| a.name.cmp(&b.name));
        v
    })
}

/// 某个能力**现在**会不会被放行(不产生副作用:不问、不记 pending)。
///
/// 语义 = "不问就放行吗":没声明 permissions / 内置插件 / 无归属 ⇒ true(legacy 放行);
/// 声明了则看声明或已记住的决定(未决定 ⇒ false,因为那次调用会被问一次)。
pub(crate) fn allowed_now(cap: &str) -> bool {
    if !crate::host::engine_exists() {
        return true;
    }
    let Some(plugin) = current_plugin() else {
        return true;
    };
    let (builtin, permissions, decision) = with_state(|s| {
        (
            *s.builtin.get(&plugin).unwrap_or(&false),
            s.manifests.get(&plugin).and_then(|m| m.permissions),
            s.decisions.get(&(plugin.clone(), cap.to_string())).copied(),
        )
    });
    match verdict(
        &RequestCtx {
            plugin,
            builtin,
            permissions,
            decision,
        },
        cap,
    ) {
        Verdict::Allow => true,
        Verdict::Denied | Verdict::Ask => false,
    }
}

/// `helix.permissions()`(JS API):当前插件**现在**能做什么。
///
/// 用途:插件据此降级(例如没有 `process` 就不去调 git,而不是先失败一次)。
/// 返回值:`{ process, fs_read, fs_write }`,语义见 [`allowed_now`]。
pub(crate) fn js_permissions(
    _this: &boa_engine::JsValue,
    _args: &[boa_engine::JsValue],
    ctx: &mut boa_engine::Context,
) -> boa_engine::JsResult<boa_engine::JsValue> {
    use boa_engine::object::ObjectInitializer;
    use boa_engine::property::Attribute;
    use boa_engine::{JsString, JsValue};
    let flag = |cap: &str| JsValue::from(allowed_now(cap));
    Ok(JsValue::from(
        ObjectInitializer::new(ctx)
            .property(
                JsString::from(CAP_PROCESS),
                flag(CAP_PROCESS),
                Attribute::all(),
            )
            .property(
                JsString::from(CAP_FS_READ),
                flag(CAP_FS_READ),
                Attribute::all(),
            )
            .property(
                JsString::from(CAP_FS_WRITE),
                flag(CAP_FS_WRITE),
                Attribute::all(),
            )
            .build(),
    ))
}

/// 清空(测试用;宿主随线程丢弃时等价)
#[cfg(test)]
pub(crate) fn reset_for_test() {
    with_state(|s| {
        s.decisions.clear();
        s.pending.clear();
        s.builtin.clear();
        s.current = None;
        s.last_declared = None;
        s.manifests.clear();
        s.command_owners.clear();
        s.handler_owners.clear();
    });
}

#[cfg(test)]
mod tests {
    use super::*;

    fn p(process: bool, fs_read: bool, fs_write: bool) -> PluginPermissions {
        PluginPermissions {
            process,
            fs_read,
            fs_write,
        }
    }

    /// C-2 判定表(纯函数):内置 / 未声明 / 已声明+已授权 / 已声明+未授权 / 已拒绝
    #[test]
    fn verdict_table() {
        let mk = |builtin, permissions, decision| RequestCtx {
            plugin: "x".into(),
            builtin,
            permissions,
            decision,
        };
        // 内置插件永不询问(17 个内置插件 grandfathered 的核心)
        assert_eq!(
            verdict(&mk(true, Some(p(false, false, false)), None), CAP_PROCESS),
            Verdict::Allow
        );
        // 没声明 permissions ⇒ legacy 放行
        assert_eq!(verdict(&mk(false, None, None), CAP_PROCESS), Verdict::Allow);
        // 声明了 process 但没声明 fs_write
        assert_eq!(
            verdict(&mk(false, Some(p(true, false, false)), None), CAP_PROCESS),
            Verdict::Allow
        );
        assert_eq!(
            verdict(&mk(false, Some(p(true, false, false)), None), CAP_FS_WRITE),
            Verdict::Ask
        );
        // 记住的决定覆盖"没声明"
        assert_eq!(
            verdict(
                &mk(false, Some(p(true, false, false)), Some(true)),
                CAP_FS_WRITE
            ),
            Verdict::Allow
        );
        assert_eq!(
            verdict(
                &mk(false, Some(p(true, false, false)), Some(false)),
                CAP_FS_WRITE
            ),
            Verdict::Denied
        );
    }

    /// `min_editor` 比较(纯函数):缺段按 0 补、宽松取前导数字、非法版本号**拒绝**
    #[test]
    fn min_editor_comparison() {
        // 编辑器 25.07.1(= [25,7,1];真实版本串带 git hash)
        assert!(min_editor_ok("25.7", "25.07.1 (c4831272)").is_ok());
        assert!(min_editor_ok("25", "25.0.0").is_ok());
        assert!(min_editor_ok("25.7.1", "25.7.1").is_ok());
        assert!(min_editor_ok("25.7.0", "25.7").is_ok(), "缺段按 0 补");
        assert!(min_editor_ok("25.8", "25.7.1").is_err(), "高于当前 ⇒ 拒");
        assert!(min_editor_ok("26", "25.99.99").is_err());
        assert!(
            min_editor_ok("25.7.1", "25.7").is_err(),
            "位数多且更高 ⇒ 拒"
        );
        // 非法版本号:报错(而不是静默跳过 —— 静默会让字段变装饰)
        let e = min_editor_ok("second", "25.7.1").unwrap_err();
        assert!(e.contains("不是版本号"), "{e}");
    }

    /// `PluginPermissions::allows` 与能力名一一对应
    #[test]
    fn permissions_allows_named_caps() {
        let only_read = PluginPermissions {
            process: false,
            fs_read: true,
            fs_write: false,
        };
        assert!(only_read.allows(CAP_FS_READ));
        assert!(!only_read.allows(CAP_FS_WRITE));
        assert!(!only_read.allows(CAP_PROCESS));
        assert!(!PluginPermissions::none().allows(CAP_FS_READ));
    }
}

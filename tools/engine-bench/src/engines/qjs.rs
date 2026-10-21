//! QuickJS（rquickjs 0.14）—— 候选引擎。桩与 boa 侧逐项对应（同名、同 arity、同返回形状）。

use std::time::Instant;

use rquickjs::{Array, Context, Ctx, Function, Object, Runtime};

use super::{script, Engine};

pub struct Qjs {
    rt: Runtime,
    ctx: Context,
    loaded: Option<&'static str>,
}

impl Qjs {
    pub fn new() -> anyhow::Result<Self> {
        let rt = Runtime::new()?;
        let ctx = Context::full(&rt)?;
        ctx.with(|ctx| -> anyhow::Result<()> {
            let globals = ctx.globals();
            let helix = Object::new(ctx.clone())?;
            helix.set("get_style", Function::new(ctx.clone(), js_get_style)?)?;
            helix.set("echo", Function::new(ctx.clone(), js_noop)?)?;
            helix.set("plugin", Function::new(ctx.clone(), js_noop)?)?;
            helix.set("register_command", Function::new(ctx.clone(), js_noop)?)?;
            helix.set("on", Function::new(ctx.clone(), js_noop)?)?;
            helix.set("map", Function::new(ctx.clone(), js_noop)?)?;
            helix.set("set_statusline", Function::new(ctx.clone(), js_noop)?)?;
            // helix.fuzzy.filter —— 与 boa 侧**同一份** nucleo 实现
            let fuzzy = Object::new(ctx.clone())?;
            fuzzy.set("session", Function::new(ctx.clone(), js_fuzzy_session)?)?;
            fuzzy.set("run", Function::new(ctx.clone(), js_fuzzy_run)?)?;
            fuzzy.set("drop", Function::new(ctx.clone(), js_fuzzy_drop)?)?;
            helix.set("fuzzy", fuzzy)?;
            globals.set("helix", helix)?;
            Ok(())
        })?;
        Ok(Self {
            rt,
            ctx,
            loaded: None,
        })
    }
}

/// `helix.get_style(scope)` —— 与 boa 侧同签名、同返回形状
fn js_get_style(name: String) -> String {
    format!("style:{name}")
}

fn js_noop() {}

/// quickjs 侧：显式 `'js` 生命周期（闭包表达不了 `for<'js>`）
fn js_fuzzy_session<'js>(ctx: Ctx<'js>, items: Array<'js>) -> rquickjs::Result<u64> {
    let mut v: Vec<String> = Vec::with_capacity(items.len());
    for i in 0..items.len() {
        v.push(items.get::<String>(i)?);
    }
    Ok(super::fuzzy::session(v))
}

fn js_fuzzy_run<'js>(
    ctx: Ctx<'js>,
    id: u64,
    query: String,
    opts: Option<Object<'js>>,
) -> rquickjs::Result<Array<'js>> {
    let limit = opts
        .and_then(|o| o.get::<_, Option<u32>>("limit").ok().flatten())
        .unwrap_or(200);
    let hits = super::fuzzy::run(id, &query, limit as usize);
    let out = Array::new(ctx.clone())?;
    for (idx, score) in hits {
        let o = Object::new(ctx.clone())?;
        o.set("index", idx)?;
        o.set("score", score)?;
        out.set(out.len(), o)?;
    }
    Ok(out)
}

fn js_fuzzy_drop(id: u64) {
    super::fuzzy::drop_session(id);
}

impl Engine for Qjs {
    fn name(&self) -> &'static str {
        "quickjs"
    }

    fn run(&mut self, scenario: &str, iters: u64) -> anyhow::Result<(u128, f64)> {
        let _ = &self.rt; // 保持 Runtime 存活
        self.ctx.with(|ctx| -> anyhow::Result<(u128, f64)> {
            if scenario == "eval" {
                let src = script("eval");
                let t0 = Instant::now();
                for _ in 0..iters {
                    ctx.eval::<(), _>(src.as_bytes())?;
                }
                return Ok((t0.elapsed().as_nanos(), iters as f64));
            }
            if self.loaded != Some(script(scenario)) {
                ctx.eval::<(), _>(script(scenario).as_bytes())?;
            }
            let run: Function = ctx.globals().get("bench_run")?;
            let t0 = Instant::now();
            let out: f64 = run.call((iters,))?;
            Ok((t0.elapsed().as_nanos(), out))
        })
    }
}

/// 让 `Ctx` 在签名里显式出现（rquickjs 的 `with` 闭包参数类型）
#[allow(dead_code)]
fn _assert_ctx_type(_: Ctx<'_>) {}

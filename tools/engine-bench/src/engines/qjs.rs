//! QuickJS（rquickjs 0.14）—— 候选引擎。桩与 boa 侧逐项对应（同名、同 arity、同返回形状）。

use std::time::Instant;

use rquickjs::{Context, Ctx, Function, Object, Runtime};

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

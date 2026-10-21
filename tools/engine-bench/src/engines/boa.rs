//! boa 0.22 —— 与产品同版本、同用法（`NativeFunction::from_fn_ptr` 注册桩）。

use std::time::Instant;

use boa_engine::{
    js_string,
    object::ObjectInitializer,
    Context, JsValue, NativeFunction, Source,
};

use super::{script, Engine};

pub struct Boa {
    ctx: Context,
    loaded: Option<&'static str>,
}

/// `helix.get_style(scope)` —— 与产品同名同参，返回一个带前缀的字符串（真实实现返回样式对象，
/// 这里只要能测"一次边界调用 + 一次字符串分配"）。
fn js_get_style(_: &JsValue, args: &[JsValue], ctx: &mut Context) -> boa_engine::JsResult<JsValue> {
    let name = args
        .first()
        .map(|v| v.to_string(ctx).map(|s| s.to_std_string_escaped()))
        .transpose()?
        .unwrap_or_default();
    Ok(JsValue::from(js_string!(format!("style:{name}"))))
}

fn js_noop(_: &JsValue, _: &[JsValue], _: &mut Context) -> boa_engine::JsResult<JsValue> {
    Ok(JsValue::undefined())
}

impl Boa {
    pub fn new() -> anyhow::Result<Self> {
        let mut ctx = Context::default();
        let helix = ObjectInitializer::new(&mut ctx)
            .function(NativeFunction::from_fn_ptr(js_get_style), js_string!("get_style"), 2)
            .function(NativeFunction::from_fn_ptr(js_noop), js_string!("echo"), 1)
            .function(NativeFunction::from_fn_ptr(js_noop), js_string!("plugin"), 2)
            .function(NativeFunction::from_fn_ptr(js_noop), js_string!("register_command"), 2)
            .function(NativeFunction::from_fn_ptr(js_noop), js_string!("on"), 2)
            .function(NativeFunction::from_fn_ptr(js_noop), js_string!("map"), 3)
            .function(NativeFunction::from_fn_ptr(js_noop), js_string!("set_statusline"), 1)
            .build();
        ctx.global_object()
            .set(js_string!("helix"), helix, false, &mut ctx)
            .map_err(|e| anyhow::anyhow!("boa: 注册 helix 桩失败: {e}"))?;
        Ok(Self { ctx, loaded: None })
    }

    fn ensure(&mut self, scenario: &str) -> anyhow::Result<()> {
        if self.loaded == Some(script_static(scenario)) {
            return Ok(());
        }
        self.ctx
            .eval(Source::from_bytes(script(scenario)))
            .map_err(|e| anyhow::anyhow!("boa eval {scenario}: {e}"))?;
        self.loaded = Some(script_static(scenario));
        Ok(())
    }
}

fn script_static(scenario: &str) -> &'static str {
    script(scenario)
}

impl Engine for Boa {
    fn name(&self) -> &'static str {
        "boa"
    }

    fn run(&mut self, scenario: &str, iters: u64) -> anyhow::Result<(u128, f64)> {
        // eval 场景：直接重量"解析 + 执行一份插件脚本"
        if scenario == "eval" {
            let src = script("eval");
            let t0 = Instant::now();
            let mut sum = 0.0;
            for _ in 0..iters {
                self.ctx
                    .eval(Source::from_bytes(src))
                    .map_err(|e| anyhow::anyhow!("boa eval: {e}"))?;
                sum += 1.0;
            }
            return Ok((t0.elapsed().as_nanos(), sum));
        }

        self.ensure(scenario)?;
        let run = self
            .ctx
            .global_object()
            .get(js_string!("bench_run"), &mut self.ctx)
            .map_err(|e| anyhow::anyhow!("boa: 取 bench_run 失败: {e}"))?
            .as_callable()
            .ok_or_else(|| anyhow::anyhow!("boa: bench_run 不存在"))?;
        let t0 = Instant::now();
        let out = run
            .call(
                &JsValue::undefined(),
                &[JsValue::from(iters as f64)],
                &mut self.ctx,
            )
            .map_err(|e| anyhow::anyhow!("boa: bench_run({iters}) 失败: {e}"))?;
        Ok((t0.elapsed().as_nanos(), out.as_number().unwrap_or(-1.0)))
    }
}

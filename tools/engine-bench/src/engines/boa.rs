//! boa 0.22 —— 与产品同版本、同用法（`NativeFunction::from_fn_ptr` 注册桩）。

use std::time::Instant;

use boa_engine::{
    js_string,
    object::{builtins::JsArray, ObjectInitializer},
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

/// `helix.fuzzy.session(items)` —— 会话式（条目只搬一次）；与产品侧同一份实现。
fn js_fuzzy_session(_: &JsValue, args: &[JsValue], ctx: &mut Context) -> boa_engine::JsResult<JsValue> {
    let Some(arr) = args.first().and_then(|v| v.as_object()) else {
        return Ok(JsValue::undefined());
    };
    let len = arr
        .get(js_string!("length"), ctx)?
        .as_number()
        .unwrap_or(0.0) as usize;
    let mut items = Vec::with_capacity(len);
    for i in 0..len {
        items.push(arr.get(i as u32, ctx)?.to_string(ctx)?.to_std_string_escaped());
    }
    Ok(JsValue::from(super::fuzzy::session(items) as f64))
}

fn js_fuzzy_run(_: &JsValue, args: &[JsValue], ctx: &mut Context) -> boa_engine::JsResult<JsValue> {
    let id = args
        .first()
        .and_then(|v| v.as_number())
        .unwrap_or(0.0) as u64;
    let query: String = args
        .get(1)
        .map(|v| v.to_string(ctx).map(|s| s.to_std_string_escaped()))
        .transpose()?
        .unwrap_or_default();
    let limit = args
        .get(2)
        .and_then(|v| v.as_object())
        .and_then(|o| o.get(js_string!("limit"), ctx).ok())
        .and_then(|v| v.as_number())
        .unwrap_or(200.0) as usize;
    let hits = super::fuzzy::run(id, &query, limit);
    let out = JsArray::new(ctx)?;
    for (idx, score) in hits {
        let o = ObjectInitializer::new(ctx)
            .property(js_string!("index"), idx as f64, Default::default())
            .property(js_string!("score"), f64::from(score), Default::default())
            .build();
        out.push(JsValue::from(o), ctx)?;
    }
    Ok(JsValue::from(out))
}

fn js_fuzzy_drop(_: &JsValue, args: &[JsValue], _: &mut Context) -> boa_engine::JsResult<JsValue> {
    if let Some(id) = args.first().and_then(|v| v.as_number()) {
        super::fuzzy::drop_session(id as u64);
    }
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
        let fuzzy = ObjectInitializer::new(&mut ctx)
            .function(NativeFunction::from_fn_ptr(js_fuzzy_session), js_string!("session"), 1)
            .function(NativeFunction::from_fn_ptr(js_fuzzy_run), js_string!("run"), 3)
            .function(NativeFunction::from_fn_ptr(js_fuzzy_drop), js_string!("drop"), 1)
            .build();
        helix
            .set(js_string!("fuzzy"), fuzzy, false, &mut ctx)
            .map_err(|e| anyhow::anyhow!("boa: 注册 fuzzy 桩失败: {e}"))?;
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

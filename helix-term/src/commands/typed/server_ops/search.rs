//! `:server search` 分支(从 `server_op` 拆出;见 architecture-audit §9 S2)。

use super::*;

pub(super) fn run(editor: &mut Editor, arg: Option<&str>) -> anyhow::Result<()> {
    let Some(kw) = arg else {
        return Err(anyhow!("usage: server search <keyword>"));
    };
    let kw = kw.to_lowercase();
    let hits: Vec<String> = registry::all()
        .into_iter()
        .filter(|s| {
            s.name.to_lowercase().contains(&kw)
                || s.languages.iter().any(|l| l.to_lowercase().contains(&kw))
        })
        .map(|s| {
            let mark = match server_manager::availability(&s.name) {
                Availability::Managed => "已装(受管)",
                Availability::Local(_) => "本机已有",
                Availability::Missing => "未装",
            };
            format!("{} ({mark}) langs={}", s.name, s.languages.join(","))
        })
        .collect();
    if hits.is_empty() {
        editor.set_status(format!("no server matches '{kw}'"));
    } else {
        editor.set_status(format!("matches: {}", hits.join("  ")));
    }
    Ok(())
}

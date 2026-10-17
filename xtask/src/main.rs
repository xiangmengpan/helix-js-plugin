mod docgen;
mod helpers;
mod path;
mod tasks;

use std::{env, error::Error};

type DynError = Box<dyn Error>;

fn main() -> Result<(), DynError> {
    let mut args = env::args().skip(1);
    let task = args.next();
    let rest: Vec<String> = args.collect();
    // dispatch 与 help 都来自 `tasks::TASKS`(G5):两处不再各写一遍任务名。
    match task {
        None => tasks::print_help(),
        Some(t) => match tasks::find(&t) {
            Some(found) => (found.run)(rest)?,
            None => return Err(format!("Invalid task name: {}", t).into()),
        },
    };
    Ok(())
}

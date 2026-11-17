//! Opt-in diagnostics: with `WINLAUNCH_LOG=<file>` set, steps that are otherwise
//! invisible (background threads, hooks) are appended to that file. Does nothing
//! otherwise, so it costs nothing in normal use. Not while elevated (see `test_env`).

use std::io::Write;

pub fn log(message: &str) {
    let Some(path) = crate::test_env("WINLAUNCH_LOG") else {
        return;
    };
    if let Ok(mut file) = std::fs::OpenOptions::new()
        .create(true)
        .append(true)
        .open(path)
    {
        let _ = writeln!(file, "[{}] {message}", std::process::id());
    }
}

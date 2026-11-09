use std::{fs::File, io::Read, path::PathBuf, time::Duration};

use due_harness::MAX_DOCUMENT_BYTES;
use due_js::Options;

fn main() -> Result<(), Box<dyn std::error::Error>> {
    let mut args = std::env::args_os().skip(1);
    let script = PathBuf::from(
        args.next()
            .ok_or("usage: due-js <workflow.js> [json-args]")?,
    );
    let arguments = args.next().unwrap_or_else(|| "{}".into());
    if args.next().is_some() {
        return Err("usage: due-js <workflow.js> [json-args]".into());
    }
    let arguments = arguments.to_str().ok_or("arguments must be UTF-8")?;
    if arguments.len() > MAX_DOCUMENT_BYTES {
        return Err("arguments exceed 1 MiB".into());
    }
    if !std::fs::metadata(&script)?.is_file() {
        return Err("script must be a regular file".into());
    }
    let mut source = String::new();
    File::open(script)?
        .take(MAX_DOCUMENT_BYTES as u64 + 1)
        .read_to_string(&mut source)?;
    let result = due_js::run(Options {
        source,
        args: serde_json::from_str(arguments)?,
        cwd: std::env::current_dir()?,
        claude_binary: std::env::var_os("DUE_JS_CLAUDE_BINARY").map(Into::into),
        codex_binary: std::env::var_os("DUE_JS_CODEX_BINARY").map(Into::into),
        timeout: Duration::from_secs(300),
    })?;
    println!("{}", serde_json::to_string(&result)?);
    Ok(())
}

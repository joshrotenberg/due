//! Historical embedded-language comparison on the supported shared runtime.

#[cfg(feature = "lua")]
mod lua;

use clap::Parser;
#[cfg(feature = "lua")]
use due_harness::Host;
use std::{path::PathBuf, time::Duration};

#[derive(Parser)]
#[command(about = "Compare embedded Lua/QuickJS clients on one Rust RPC bridge")]
struct Options {
    /// Embedded runtime: lua or js.
    language: String,
    script: PathBuf,
    /// Absolute path to a Due binary built with --features rpc.
    #[arg(long)]
    due: PathBuf,
    #[arg(long)]
    cwd: PathBuf,
    /// A new directory for original stdout bytes and the process receipt.
    #[arg(long)]
    evidence: PathBuf,
    /// JSON passed verbatim to bridge.args.
    #[arg(long, default_value = "{}")]
    args: String,
    /// Run managed Promise/coroutine workflows with supervised verifier processes.
    #[arg(long = "async")]
    async_mode: bool,
    /// Whole workflow deadline, including RPC waits.
    #[arg(long, default_value_t = 300_000, value_parser = clap::value_parser!(u64).range(1..=2_147_483_647))]
    timeout_ms: u64,
}

fn run(options: Options) -> Result<(), String> {
    match options.language.as_str() {
        #[cfg(feature = "lua")]
        "lua" => (),
        #[cfg(feature = "js")]
        "js" => (),
        _ => return Err("language is unavailable in this build (choose lua or js)".into()),
    }
    let source =
        std::fs::read_to_string(&options.script).map_err(|e| format!("read script: {e}"))?;
    let input = due_harness::Options {
        due: options.due,
        cwd: options.cwd,
        evidence: options.evidence,
        source,
        args: options.args,
        timeout: Duration::from_millis(options.timeout_ms),
    };
    let outcome = due_harness::run_with(
        input,
        &options.language,
        options.async_mode,
        |host, source, args, async_mode| match options.language.as_str() {
            #[cfg(feature = "lua")]
            "lua" => lua::run(host, source, args, async_mode).map(|()| serde_json::Value::Null),
            #[cfg(feature = "js")]
            "js" => due_harness::evaluate_js(host, source, args, async_mode),
            _ => unreachable!(),
        },
    )?;
    outcome.error.map_or(Ok(()), Err)
}

fn main() {
    if let Err(error) = run(Options::parse()) {
        eprintln!("{error}");
        std::process::exit(1);
    }
}

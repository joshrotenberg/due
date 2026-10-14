use finocchio_spike::{compiler::Compiler, semantic};
use std::{collections::BTreeMap, env, fs};

#[tokio::main(flavor = "current_thread")]
async fn main() -> Result<(), Box<dyn std::error::Error>> {
    let args: Vec<String> = env::args().skip(1).collect();
    let result = match args.first().map(String::as_str) {
        Some("workflow") => workflow(&args[1..]).await,
        Some("compile" | "provenance") if args.len() == 2 => {
            serde_json::to_value(Compiler::new()?.compile(&fs::read_to_string(&args[1])?, &args[1])?)?
        }
        Some("check" | "definition" | "rename") if args.len() >= 2 => {
            let sources: BTreeMap<String, String> = serde_json::from_str(&fs::read_to_string(&args[1])?)?;
            let operation = match args[0].as_str() {
                "check" if args.len()==2 => serde_json::json!({"op":"check"}),
                "definition" if args.len()==4 => serde_json::json!({"op":"definition","file":args[2],"offset":args[3].parse::<usize>()?}),
                "rename" if args.len()==5 => serde_json::json!({"op":"rename","file":args[2],"offset":args[3].parse::<usize>()?,"name":args[4]}),
                _ => return Err("invalid semantic command arguments".into()),
            };
            semantic::request(&sources, operation)?
        }
        _ => return Err("usage: finocchio-spike workflow {describe|check} CONFIG | workflow run CONFIG INPUT ARTIFACTS | {compile|provenance} FILE | check SOURCES.json | {definition|rename} SOURCES.json MODULE BYTE_OFFSET [NEW_NAME]".into()),
    };
    println!("{}", serde_json::to_string_pretty(&result)?);
    if result.get("supported").and_then(|v| v.as_bool()) == Some(false)
        || result.get("error").is_some_and(|v| !v.is_null())
        || result
            .get("diagnostics")
            .and_then(|v| v.as_array())
            .is_some_and(|v| !v.is_empty())
    {
        std::process::exit(if result["status"] == "cancelled" {
            130
        } else {
            1
        });
    }
    Ok(())
}

#[cfg(unix)]
async fn workflow(args: &[String]) -> serde_json::Value {
    use finocchio_spike::workflow::{self, Cancellation, Config, Mode};
    use std::path::Path;
    let mode = match args.first().map(String::as_str) {
        Some("run") if args.len() == 4 => Mode::Run,
        Some("describe") if args.len() == 2 => Mode::Describe,
        Some("check") if args.len() == 2 => Mode::Check,
        _ => return workflow::error("usage", "workflow {describe|check} CONFIG | workflow run CONFIG INPUT.json FRESH_ARTIFACT_DIRECTORY".into()),
    };
    let config = match Config::load(Path::new(&args[1])) {
        Ok(c) => c,
        Err(e) => return workflow::error("configuration", e),
    };
    let input = if mode == Mode::Run {
        match fs::read(&args[2])
            .map_err(|e| e.to_string())
            .and_then(|bytes| {
                if bytes.len() > 1024 * 1024 {
                    return Err("input exceeds 1 MiB".into());
                }
                serde_json::from_slice(&bytes).map_err(|e| e.to_string())
            }) {
            Ok(input) => input,
            Err(e) => return workflow::error("input", e),
        }
    } else {
        serde_json::Value::Null
    };
    let cancellation = Cancellation::default();
    let mut term = match tokio::signal::unix::signal(tokio::signal::unix::SignalKind::terminate()) {
        Ok(s) => s,
        Err(e) => return workflow::error("signal", e.to_string()),
    };
    let mut int = match tokio::signal::unix::signal(tokio::signal::unix::SignalKind::interrupt()) {
        Ok(s) => s,
        Err(e) => return workflow::error("signal", e.to_string()),
    };
    let run = workflow::execute(
        config,
        mode,
        input,
        args.get(3).map(Path::new),
        cancellation.clone(),
    );
    tokio::pin!(run);
    loop {
        tokio::select! {
            output = &mut run => return output,
            _ = term.recv() => cancellation.cancel(),
            _ = int.recv() => cancellation.cancel(),
        }
    }
}

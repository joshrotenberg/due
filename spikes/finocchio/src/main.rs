use finocchio_spike::{compiler::Compiler, semantic};
use std::{collections::BTreeMap, env, fs};

fn main() -> Result<(), Box<dyn std::error::Error>> {
    let args: Vec<String> = env::args().skip(1).collect();
    let result = match args.first().map(String::as_str) {
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
        _ => return Err("usage: finocchio-spike {compile|provenance} FILE | check SOURCES.json | {definition|rename} SOURCES.json MODULE BYTE_OFFSET [NEW_NAME]".into()),
    };
    println!("{}", serde_json::to_string_pretty(&result)?);
    if result.get("supported").and_then(|v| v.as_bool()) == Some(false)
        || result.get("error").is_some_and(|v| !v.is_null())
        || result
            .get("diagnostics")
            .and_then(|v| v.as_array())
            .is_some_and(|v| !v.is_empty())
    {
        std::process::exit(1);
    }
    Ok(())
}

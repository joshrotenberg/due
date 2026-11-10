use std::{env, path::PathBuf, process::ExitCode};

use clap::Args;

#[derive(Debug, Args)]
pub struct McpArgs {
    /// Working directory for native workers. Defaults to the server working directory.
    #[arg(long, value_name = "DIR")]
    cwd: Option<PathBuf>,
}

pub async fn execute(args: McpArgs) -> ExitCode {
    match run(args).await {
        Ok(()) => ExitCode::SUCCESS,
        Err(error) => {
            eprintln!("due mcp: {error}");
            ExitCode::FAILURE
        }
    }
}

async fn run(args: McpArgs) -> Result<(), Box<dyn std::error::Error + Send + Sync>> {
    let cwd = match args.cwd {
        Some(cwd) => cwd,
        None => env::current_dir()?,
    };
    let mut config = due_mcp::Config::new(cwd);
    config.claude_binary = env::var_os("DUE_MCP_CLAUDE_BINARY").map(Into::into);
    config.codex_binary = env::var_os("DUE_MCP_CODEX_BINARY").map(Into::into);
    due_mcp::serve_stdio(config, crate::shutdown_signal()?).await
}

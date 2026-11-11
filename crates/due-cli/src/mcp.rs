use std::{
    env,
    fs::OpenOptions,
    io::Read,
    path::{Path, PathBuf},
    process::ExitCode,
};

use due_mcp::ProfileCatalog;

use clap::Args;

#[derive(Debug, Args)]
pub struct McpArgs {
    /// Working directory for native workers. Defaults to the server working directory.
    #[arg(long, value_name = "DIR")]
    cwd: Option<PathBuf>,
    /// Require named workers from this version-1 JSON catalog.
    #[arg(long, value_name = "FILE")]
    profiles: Option<PathBuf>,
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
    config.profiles = args.profiles.as_deref().map(read_profiles).transpose()?;
    due_mcp::serve_stdio(config, crate::shutdown_signal()?).await
}

fn read_profiles(path: &Path) -> Result<ProfileCatalog, Box<dyn std::error::Error + Send + Sync>> {
    let mut options = OpenOptions::new();
    options.read(true);
    #[cfg(unix)]
    {
        use std::os::unix::fs::OpenOptionsExt;
        options.custom_flags(libc::O_NONBLOCK);
    }
    let file = options
        .open(path)
        .map_err(|error| format!("cannot open profile catalog: {error}"))?;
    if !file.metadata()?.is_file() {
        return Err("profile catalog must be a regular JSON file".into());
    }
    let mut source = Vec::new();
    file.take(ProfileCatalog::MAX_BYTES as u64 + 1)
        .read_to_end(&mut source)
        .map_err(|error| format!("cannot read profile catalog: {error}"))?;
    Ok(ProfileCatalog::from_json(&source)?)
}

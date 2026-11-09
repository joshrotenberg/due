fn main() -> Result<(), Box<dyn std::error::Error>> {
    due_harness::core::run_cli(
        "usage: due-lua <workflow.lua> [json-args]",
        "DUE_LUA",
        due_lua::run,
    )
}

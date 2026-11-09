fn main() -> Result<(), Box<dyn std::error::Error>> {
    due_harness::core::run_cli(
        "usage: due-js <workflow.js> [json-args]",
        "DUE_JS",
        due_js::run,
    )
}

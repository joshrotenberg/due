// The same review(prefix, profile, task) interface with project-specific prompts.
// Only functions are retained. Put imports inside functions, not at module scope.
fn review(prefix, profile, task) {
    import "patterns/v1" as p;
    due::phase("local-review");
    p::answers(due::parallel(prefix, [
        p::job(prefix + "-assumptions", profile, "Check hidden assumptions: " + task),
        p::job(prefix + "-counterexample", profile, "Find a concrete counterexample: " + task)
    ]))
}

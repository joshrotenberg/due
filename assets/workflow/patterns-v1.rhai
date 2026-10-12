// Functions only: module-level statements are never executed by the host.
fn job(id, profile, prompt) { #{ id: id, profile: profile, prompt: prompt } }
fn answer(step) {
    if !step.ok { throw "prerequisite failed: " + step.id; }
    step.answer
}
fn answers(steps) {
    let values = [];
    for step in steps { values.push(answer(step)); }
    values
}
fn review(prefix, profile, task) {
    due::phase("review");
    let results = due::parallel(prefix, [
        job(prefix + "-correctness", profile, "Review correctness: " + task),
        job(prefix + "-edges", profile, "Review edge cases: " + task)
    ]);
    answers(results)
}

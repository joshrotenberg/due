// Use workflow.json with entry changed to arrival.rhai (max_runs >= 3).
import "patterns/v1" as p;
let a = due::start(p::job("correctness", "reader", "Review correctness: " + input.task));
let b = due::start(p::job("edges", "reader", "Review edge cases: " + input.task));
let first = due::wait_any([a,b]);
let remaining = if first.id == "correctness" { b } else { a };
let prompt = if first.ok { "Examine the first review: " + json::encode(first.answer) }
    else { "The first review failed. Independently inspect the task." };
let replacement = due::start(p::job("replacement", "reader", prompt));
let stopped = due::cancel(remaining);
#{first:first, cancelled:stopped, replacement:due::wait(replacement)}

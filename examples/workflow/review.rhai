import "patterns/v1" as p;
// To substitute the local composition, change this import to "local/review".
import "patterns/v1" as review;

due::phase("prepare");
let draft = p::answer(due::run(p::job("draft", "reader", input.task)));
let findings = review::review("review", "reader", json::encode(draft));

due::phase("synthesis");
let assessment = p::answer(due::run(p::job("synthesis", "reader",
    "Reconcile these reviews, preserving disagreements: " + json::encode(findings))));
if assessment.needs_more_evidence {
    due::phase("follow-up");
    assessment = p::answer(due::run(p::job("follow-up", "reader",
        "Resolve the open questions without editing: " + json::encode(assessment))));
}
assessment

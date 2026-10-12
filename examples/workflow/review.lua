local p = require("patterns/v1")
-- Substitute require("local/review") without changing Rust.
local review = require("patterns/v1")
due.phase("prepare")
local draft = p.answer(due.run(p.job("draft", "reader", input.task)))
local findings = review.review("review", "reader", json.encode(draft))
due.phase("synthesis")
local assessment = p.answer(due.run(p.job("synthesis", "reader",
    "Reconcile these reviews, preserving disagreements: " .. json.encode(findings))))
if assessment.needs_more_evidence then
    due.phase("follow-up")
    assessment = p.answer(due.run(p.job("follow-up", "reader",
        "Resolve the open questions without editing: " .. json.encode(assessment))))
end
return assessment

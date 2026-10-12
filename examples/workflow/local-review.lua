local p = require("patterns/v1")
local function review(prefix, profile, task)
    due.phase("local-review")
    return p.answers(due.parallel(prefix, {
        p.job(prefix .. "-assumptions", profile, "Check hidden assumptions: " .. task),
        p.job(prefix .. "-counterexample", profile, "Find a concrete counterexample: " .. task)
    }))
end
return {review=review}

-- Same returned-function protocol and policy as arrival.fnl.
local p = require("patterns/v1")
return function(input)
    local a = due.start(p.job("correctness", "reader", "Review correctness: " .. input.task))
    local b = due.start(p.job("edges", "reader", "Review edge cases: " .. input.task))
    local first = due.wait_any({a, b})
    local remaining = first.id == "correctness" and b or a
    local prompt = first.ok and ("Examine the first review: " .. json.encode(first.answer))
        or "The first review failed. Independently inspect the task."
    local replacement = due.start(p.job("replacement", "reader", prompt))
    local stopped = due.cancel(remaining)
    return {first=first, cancelled=stopped, replacement=due.wait(replacement)}
end

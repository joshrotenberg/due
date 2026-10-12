local p = {}
function p.job(id, profile, prompt) return {id=id, profile=profile, prompt=prompt} end
function p.answer(step)
    if not step.ok then error("prerequisite failed: " .. step.id) end
    return step.answer
end
function p.answers(steps)
    local values = json.array()
    for _, step in ipairs(steps) do values[#values+1] = p.answer(step) end
    return values
end
function p.review(prefix, profile, task)
    due.phase("review")
    return p.answers(due.parallel(prefix, {
        p.job(prefix .. "-correctness", profile, "Review correctness: " .. task),
        p.job(prefix .. "-edges", profile, "Review edge cases: " .. task)
    }))
end
return p

local p = require("local/pipeline")
return function(input)
    local function work(prefix, profile)
        return p.pipeline({prefix=prefix, task=input.task,
            profiles={design=profile, implement=profile, review=profile}})
    end
    local outcomes = due.concurrent("implementations", {
        function() return work("a", "workspace_a") end,
        function() return work("b", "workspace_b") end
    })
    local findings = json.array()
    for component, pipeline in ipairs(outcomes) do
        for _, step in ipairs(pipeline.steps) do
            if step.ok then
                for _, text in ipairs(step.answer.findings or {}) do
                    findings[#findings+1] = {component=component, text=text}
                end
            end
        end
    end
    findings = p.stable_sort(findings, function(a,b) return a.text < b.text end)
    due.phase("synthesis")
    local synthesis = due.run({id="synthesis", profile="synthesizer",
        prompt="Synthesize these independent outcomes; do not edit: " .. json.encode({outcomes=outcomes, findings=findings})})
    return {outcomes=outcomes, findings=findings, synthesis=synthesis}
end

-- A public function library: no private bindings, mutable module state or scheduler.
local p = {}
function p.stage(config, name, previous)
    return due.run({
        id = config.prefix .. "-" .. name,
        profile = config.profiles[name],
        prompt = name .. ": " .. config.task .. "\nPrior outcomes: " .. json.encode(previous)
    })
end
function p.pipeline(config)
    local steps = json.array()
    for _, name in ipairs({"design", "implement", "review"}) do
        due.phase(name)
        local stage = config.stages and config.stages[name] or p.stage
        local outcome = stage(config, name, steps)
        steps[#steps+1] = outcome
        -- Failed agents are data. The library chooses to stop this pipeline.
        if not outcome.ok then return {ok=false, stopped_at=name, steps=steps} end
    end
    return {ok=true, steps=steps}
end
-- Copying stable merge sort, entirely in Lua: comparator and loops can suspend
-- under the instruction hook. The comparator must define a strict ordering.
function p.stable_sort(items, less)
    local out, scratch = json.array(), {}
    for i, item in ipairs(items) do out[i] = item end
    local width, n = 1, #out
    while width < n do
        for first = 1, n, 2 * width do
            local middle = math.min(first + width, n + 1)
            local last = math.min(first + 2 * width, n + 1)
            local left, right = first, middle
            for target = first, last - 1 do
                if left < middle and (right >= last or not less(out[right], out[left])) then
                    scratch[target], left = out[left], left + 1
                else
                    scratch[target], right = out[right], right + 1
                end
            end
        end
        out, scratch = scratch, out
        width = width * 2
    end
    -- Empty/nonempty array identity, without marking an already marked table.
    local result = json.array()
    for i, item in ipairs(out) do result[i] = item end
    return result
end
return p

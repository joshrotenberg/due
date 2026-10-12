local component = require("local/component") -- component.fnl in the manifest
local patterns = require("patterns/v1")
return function(input)
    local review = component.in_phase("review", function(task)
        return patterns.answer(due.run(patterns.job("review", "reader", task)))
    end)
    return review(input.task)
end

local component = require('local/component')
local function run(id)
    return component.run(id, 'barrier-' .. id)
end
return {run=run}

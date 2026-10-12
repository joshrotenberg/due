-- A normal higher-order component, with no additional host primitive.
local function in_phase(name, operation)
    return function(...)
        due.phase(name)
        return operation(...)
    end
end
return {in_phase=in_phase}

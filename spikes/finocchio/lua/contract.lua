-- A declaration library, not a change to Fennel function/call semantics.
local docs = setmetatable({}, {__mode="k"})
local predicates = {
    number=function(x) return type(x)=="number" end,
    integer=function(x) return math.type(x)=="integer" end,
    string=function(x) return type(x)=="string" end,
    boolean=function(x) return type(x)=="boolean" end,
    ["function"]=function(x) return type(x)=="function" end,
    ["nil"]=function(x) return x==nil end,
    ["any"]=function() return true end,
    ["string?"]=function(x) return x==nil or type(x)=="string" end,
}
local function copy(value)
    if type(value)~="table" then return value end
    local out={} for k,v in pairs(value) do out[k]=copy(v) end return out
end
local function wrap(name, parameters, doc, implementation)
    parameters=copy(parameters)
    for _,p in ipairs(parameters) do assert(predicates[p.type], "unknown contract type: "..p.type) end
    local function checked(...)
        local count=select('#', ...)
        if count~=#parameters then
            error(string.format("contract %s: expected %d arguments, got %d", name, #parameters, count), 2)
        end
        for i,p in ipairs(parameters) do
            local value=select(i, ...)
            if not predicates[p.type](value) then
                error(string.format("contract %s argument %d (%s): expected %s, got %s", name, i, p.name, p.type, type(value)), 2)
            end
        end
        return implementation(...)
    end
    docs[checked]={name=name, parameters=parameters, doc=doc, arity=#parameters,
        semantics="exact positional arity; nil counts; arguments only"}
    return checked
end
local function define(name, parameters, doc, build)
    local implementation
    local checked=wrap(name,parameters,doc,function(...) return implementation(...) end)
    implementation=build(checked)
    return checked
end
return {wrap=wrap, define=define, describe=function(f) return copy(docs[f]) end}

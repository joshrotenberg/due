return function(fennel, source, filename, macros)
    -- Public compilerEnv option: user macros get pure computation and AST APIs,
    -- not runtime host capabilities. The VM itself has no IO/process/network.
    local env = {assert=assert, error=error, ipairs=ipairs, pairs=pairs,
        next=next, select=select, type=type, tostring=tostring, tonumber=tonumber,
        math=math, string=string, table=table, unpack=table.unpack, pcall=pcall,
        getmetatable=getmetatable, setmetatable=setmetatable}
    fennel.path = ""
    fennel["macro-path"] = ""
    -- One explicit, in-memory macro module; no fallback loading.
    local searchers = fennel["macro-searchers"]
    for k in pairs(searchers) do searchers[k] = nil end
    searchers[1] = function(name)
        if name ~= "finocchio" then return nil end
        return function()
            return fennel.eval(macros, {env="_COMPILER", compilerEnv=env,
                filename="finocchio/macros.fnl"})
        end
    end
    local events = {}
    local produced = {} -- Actual AST identity, not inferred matching text.
    local function expanded(ast, result)
        local s = fennel["ast-source"](ast)
        local id = #events+1
        events[id] = {id=id, parent=produced[ast], name=tostring(ast[1]), file=s.filename,
            line=s.line, start=s.bytestart and s.bytestart-1, ["end"]=s.byteend,
            input=fennel.view(ast), output=fennel.view(result)}
        if type(result)=="table" then produced[result]=id end
    end
    local ok, code, map = pcall(fennel.compileString, source, {
        filename=filename, compilerEnv=env, useMetadata=false,
        correlate=false, requireAsInclude=false, ["error-pinpoint"]=false,
        allowedGlobals={"contract", "data", "assert", "error", "pcall", "select",
            "type", "tostring", "tonumber", "pairs", "ipairs", "next", "rawget",
            "rawset", "rawequal", "getmetatable", "setmetatable", "math", "string", "table",
            "require", "_VERSION", "_G"},
        plugins={{name="finocchio-provenance", versions={"1.6.1"}, macroexpand=expanded}}
    })
    local lines = {}
    if ok then
        -- The second return's layout is NOT a stable public source-map contract.
        for i=1, select(2, code:gsub("\n", ""))+1 do
            local entry = map[i]
            if entry and entry[1] and entry[2] then
                lines[#lines+1] = {i, entry[1], entry[2]}
            end
        end
    end
    return {lua=ok and code or nil, error=not ok and tostring(code) or nil,
        expansions=events, pinned_line_map=lines}
end

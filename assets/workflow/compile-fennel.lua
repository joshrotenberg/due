-- Deliberate integration with the pinned 1.6.1 scope/parser APIs. This file is
-- trusted compiler code, never installed in the workflow VM.
return function(fennel, source, filename)
    local scope = fennel.scope()
    local function unsupported(name)
        return function(ast)
            error(filename .. ":" .. tostring(type(ast) == "table" and ast.line or 1)
                  .. ": unsupported Fennel form " .. name .. " (due-functions-v1)", 0)
        end
    end
    -- Override both dispatch tables BEFORE macro expansion. In particular,
    -- import-macros calls _SPECIALS directly, before the compiler call hook.
    for _, name in ipairs({"macro", "macros", "import-macros", "require-macros",
                            "eval-compiler", "include", "lua", "quote", "unquote",
                            "with-open", "assert-repl", "macrodebug", "global"}) do
        scope.macros[name] = false
        scope.specials[name] = unsupported(name)
    end
    -- No ambient macro search path even if a future built-in tries to use one.
    for k in pairs(fennel["macro-searchers"]) do fennel["macro-searchers"][k] = nil end
    fennel.path = ""
    fennel["macro-path"] = ""
    local nodes, events = 0, 0
    local function walk(ast, depth)
        nodes = nodes + 1
        if nodes > 32768 or depth > 64 then
            error(filename .. ":" .. tostring(type(ast) == "table" and ast.line or 1)
                  .. ": Fennel AST node/depth budget exhausted", 0)
        end
        if type(ast) == "table" and not fennel["sym?"](ast) then
            for k, v in pairs(ast) do
                if type(k) == "table" then walk(k, depth + 1) end
                if type(v) == "table" then walk(v, depth + 1) end
            end
        end
    end
    -- Upstream parser, not token/word filtering; strings/comments containing
    -- excluded names are ordinary data. Parsing itself is also process bounded.
    for _, ast in fennel.parser(source, filename) do walk(ast, 0) end
    local function charge(ast)
        events = events + 1
        if events > 4096 then
            error(filename .. ":" .. tostring(type(ast) == "table" and ast.line or 1)
                  .. ": Fennel expansion budget exhausted", 0)
        end
    end
    return fennel.compileString(source, {
        filename=filename, scope=scope, compilerEnv={}, correlate=false,
        useMetadata=false, requireAsInclude=false, assertAsRepl=false,
        toBeClosed=false, ["error-pinpoint"]=false,
        allowedGlobals={"due", "json", "require", "assert", "error", "ipairs",
                        "pairs", "next", "pcall", "select", "tonumber", "tostring",
                        "type", "rawequal", "rawget", "rawset", "rawlen",
                        "math", "string", "table", "utf8", "getmetatable", "_VERSION", "_G"},
        plugins={{name="due-functions-v1", versions={"1.6.1"}, call=charge,
                  ["symbol-to-expression"]=charge, destructure=charge}}
    })
end

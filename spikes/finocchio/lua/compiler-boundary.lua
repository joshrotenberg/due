-- Observation only: version-pinned unstable plugin callbacks and AST APIs.
-- Never inspect scope tables, rewrite AST metadata or consume the line map.
return function(f, driver, source, filename, macros, experimental_macros, array_mt)
    local function array() return setmetatable({}, array_mt) end
    local events, diagnostics = array(), array()
    local ids, appearances, next_id = {}, {}, 0
    local function identity(node)
        if type(node) ~= "table" then return nil end
        if not ids[node] then next_id=next_id+1; ids[node]=next_id end
        return ids[node]
    end
    local function location(node)
        local s = f["ast-source"](node)
        -- ast-source can return the AST. The extra propagation marker is an
        -- explicitly pinned internal hint, not proof of a parser/caller origin.
        return {file=s.filename, line=s.line, start=s.bytestart and s.bytestart-1,
            finish=s.byteend, origin="public-ast-source", confidence="reported",
            pinned_from_macro=s["from-macro?"]}
    end
    local function walk(node, visit, seen)
        if type(node) ~= "table" or seen[node] then return end
        seen[node]=true
        visit(node)
        if f["sym?"](node) or f["varg?"](node) then return end
        if f["list?"](node) or f["sequence?"](node) then
            for k,v in pairs(node) do
                if type(k)=="number" then walk(v,visit,seen) end
            end
        elseif f["table?"](node) then
            for k,v in pairs(node) do walk(k,visit,seen); walk(v,visit,seen) end
        end
    end
    local function links(node)
        local out=array()
        for _,a in ipairs(appearances[node] or {}) do
            out[#out+1]={expansion=a.expansion, role=a.role, origin=a.origin,
                confidence="exact-identity"}
        end
        return out
    end
    local plugin = {name="finocchio-boundary-v0", versions={"1.6.1"}}
    plugin.macroexpand = function(ast, result)
        -- This is a post-expansion observation. A macro may already have
        -- mutated its input; retained-input does not certify parser provenance.
        local input_nodes={}
        walk(ast,function(node) input_nodes[node]=true end,{})
        local event={id=#events+1, name=tostring(ast[1]), input_node=identity(ast),
            location=location(ast), links=links(ast), output_nodes=array()}
        events[#events+1]=event
        walk(result,function(node)
            local origin = input_nodes[node] and "retained-input"
                or appearances[node] and "previous-output" or "first-observed-output"
            local role = node==result and "root" or "descendant"
            event.output_nodes[#event.output_nodes+1]={node=identity(node),
                location=location(node), origin=origin, role=role}
            appearances[node]=appearances[node] or {}
            table.insert(appearances[node],{expansion=event.id,role=role,origin=origin})
        end,{})
        -- nil: do not intercept the compiler's remaining plugins or behavior.
    end
    plugin["assert-compile"] = function(_, message, ast)
        local related=links(ast)
        diagnostics[#diagnostics+1]={phase="compile", message=tostring(message),
            node=identity(ast), location=location(ast), related=related,
            attribution=#related>0 and "related" or "unknown",
            origin="public-assert-compile"}
        -- No truthy return, reset or throw: preserve the original failure.
    end
    local compilation=driver(f,source,filename,macros,
        {macros=experimental_macros,plugin=plugin})
    return {version=0,compilation=compilation,events=events,diagnostics=diagnostics}
end

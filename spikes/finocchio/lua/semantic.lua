-- Public parser + independent scope IR. Only shipped macros have source rules.
-- Generated temporaries are deliberately absent from source edit spans.
return function(f, sources, request, array_mt)
    local function array() return setmetatable({}, array_mt) end
    if request.op~="check" and request.op~="definition" and request.op~="rename" then
        return {supported=false,error="unknown operation"}
    end
    if request.op~="check" and (type(request.file)~="string" or
        math.type(request.offset)~="integer" or request.offset<0) then
        return {supported=false,error="expected module and nonnegative byte offset"}
    end
    local function unsupported(message) error("unsupported: " .. message, 0) end
    local reserved = {}
    for _,name in ipairs({"fn", "local", "let", "do", "if", "import-macros", "require",
        "defn", "twice", "nil", "true", "false", "var", "set", "macro", "macros",
        "each", "for", "match", "quote", "unquote", "values", "global",
        "contract"}) do reserved[name]=true end
    for name, metadata in pairs(f.syntax()) do
        if metadata["special?"] or metadata["macro?"] then reserved[name]=true end
    end
    local contract_types={number=true,integer=true,string=true,boolean=true,
        ["function"]=true,["nil"]=true,any=true,["string?"]=true}
    local builtins = {}
    for _,name in ipairs({"+", "-", "*", "/", "=", "<", ">", "<=", ">=", "not=", ".."}) do builtins[name]=true end
    local function symbol(x) return f["sym?"](x) and tostring(x) or nil end
    local function list(x) return f["list?"](x) end
    local function seq(x) return f["sequence?"](x) end
    local function ismap(x) return type(x)=="table" and not symbol(x) and not list(x) and not seq(x) end
    local function simple(name)
        if not name or reserved[name] or builtins[name] or name:find("[%.:&]") then return false end
        local ok, forms = pcall(function()
            local out={} for _,v in f.parser(name, "rename") do out[#out+1]=v end return out
        end)
        return ok and #forms==1 and symbol(forms[1])==name and #name>0
    end
    local function analyze(input)
        local definitions, references, diagnostics, modules, loading = array(), array(), array(), {}, {}
        local function scope(parent) return {parent=parent, bindings={}} end
        local function resolve(s, name)
            while s do if s.bindings[name] then return s.bindings[name] end s=s.parent end
        end
        local function span(file, node, text, offset)
            local loc=f["ast-source"](node)
            if not loc.bytestart then unsupported("no source span for " .. text) end
            local start=loc.bytestart-1+(offset or 0)
            if input[file]:sub(start+1,start+#text)~=text then unsupported("non-source/generated symbol "..text) end
            return {file=file, start=start, finish=start+#text}
        end
        local function bind(s, file, node, value)
            local name=symbol(node)
            if not simple(name) then unsupported("binding must be a simple non-reserved symbol") end
            if s.bindings[name] then unsupported("duplicate binding "..name) end
            local loc=span(file,node,name)
            local id=#definitions+1
            loc.id=id; loc.name=name; loc.value=value or {}
            definitions[id]=loc; s.bindings[name]=id
            return id
        end
        local function reference(file, node, name, id, offset, editable)
            local r=span(file,node,name,offset)
            r.binding=id; r.editable=editable~=false
            references[#references+1]=r
            return definitions[id].value
        end
        local function use(s,file,node)
            local name=symbol(node)
            if name=="nil" then return {literal="nil"} end
            local root,field=name:match("^([^%.:]+)%.([^%.:]+)$")
            if root then
                local id=resolve(s,root)
                if not id then unsupported("unresolved module alias "..root) end
                local alias=reference(file,node,root,id)
                if not alias.exports or not alias.exports[field] then unsupported("unknown module member "..name) end
                return reference(file,node,field,alias.exports[field],#root+1,false), alias.exports[field]
            end
            local id=resolve(s,name)
            if not id then unsupported("unresolved symbol/form "..name) end
            return reference(file,node,name,id), id
        end
        local expr, module
        local function body(forms, first, s, file, macros)
            local value={}
            for i=first,#forms do value=expr(forms[i],s,file,macros,true) end
            return value
        end
        local function function_form(node,s,file,macros,contracted)
            local named=symbol(node[2])
            local index=named and 3 or 2
            if contracted and not named then unsupported("defn requires name") end
            local params=node[index]
            if not seq(params) then unsupported("only simple parameter vectors") end
            local step=contracted and 2 or 1
            if #params%step~=0 then unsupported("contract parameter pairs") end
            local signature={arity=math.floor(#params/step), types=array(), exact=contracted}
            if named then bind(s,file,node[2],signature) end
            local inner=scope(s)
            for i=1,#params,step do
                bind(inner,file,params[i],{})
                if contracted then
                    if type(params[i+1])~="string" or not contract_types[params[i+1]] then unsupported("known literal contract types required") end
                    signature.types[#signature.types+1]=params[i+1]
                end
            end
            if contracted and type(node[index+1])~="string" then unsupported("literal documentation required") end
            local first=index+(contracted and 2 or 1)
            if not node[first] then unsupported("function body required") end
            body(node,first,inner,file,macros)
            return signature
        end
        expr=function(node,s,file,macros,statement)
            if symbol(node) then return use(s,file,node) end
            if type(node)~="table" then
                return {literal=type(node), integer=type(node)=="number" and math.type(node)=="integer" or nil}
            end
            if seq(node) then for _,v in ipairs(node) do expr(v,s,file,macros) end return {} end
            if ismap(node) then
                -- The public parser normalizes {: x} to {:x x}, losing the
                -- distinction even in its private keys metadata. Reparse the
                -- original contents as a sequence to retain the ':' token.
                -- This uses public APIs, handles comments/whitespace, and never
                -- invents a source edit from normalized map keys.
                local loc=f["ast-source"](node)
                if not loc.bytestart or not loc.byteend then unsupported("map without source span") end
                local contents=input[file]:sub(loc.bytestart+1,loc.byteend-1)
                for _,entries in f.parser("["..contents.."]",file) do
                    for i=1,#entries,2 do
                        if symbol(entries[i])==":" then unsupported("shorthand map/export keys; use explicit string keys") end
                    end
                end
                local exports={}
                local keys={} for k in pairs(node) do
                    if type(k)~="string" then unsupported("only explicit string map/export keys") end
                    keys[#keys+1]=k
                end
                table.sort(keys)
                for _,k in ipairs(keys) do
                    local _,id=expr(node[k],s,file,macros)
                    if id then exports[k]=id end
                end
                return {exports=exports}
            end
            if not list(node) then unsupported("AST node kind") end
            local head=symbol(node[1])
            if (head=="local" or head=="defn" or head=="import-macros" or
                (head=="fn" and symbol(node[2]))) and not statement then
                unsupported("declaration in expression position")
            end
            if head=="import-macros" then
                if s.parent then unsupported("nested macro import") end
                if #node~=3 or node[3]~="finocchio" or not ismap(node[2]) then unsupported("only explicit finocchio macro imports") end
                for k,v in pairs(node[2]) do
                    if (k~="defn" and k~="twice") or symbol(v)~=k then unsupported("macro aliases or unknown macros") end
                    macros[k]=true
                end
                return {}
            elseif head=="fn" then return function_form(node,s,file,macros,false)
            elseif head=="defn" then
                if not macros.defn then unsupported("defn not imported") end
                if #node~=5 then unsupported("defn requires one body expression") end
                return function_form(node,s,file,macros,true)
            elseif head=="local" then
                if #node~=3 then unsupported("local shape") end
                local value=expr(node[3],s,file,macros)
                bind(s,file,node[2],value)
                return {}
            elseif head=="let" then
                if not seq(node[2]) or #node[2]%2~=0 then unsupported("simple let pairs only") end
                local inner=scope(s)
                for i=1,#node[2],2 do
                    local value=expr(node[2][i+1],inner,file,macros)
                    bind(inner,file,node[2][i],value)
                end
                return body(node,3,inner,file,macros)
            elseif head=="do" then return body(node,2,scope(s),file,macros)
            elseif head=="if" then
                if #node~=4 then unsupported("only three-argument if") end
                for i=2,4 do expr(node[i],s,file,macros) end return {}
            elseif head=="twice" then
                if not macros.twice or #node~=2 then unsupported("twice shape/import") end
                expr(node[2],s,file,macros)
                return {}
            elseif head=="require" then
                if #node~=2 or type(node[2])~="string" then unsupported("static require only") end
                return module(node[2])
            elseif builtins[head] then
                for i=2,#node do expr(node[i],s,file,macros) end return {}
            elseif not head or reserved[head] then unsupported("form "..tostring(head))
            else
                local value=use(s,file,node[1])
                if not value.arity then unsupported("call of unknown function shape "..head) end
                local loc=f["ast-source"](node)
                if value.exact and #node-1~=value.arity then
                    diagnostics[#diagnostics+1]={file=file,line=loc.line,start=loc.bytestart-1,finish=loc.byteend,
                        message=head..": expected "..value.arity.." arguments, got "..(#node-1)}
                end
                for i=2,#node do
                    local arg=expr(node[i],s,file,macros)
                    local expected=value.types and value.types[i-1]
                    if expected and arg.literal and expected~="any" then
                        local actual=arg.literal
                        local valid=actual==expected or (expected=="integer" and actual=="number" and arg.integer)
                            or (expected=="string?" and (actual=="nil" or actual=="string"))
                        if not valid then diagnostics[#diagnostics+1]={file=file,line=loc.line,start=loc.bytestart-1,finish=loc.byteend,
                            message=head.." argument "..(i-1)..": expected "..expected..", got "..actual} end
                    end
                end
                return {}
            end
        end
        module=function(file)
            if modules[file] then return modules[file] end
            if loading[file] then unsupported("module cycle "..file) end
            if not input[file] then unsupported("missing module "..file) end
            loading[file]=true
            local forms={} for _,node in f.parser(input[file],file) do forms[#forms+1]=node end
            local value=body(forms,1,scope(nil),file,{})
            modules[file]=value; loading[file]=nil
            return value
        end
        local names={} for name in pairs(input) do names[#names+1]=name end table.sort(names)
        for _,name in ipairs(names) do module(name) end
        return {supported=true, definitions=definitions, references=references, diagnostics=diagnostics}
    end
    local ok,result=pcall(analyze,sources)
    if not ok then return {supported=false,error=tostring(result)} end
    -- Keep inference state private; all operations expose the same binding shape.
    for _,d in ipairs(result.definitions) do d.value=nil end
    if request.op=="check" then return result end
    local selected
    for _,d in ipairs(result.definitions) do
        if d.file==request.file and request.offset>=d.start and request.offset<d.finish then selected=d.id end
    end
    for _,r in ipairs(result.references) do
        if r.file==request.file and request.offset>=r.start and request.offset<r.finish then selected=r.binding end
    end
    if not selected then return {supported=false,error="no source binding at offset"} end
    if request.op=="definition" then
        local refs=array() for _,r in ipairs(result.references) do if r.binding==selected then refs[#refs+1]=r end end
        return {supported=true,definition=result.definitions[selected],references=refs}
    end
    if request.op~="rename" then return {supported=false,error="unknown operation"} end
    if not simple(request.name) then return {supported=false,error="invalid or reserved replacement symbol"} end
    local edits={result.definitions[selected]}
    for _,r in ipairs(result.references) do if r.binding==selected and r.editable then edits[#edits+1]=r end end
    table.sort(edits,function(a,b) if a.file==b.file then return a.start>b.start else return a.file<b.file end end)
    local changed={} for k,v in pairs(sources) do changed[k]=v end
    for _,edit in ipairs(edits) do
        local source=changed[edit.file]
        changed[edit.file]=source:sub(1,edit.start)..request.name..source:sub(edit.finish+1)
    end
    local valid,after=pcall(analyze,changed)
    if not valid then return {supported=false,error="rename rejected: "..tostring(after)} end
    if #after.definitions~=#result.definitions or #after.references~=#result.references then
        return {supported=false,error="rename changed binding structure"}
    end
    for i,r in ipairs(result.references) do
        if r.binding~=after.references[i].binding then return {supported=false,error="rename would capture or rebind a reference"} end
    end
    return {supported=true,sources=changed,edits=edits,diagnostics=after.diagnostics}
end

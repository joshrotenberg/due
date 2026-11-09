local bridge = ...
local co = coroutine
local codec = json
local pending, tasks = {}, {}
local task_count = 0
local body, result, failure
local errors = { __tostring = function(failure) return failure.message end }

local function advance(thread, ...)
  local ok, value = co.resume(thread, ...)
  if not ok then
    failure = failure or tostring(value)
    error(failure, 0)
  end
  if co.status(thread) == "dead" then
    local complete = tasks[thread]
    tasks[thread] = nil
    task_count = task_count - 1
    complete(value)
  end
end

local function schedule(body, complete)
  local thread = co.create(body)
  tasks[thread] = complete
  task_count = task_count + 1
  return thread
end

local function caller()
  local thread = co.running()
  if not tasks[thread] then error("operation requires an active workflow task", 3) end
  return thread
end

local function submit(op, params)
  local thread = caller()
  local id = bridge.start(codec.encode({ op = op, params = params }))
  pending[id] = thread
  local reply = co.yield()
  if reply.Err then error(setmetatable(reply.Err, errors), 0) end
  return reply.Ok
end

local scope = {}
function scope.spawn(provider, prompt, timeout_ms)
  return submit("spawn", { provider = provider, prompt = prompt, timeout_ms = timeout_ms })
end
function scope.followUp(session, prompt, timeout_ms)
  return submit("follow_up", { session = session, prompt = prompt, timeout_ms = timeout_ms })
end
function scope.inspect(run) return submit("inspect", { run = run }) end
function scope.wait(runs, timeout_ms)
  local sequence = codec.array()
  for key, run in next, runs do sequence[key] = run end
  if timeout_ms == nil then timeout_ms = 1000 end
  return submit("wait", { runs = sequence, timeout_ms = timeout_ms })
end
function scope.interrupt(run) return submit("interrupt", { run = run }) end
function scope.respond(interaction, response)
  return submit("respond", { interaction = interaction, response = response })
end
function scope.readItems(session, after, limit)
  if after == nil then after = 0 end
  if limit == nil then limit = 32 end
  return submit("read_items", { session = session, after = after, limit = limit })
end
function scope.parallel(...)
  local parent = caller()
  local branches = table.pack(...)
  if task_count + branches.n > 17 then error("too many active Lua tasks", 2) end
  for index = 1, branches.n do
    if type(branches[index]) ~= "function" then error("parallel requires functions", 2) end
  end
  local values = codec.array()
  local remaining = branches.n
  local threads = {}
  for index = 1, branches.n do
    threads[index] = schedule(branches[index], function(value)
      if value == nil then value = codec.null end
      values[index] = value
      remaining = remaining - 1
      if remaining == 0 and co.status(parent) == "suspended" then advance(parent) end
    end)
  end
  -- Reserve each sibling's slot before a branch can schedule its own children.
  for _, thread in ipairs(threads) do advance(thread) end
  if remaining > 0 then co.yield() end
  return values
end

coroutine = nil
Due = {
  args = codec.decode(bridge.args),
  log = bridge.log,
  run = function(workflow)
    if body then error("Due.run may be called once", 2) end
    if type(workflow) ~= "function" then error("Due.run requires a function", 2) end
    body = workflow
  end,
}

return function()
  if not body then error("workflow must call Due.run", 0) end
  if task_count == 0 and result == nil then
    advance(schedule(function() return body(scope) end, function(value)
      if value == nil then value = codec.null end
      result = codec.encode(value)
    end))
  end
  for _ = 1, 16 do
    if failure then error(failure, 0) end
    if result ~= nil then return result end
    local raw = bridge.poll()
    if not raw then break end
    local completion = codec.decode(raw)
    local thread = pending[completion.id]
    if not thread then error("unknown core completion", 0) end
    pending[completion.id] = nil
    advance(thread, completion.result)
  end
end

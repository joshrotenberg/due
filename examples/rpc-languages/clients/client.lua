-- Synchronous cooperative client. No replay: gaps/omissions fail closed.
-- Validated old cursors are ignored. Raw-byte fidelity belongs to the host.
DueClient = {}
DueClient.__index = DueClient
local function object(v) return type(v) == 'table' and v ~= json.null end
local function str(v) return type(v) == 'string' and #v > 0 end
local function integer(v) return type(v) == 'number' and v == v and v >= 0 and v <= 9007199254740991 and v % 1 == 0 end
local function count(t) local n = 0; for _ in pairs(t) do n = n + 1 end; return n end
-- Copy only callback-sensitive lifecycle payloads, never re-encode received
-- numbers. Preserve the host's array/object metatables and JSON null identity.
local function copy(value, seen)
  if not object(value) then return value end
  seen = seen or {}
  if seen[value] then return seen[value] end
  local result = {}; seen[value] = result
  for k, v in pairs(value) do result[k] = copy(v, seen) end
  local mt = getmetatable(value)
  if type(mt) == 'table' then setmetatable(result, mt) end
  return result
end
function DueClient.new(options)
  options = options or {}
  return setmetatable({onItem=options.onItem, onInteraction=options.onInteraction,
    pending={}, finished={}, interactions={}, seq=0, cursor=0}, DueClient)
end
function DueClient:_fail(e)
  if self.failure == nil then self.failure = e; self.interactions = {} end
  error(self.failure, 0)
end
function DueClient:_check(blocking)
  if self.failure ~= nil then error(self.failure, 0) end
  if self.closed then error('Client closed', 0) end
  if blocking and self.callback then self:_fail('Reentrant blocking operation') end
end
function DueClient:_guard(fn)
  local ok, value = pcall(fn)
  if not ok then self:_fail(value) end
  return value
end
function DueClient:_callback(fn, item)
  if fn == nil then return nil end
  if type(fn) ~= 'function' then self:_fail('Callback must be a function') end
  -- A private coroutine is used solely to detect yield, never as a scheduler.
  self.callback = true
  local co = coroutine.create(fn)
  local ok, value = coroutine.resume(co, item)
  self.callback = false
  if not ok then self:_fail(value) end
  if coroutine.status(co) ~= 'dead' or type(value) == 'thread' or type(value) == 'function' then
    self:_fail('Callbacks must complete synchronously')
  end
  self:_check(false)
  return value
end
function DueClient:request(method, params)
  self:_check(false)
  if self.closing then error('Client closing', 0) end
  return self:_guard(function()
    assert(str(method) and object(params), 'Invalid request')
    assert(count(self.pending) < 64, 'Pending request limit')
    assert(self.seq < 9007199254740991, 'Request ID limit')
    self.seq = self.seq + 1
    local id = tostring(self.seq)
    self.pending[id] = {}
    bridge.send(json.encode(json.object({jsonrpc='2.0', id=id, method=method, params=params})))
    return id
  end)
end
function DueClient:_key(turn, request) return json.encode(json.array({turn, request})) end
function DueClient:_item(item)
  assert(object(item) and str(item.session_id) and str(item.kind) and object(item.data), 'Malformed item')
  assert(integer(item.cursor) and item.cursor > 0, 'Invalid cursor')
  assert(item.turn_id == json.null or str(item.turn_id), 'Invalid turn identity')
  assert(item.push_dropped == nil or (integer(item.push_dropped) and item.push_dropped == 0), 'Dropped items')
  assert(not self.session or self.session == item.session_id, 'Session identity changed')
  self.session = item.session_id
  if item.cursor <= self.cursor then return end
  assert(item.cursor == self.cursor + 1, 'Cursor gap; replay is not implemented')
  self.cursor = item.cursor
  -- Preserve lifecycle fields and terminal data before exposing the original payload.
  local kind, turn = item.kind, item.turn_id
  local saved
  if kind == 'interaction' or kind == 'interaction_resolved' or kind == 'turn_finished' then
    saved = copy(item)
  end
  local key
  if kind == 'interaction' or kind == 'interaction_resolved' then
    assert(str(turn) and str(saved.data.request_id), 'Invalid interaction identity')
    key = self:_key(turn, saved.data.request_id)
    if kind == 'interaction' and not self.closing then
      assert(not self.interactions[key], 'Duplicate interaction')
      assert(count(self.interactions) < 64, 'Interaction limit')
      self.interactions[key] = {turn=turn, request=saved.data.request_id}
    else self.interactions[key] = nil end
  end
  if kind == 'turn_finished' then
    assert(str(turn) and saved.data.run_id == turn and
      (saved.data.status == 'succeeded' or saved.data.status == 'failed' or saved.data.status == 'cancelled' or saved.data.status == 'timed_out' or saved.data.status == 'planned'), 'Invalid turn result')
    assert(not self.finished[turn] and count(self.finished) < 64, 'Finished turn cache limit or duplicate terminal')
    self.finished[turn] = saved.data
    for k, v in pairs(self.interactions) do if v.turn == turn then self.interactions[k] = nil end end
  end
  if kind == 'session_failed' or kind == 'omitted' then
    self.interactions = {}; self.closing = true
    -- Preserve the originating failure before callbacks can throw or reply.
    self.failure = self.failure or (kind == 'session_failed' and
      ('Due session failed: ' .. (type(item.data.message) == 'string' and item.data.message or 'no message')) or
      'Native item omitted')
  end
  self:_callback(self.onItem, item)
  assert(kind ~= 'session_failed', 'Due session failed')
  assert(kind ~= 'omitted', 'Native item omitted')
  if kind == 'interaction' and not self.closing and not self.interactions[key].answered then
    assert(self.onInteraction, 'Interaction requires explicit handler')
    local response = self:_callback(self.onInteraction, saved)
    local entry = self.interactions[key]
    if response ~= nil and response ~= json.null then
      assert(entry and not entry.answered, 'Interaction already answered')
      -- Route from private fields even if the handler mutated its item.
      local id = self:reply({session_id=self.session, turn_id=entry.turn, data={request_id=entry.request}}, response)
      self.pending[id].automatic = true
    end
  end
end
function DueClient:poll(timeout)
  self:_check(true)
  return self:_guard(function()
    timeout = timeout or 0
    assert(integer(timeout), 'Invalid timeout')
    local raw = bridge.recv(math.min(timeout, 10000))
    if raw == nil then return false end
    local msg = json.decode(raw)
    assert(object(msg) and msg.jsonrpc == '2.0', 'Invalid JSON-RPC envelope')
    if msg.method ~= nil then
      assert(msg.method == 'session.item' and msg.id == nil and msg.result == nil and msg.error == nil, 'Invalid notification')
      self:_item(msg.params)
    else
      assert(str(msg.id) and msg.params == nil and ((msg.result ~= nil) ~= (msg.error ~= nil)), 'Invalid response')
      local p = self.pending[msg.id]
      assert(p and not p.done, 'Unknown or duplicate response ID')
      if msg.error ~= nil then
        assert(object(msg.error) and type(msg.error.code) == 'number' and msg.error.code % 1 == 0 and type(msg.error.message) == 'string', 'Invalid RPC error')
      end
      p.done, p.result, p.error = true, msg.result, msg.error
      if p.automatic then
        self.pending[msg.id] = nil
        if p.error then error(p.error, 0) end
      end
    end
    return true
  end)
end
function DueClient:_deadline(timeout, default)
  timeout = timeout or default
  if not integer(timeout) then self:_fail('Invalid timeout') end
  return self:_guard(function() return bridge.now_ms() + timeout end)
end
function DueClient:_pump(deadline)
  local remaining = self:_guard(function() return deadline - bridge.now_ms() end)
  if remaining <= 0 then error('Due RPC timeout', 0) end
  self:poll(math.min(remaining, 10000))
end
function DueClient:await(id, timeout)
  self:_check(true)
  local p = self.pending[id]
  if not p or p.automatic then error('Unknown request ID', 0) end
  local deadline = self:_deadline(timeout, 30000)
  while not p.done do self:_pump(deadline) end
  self.pending[id] = nil
  if p.error then error(p.error, 0) end -- Ordinary RPC errors are recoverable.
  return p.result
end
function DueClient:open(profile)
  self:_check(true)
  if self.opened or self.openAttempted then error('Session already opened', 0) end
  self.openAttempted = true
  local params = self:_guard(function()
    assert(object(profile), 'Invalid profile')
    local p = json.object({}); for k,v in pairs(profile) do p[k]=v end; p.stream=true; return p
  end)
  local r = self:await(self:request('session.open', params))
  self:_guard(function()
    assert(object(r) and r.protocol_version == 1 and object(r.stream) and r.stream.enabled == true and r.stream.method == 'session.item' and str(r.session_id), 'Unsupported session protocol')
    assert(not self.session or self.session == r.session_id, 'Session identity changed')
    self.session, self.opened = r.session_id, true
  end)
  return r
end
function DueClient:_params(extra)
  assert(self.opened, 'Session not open'); extra.session_id=self.session; return json.object(extra)
end
function DueClient:start(prompt)
  self:_check(true)
  local r = self:await(self:request('turn.start', self:_params({prompt=prompt})))
  self:_guard(function() assert(object(r) and str(r.turn_id), 'Invalid start result') end)
  return r.turn_id
end
function DueClient:wait(turn, timeout)
  self:_check(true)
  if not str(turn) then error('Invalid turn ID', 0) end
  local deadline = self:_deadline(timeout, 300000)
  while not self.finished[turn] do self:_pump(deadline) end
  local r = self.finished[turn]; self.finished[turn] = nil; return r
end
function DueClient:turn(prompt, timeout) self:_check(true); return self:wait(self:start(prompt), timeout) end
function DueClient:interrupt(turn) self:_check(false); return self:request('turn.interrupt', self:_params({turn_id=turn})) end
function DueClient:reply(item, response)
  self:_check(false)
  if self.closing then error('Client closing', 0) end
  assert(object(item) and object(item.data) and item.session_id == self.session, 'Invalid interaction')
  local entry = self.interactions[self:_key(item.turn_id, item.data.request_id)]
  assert(entry and not entry.answered, 'Interaction retired or already answered')
  assert(response ~= nil and response ~= json.null, 'Explicit response required')
  entry.answered = true
  return self:request('interaction.reply', json.object({session_id=self.session, turn_id=entry.turn, request_id=entry.request, response=response}))
end
function DueClient:close()
  self:_check(true)
  local id = self:request('session.close', self:_params({}))
  self.closing = true; self.interactions = {}
  local r = self:await(id)
  self.closed = true; self.pending = {}; self.finished = {}
  return r
end

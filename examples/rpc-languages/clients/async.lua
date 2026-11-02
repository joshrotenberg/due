-- Managed coroutine adapter over the same validated RPC client and Rust bridge.
-- Only the workflow and permission callbacks create tasks; observers stay sync.
local Client = DueClient
local create, resume, status, yield = coroutine.create, coroutine.resume, coroutine.status, coroutine.yield
local signals = setmetatable({}, {__mode='k'})
local token = {}
local scheduler
local function object(v) return type(v)=='table' and v~=json.null end
local function count(t) local n=0; for _ in pairs(t) do n=n+1 end; return n end
local function integer(v, max) return type(v)=='number' and v==v and v>=0 and v<=max and v%1==0 end
local function cancellation(message)
  return setmetatable({kind='cancelled',message=message}, {__tostring=function(v) return v.message end})
end
local function reason(signal)
  local state=signals[signal]
  assert(state, 'Invalid supervision signal')
  if state.aborted then return state.reason end
  if state.parent then return reason(state.parent) end
end
local function checkSignal(signal)
  local failure=reason(signal)
  if failure~=nil then error(failure,0) end
end
local function newSignal(parent)
  local value={}
  signals[value]={parent=parent}
  return setmetatable(value, {
    __index=function(self,key)
      if key=='aborted' then return reason(self)~=nil end
      if key=='reason' then return reason(self) end
      if key=='throwIfAborted' then return checkSignal end
    end,
    __newindex=function() error('Supervision signals are read-only',0) end,
  })
end
local function abort(signal, failure)
  local state=signals[signal]
  if not state.aborted then
    state.aborted=true
    state.reason=failure==nil and cancellation('Scope cancelled') or failure
  end
end
local function await(check)
  assert(scheduler and scheduler.current, 'Operation requires a managed workflow or permission callback')
  local ok,value=yield({marker=token,check=check})
  if not ok then error(value,0) end
  return value
end
local function fail(failure)
  if failure==nil then failure='Lua workflow failed without an error value' end
  if scheduler.failure==nil then scheduler.failure=failure end
  abort(scheduler.scope.signal,scheduler.failure)
  for _,entry in pairs(scheduler.decisions) do abort(entry.signal,scheduler.failure) end
end
local function spawn(fn,kind,entry)
  assert(count(scheduler.tasks)<64,'Managed task limit exceeded')
  scheduler.sequence=scheduler.sequence+1
  local task={co=create(fn),kind=kind,entry=entry,id=scheduler.sequence}
  scheduler.tasks[task.id]=task
  return task
end
local function managed(signal)
  assert(scheduler and scheduler.current,'Operation requires a managed task')
  if scheduler.phase~='running' then error('Supervision scope is closing',0) end
  checkSignal(scheduler.scope.signal)
  signal=signal or scheduler.scope.signal
  checkSignal(signal)
  return signal
end

local Worker={}
Worker.__index=Worker
local workers=setmetatable({}, {__mode='k'})
local function state(worker)
  local value=workers[worker]
  assert(value,'Unknown worker')
  if value.closed then error('Worker is closed',0) end
  return value
end
local function request(worker,method,params,signal,cleanup)
  local s=state(worker)
  if not cleanup then managed(signal) end
  local id=s.client:request(method,params)
  local deadline=bridge.now_ms()+30000
  return await(function()
    if not cleanup and signal then checkSignal(signal) end
    local ready,value=s.client:tryAwait(id)
    if ready then return true,true,value end
    if bridge.now_ms()>=deadline then
      local failure='RPC '..method..' acknowledgement timed out'
      fail(failure); error(failure,0)
    end
    return false
  end)
end
local function params(worker,extra)
  return state(worker).client:_params(extra)
end
local function retire(entry,message)
  abort(entry.signal,cancellation(message))
end
local function retired(entry)
  return reason(entry.signal)~=nil or scheduler.phase~='running'
end
local function observe(s,item)
  -- Read routing identity before the user's observer can mutate its payload.
  local kind,turn=item.kind,item.turn_id
  if kind=='interaction_resolved' then
    local key=s.client:_key(turn,item.data.request_id)
    local entry=scheduler.decisions[key]
    if entry then retire(entry,'Interaction resolved') end
  elseif kind=='turn_finished' then
    for _,entry in pairs(scheduler.decisions) do
      if entry.turn==turn then retire(entry,'Turn finished') end
    end
    if s.active and (s.active.id==turn or s.active.id==nil) then s.active.finished=true end
  elseif kind=='session_failed' or kind=='omitted' then
    fail(s.client.failure or 'Native session failed')
  end
  if s.options.onItem then
    local returned=s.options.onItem(item)
    assert(type(returned)~='thread' and type(returned)~='function','Observers must complete synchronously')
  end
end
local function decision(s,item)
  if scheduler.phase~='running' or scheduler.failure~=nil then return nil end
  assert(type(s.options.onInteraction)=='function','Interaction requires explicit handler')
  local turn,requestId,session=item.turn_id,item.data.request_id,item.session_id
  local key=s.client:_key(turn,requestId)
  local entry={turn=turn,request=requestId,session=session,signal=newSignal(scheduler.scope.signal)}
  scheduler.decisions[key]=entry
  spawn(function()
    checkSignal(entry.signal)
    local response=s.options.onInteraction(item,entry.signal)
    if retired(entry) then return end
    assert(object(response),'Permission callback must return an explicit response object')
    local pending=s.client.interactions[key]
    if not pending or pending.answered then return end
    -- Callback-owned mutations must never redirect the reply.
    local route={session_id=session,turn_id=turn,data={request_id=requestId}}
    local id=s.client:reply(route,response)
    local deadline=bridge.now_ms()+30000
    local ok,problem=pcall(function()
      await(function()
        -- Retirement prevents new sends; it does not erase an already-sent
        -- reply's outcome. Genuine late rejection must remain a failure.
        local ready,result=s.client:tryAwait(id)
        if ready then return true,true,result end
        if bridge.now_ms()>=deadline then error('Permission reply acknowledgement timed out',0) end
        return false
      end)
    end)
    if not ok then
      local message=object(problem) and problem.message
      local stale=message=='unknown or finished turn' or message=='unknown or resolved interaction' or
        message=='No pending interaction'
      if not (stale and retired(entry)) then error(problem,0) end
    end
  end,'decision',entry)
  return nil -- The managed callback owns the eventual reply.
end

function Worker:start(prompt)
  managed()
  assert(type(prompt)=='string' and prompt:find('%S'),'Prompt must be nonempty')
  local s=state(self)
  if s.active then error('One worker turn at a time; await the current turn',0) end
  local active={}
  s.active=active
  local ok,result=pcall(request,self,'turn.start',params(self,{prompt=prompt}),scheduler.scope.signal)
  if not ok then
    -- An ordinary RPC rejection starts no turn; allow the caller to correct it.
    if s.active==active then s.active=nil end
    error(result,0)
  end
  assert(object(result) and type(result.turn_id)=='string' and #result.turn_id>0,'Invalid turn start response')
  active.id=result.turn_id
  return result.turn_id
end
function Worker:wait(turn,timeout)
  managed()
  local s=state(self)
  assert(s.active and s.active.id==turn,'Unknown or consumed turn')
  assert(not s.active.waiting,'A wait for this turn already exists')
  timeout=timeout or 300000
  assert(integer(timeout,2147483647) and timeout>0,'Invalid turn wait timeout')
  s.active.waiting=true
  local deadline=bridge.now_ms()+timeout
  local result=await(function()
    checkSignal(scheduler.scope.signal)
    local ready,value=s.client:tryWait(turn)
    if ready then return true,true,value end
    if bridge.now_ms()>=deadline then fail('Worker turn wait timed out'); error(scheduler.failure,0) end
    return false
  end)
  s.active=nil
  return result
end
function Worker:turn(prompt,timeout) return self:wait(self:start(prompt),timeout) end
function Worker:interrupt(turn)
  managed()
  local s=state(self)
  assert(s.active and s.active.id==turn and not s.active.finished,'No active turn with that ID')
  assert(not s.interruptPending and not s.active.interrupted,'Interruption already requested')
  local active=s.active
  active.interrupted=true; s.interruptPending=true
  local ok,result=pcall(request,self,'turn.interrupt',params(self,{turn_id=turn}),scheduler.scope.signal)
  s.interruptPending=false
  if not ok then
    if s.active==active then active.interrupted=nil end
    error(result,0)
  end
  assert(object(result) and result.interrupt_requested==true and result.turn_id==turn,
    'Interruption acknowledgement has an invalid turn identity')
  return result
end
function Worker:reply(item,response)
  managed()
  local s=state(self)
  local id=s.client:reply(item,response)
  local deadline=bridge.now_ms()+30000
  return await(function()
    checkSignal(scheduler.scope.signal)
    local ready,value=s.client:tryAwait(id)
    if ready then return true,true,value end
    if bridge.now_ms()>=deadline then
      local failure='RPC interaction.reply acknowledgement timed out'
      fail(failure); error(failure,0)
    end
    return false
  end)
end
local function closeWorker(worker,cleanup)
  local s=workers[worker]
  if s.closed then return end
  for _,entry in pairs(scheduler.decisions) do retire(entry,'Worker closed') end
  if s.client.failure~=nil or not s.client.opened then s.closed=true; return end
  local id=s.client:request('session.close',s.client:_params({}))
  s.client.closing=true; s.client.interactions={}
  local deadline=bridge.now_ms()+30000
  local result=await(function()
    if not cleanup then checkSignal(scheduler.scope.signal) end
    local ready,value=s.client:tryAwait(id)
    if ready then return true,true,value end
    if bridge.now_ms()>=deadline then error('Session close acknowledgement timed out',0) end
    return false
  end)
  -- Due replies may arrive out of order. A native close acknowledgement must
  -- not discard an earlier interaction.reply acknowledgement still in flight.
  await(function()
    local waiting=false
    for _,task in pairs(scheduler.tasks) do if task.kind=='decision' then waiting=true end end
    if not waiting then return true,true,nil end
    if bridge.now_ms()>=deadline then error('Permission callbacks did not settle during close',0) end
    return false
  end)
  s.client.closed=true; s.client.pending={}; s.client.finished={}
  s.closed=true; s.active=nil
  return result
end
function Worker:close()
  managed()
  assert(scheduler.current.kind=='root','Only the workflow body may close the worker')
  return closeWorker(self,false)
end

local Scope={}
Scope.__index=Scope
function Scope:cancel(message) fail(cancellation(message or 'Caller cancelled scope')) end
function Scope:open(profile,options)
  managed()
  assert(not scheduler.worker,'One worker session per supervision scope')
  assert(object(profile),'Profile must be an object')
  options=options or {}
  assert(object(options),'Callbacks must be an object')
  for key,value in pairs(options) do
    assert((key=='onItem' or key=='onInteraction') and type(value)=='function','Unknown or invalid worker callback')
  end
  local worker=setmetatable({},Worker)
  local s={options={onItem=options.onItem,onInteraction=options.onInteraction}}
  s.client=Client.new({onItem=function(item) observe(s,item) end,onInteraction=function(item) return decision(s,item) end})
  workers[worker]=s; scheduler.worker=worker
  local p=json.object({}); for key,value in pairs(profile) do p[key]=value end; p.stream=true
  local ok,opened=pcall(function()
    local result=request(worker,'session.open',p,self.signal)
    s.client:acceptOpen(result)
    return result
  end)
  if not ok then fail(opened); error(opened,0) end
  worker.opened=opened
  return worker
end
function Scope:sleep(ms,signal)
  signal=managed(signal)
  assert(integer(ms,300000),'Sleep must be an integer in 0..300000 milliseconds')
  local deadline=bridge.now_ms()+ms
  return await(function()
    checkSignal(signal)
    if bridge.now_ms()>=deadline then return true,true,nil end
    return false
  end)
end
function Scope:exec(spec,signal)
  signal=managed(signal)
  assert(object(spec),'Command specification must be an object')
  local id=bridge.exec_start(json.encode(spec))
  scheduler.execs[id]=true
  local cancelled=false
  return await(function()
    local why=reason(signal) or reason(self.signal)
    if why~=nil and not cancelled then bridge.exec_cancel(id); cancelled=true end
    local raw=bridge.exec_poll(id)
    if raw==nil then return false end
    scheduler.execs[id]=nil
    if why~=nil then return true,false,why end
    return true,true,json.decode(raw)
  end)
end

Due={}
function Due.run(body)
  assert(scheduler==nil,'Exactly one Due.run workflow is permitted')
  assert(type(body)=='function','Due.run requires a workflow function')
  local scope=setmetatable({signal=newSignal()},Scope)
  scheduler={scope=scope,tasks={},decisions={},execs={},sequence=0,phase='running'}
  scheduler.root=spawn(function() return body(scope) end,'root')
end
local function complete(task,ok,value)
  if not ok and value==nil then value='Lua task failed without an error value' end
  task.done=true; task.result=value; task.ok=ok
  scheduler.tasks[task.id]=nil
  if task.kind=='root' then
    if not ok then fail(value) end
    scheduler.rootDone=true
  elseif task.kind=='decision' then
    local entry=task.entry
    local key=workers[scheduler.worker].client:_key(entry.turn,entry.request)
    if scheduler.decisions[key]==entry then scheduler.decisions[key]=nil end
    local cancelled=reason(entry.signal)
    if not ok and not (cancelled~=nil and value==cancelled) then fail(value) end
  elseif task.kind=='close' then
    if not ok then fail(value) end
    scheduler.closeDone=true
  end
end
local function advance(task)
  local ready,ok,value=true,true,nil
  if task.wait then
    local checked,a,b,c=pcall(task.wait.check)
    if not checked then ready,ok,value=true,false,a else ready,ok,value=a,b,c end
    if not ready then return end
  end
  scheduler.current=task
  local resumed,result=resume(task.co,ok,value)
  scheduler.current=nil
  if not resumed or status(task.co)=='dead' then complete(task,resumed,result); return end
  if not object(result) or result.marker~=token or type(result.check)~='function' then
    complete(task,false,'Unmanaged coroutine yield'); return
  end
  task.wait=result
end
function Due._step()
  assert(scheduler,'Async script must call Due.run')
  if scheduler.phase=='done' then return true end
  local s=scheduler.worker and workers[scheduler.worker]
  if s and not s.closed and s.client.failure==nil then
    -- Bound work per tick so native streams and suspended callbacks both progress.
    for _=1,32 do
      local ok,got=pcall(s.client.poll,s.client,0)
      if not ok then fail(got); break end
      if not got then break end
    end
  end
  if s and s.client.failure~=nil then fail(s.client.failure) end
  local todo={}; for _,task in pairs(scheduler.tasks) do todo[#todo+1]=task end
  for _,task in ipairs(todo) do if not task.done then advance(task) end end
  if scheduler.rootDone and scheduler.phase=='running' then
    if s and s.active then fail('Workflow returned with an active turn or unconsumed worker result') end
    scheduler.phase='closing'
    abort(scheduler.scope.signal,scheduler.failure or cancellation('Scope closed'))
    for _,entry in pairs(scheduler.decisions) do retire(entry,'Scope closed') end
    scheduler.closer=spawn(function()
      if scheduler.worker then closeWorker(scheduler.worker,true) end
    end,'close')
  end
  if scheduler.phase=='closing' and scheduler.closeDone and next(scheduler.tasks)==nil then
    scheduler.phase='done'
    if next(scheduler.execs)~=nil then fail('Unsettled verifier operation') end
    if scheduler.failure~=nil then
      -- Structured RPC failures and cancellation reasons retain their identity
      -- inside coroutines, but the host needs their message in its receipt.
      local failure=scheduler.failure
      error(object(failure) and type(failure.message)=='string' and failure.message or tostring(failure),0)
    end
    Due.result=scheduler.root.result
    return true
  end
  return false
end

-- Trusted scripts still get coroutines through managed workflow/callback calls.
-- Raw coroutine creation/resumption would escape ownership and is unavailable.
coroutine={running=coroutine.running,isyieldable=coroutine.isyieldable}
DueClient=nil

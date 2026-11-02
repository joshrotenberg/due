-- Host supplies JSON args: {profile: {...}, marker?: string,
-- first_prompt?: string, second_prompt?: string}. No guest imports or I/O.
local args = json.decode(bridge.args)
assert(type(args) == 'table' and type(args.profile) == 'table', 'profile required')
local marker = args.marker or 'due-marker-7391'
local turnTimeout = args.turn_timeout_ms or 300000
assert(type(marker) == 'string' and #marker > 0, 'marker must be nonempty')
local provider
local client = DueClient.new({
  onItem = function(item)
    -- Some decoded native numbers cannot pass outbound JSON validation. The
    -- host owns raw bytes; log an explicit summary instead of failing the turn.
    local ok, line = pcall(json.encode, item)
    if ok then bridge.log(line)
    else bridge.log('Item ' .. tostring(item.cursor) .. ' ' .. item.kind ..
      ' (decoded payload cannot be JSON-encoded; raw payload retained by host)') end
  end,
  onInteraction = function(item)
    bridge.log('Denying unexpected permission request')
    if provider == 'claude' then
      return json.object({behavior='deny', message='Unexpected permission in conversation example'})
    end
    local request = item.data and item.data.request
    assert(provider == 'codex' and request and type(request.method) == 'string' and
      request.method:match('requestApproval$'), 'Unexpected interaction with no known denial shape')
    return json.object({decision='decline'})
  end,
})
local opened = client:open(args.profile)
provider = opened.provider
local ok, failure = pcall(function()
  local first = client:turn(args.first_prompt or ('Remember this marker: ' .. marker .. '. Do not use tools or access files. Reply with only the word stored.'), turnTimeout)
  assert(first.status == 'succeeded', 'First turn failed')
  local second = client:turn(args.second_prompt or 'What marker did I ask you to remember? Do not use tools or access files. Reply with only the marker.', turnTimeout)
  assert(second.status == 'succeeded', 'Second turn failed')
  assert(type(second.text) == 'string' and second.text:find(marker, 1, true), 'Recall did not include marker')
end)
local closed, closeError = pcall(function() client:close() end)
if not ok then error(failure, 0) end
if not closed then error(closeError, 0) end

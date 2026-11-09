-- Synthetic workflow for the same fake native executables used by due-js.
Due.run(function(scope)
  local function settle(run, after, response)
    local approvals, items, result = 0, 0, nil
    while true do
      local page = scope.readItems(run.session, after, 4)
      after = page.cursor
      for _, item in ipairs(page.items) do
        if item.run == run.run then
          items = items + 1
          if item.interaction ~= json.null then
            if not response then error("unexpected fixture approval") end
            scope.respond(item.interaction, response)
            approvals = approvals + 1
          end
        end
      end
      if after >= page.latest_cursor then
        if result then return result, after, approvals, items end
        local waited = scope.wait({ run.run }, 25)
        if waited.condition_met then
          result = waited.runs[1].result
          if result.status ~= "succeeded" then error(json.encode(result.error)) end
        end
      end
    end
  end

  local function conversation(provider, response)
    local first = scope.spawn(provider, "first")
    local initial, after = settle(first, 0)
    local follow = scope.followUp(first.session, "ask")
    local second, _, approvals, items = settle(follow, after, response)
    return {
      provider = provider,
      first = initial.text,
      second = second.text,
      sameNativeSession = type(initial.session_id) == "string"
        and initial.session_id == second.session_id,
      approvals = approvals,
      items = items,
    }
  end

  return scope.parallel(
    function() return conversation("claude", { behavior = "allow" }) end,
    function() return conversation("codex", { decision = "decline" }) end
  )
end)

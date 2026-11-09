// Synthetic workflow for the fake native executables in tests/workflow.rs.
Due.run(async scope => {
  async function settle(run, after, response) {
    let approvals = 0;
    let items = 0;
    let result;
    for (;;) {
      const page = await scope.readItems(run.session, after, 4);
      after = page.cursor;
      for (const item of page.items) {
        if (item.run !== run.run) continue;
        items++;
        if (item.interaction) {
          if (!response) throw new Error('unexpected fixture approval');
          await scope.respond(item.interaction, response);
          approvals++;
        }
      }
      if (after < page.latest_cursor) continue;
      if (result) return { result, after, approvals, items };
      const waited = await scope.wait([run.run], 25);
      if (waited.condition_met) {
        result = waited.runs[0].result;
        if (result.status !== 'succeeded') throw new Error(JSON.stringify(result.error));
      }
    }
  }

  async function conversation(provider, response) {
    const first = await scope.spawn(provider, 'first');
    const initial = await settle(first, 0);
    const follow = await scope.followUp(first.session, 'ask');
    const second = await settle(follow, initial.after, response);
    return {
      provider,
      first: initial.result.text,
      second: second.result.text,
      sameNativeSession: typeof initial.result.session_id === 'string'
        && initial.result.session_id === second.result.session_id,
      approvals: second.approvals,
      items: second.items,
    };
  }

  return Promise.all([
    conversation('claude', { behavior: 'allow' }),
    conversation('codex', { decision: 'decline' }),
  ]);
});

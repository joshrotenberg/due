// Two-turn Due RPC conversation for the embedded guest clients. Loaded after clients/client.js.
//
// bridge.args is a JSON string, for example:
//   {"profile":{"provider":"claude","model":"sonnet","cwd":"/tmp/work","timeout":"2m"},
//    "marker":"larch-4821",
//    "first_prompt":"...optional...", "second_prompt":"...optional...", "turn_timeout_ms":300000}
// Only "profile" is required. Every observed session item is logged. Any permission request is
// explicitly denied (the denial is logged); unknown interaction kinds fail the session closed.
var conversationResult = (function () {
  'use strict';

  var input = JSON.parse(bridge.args);
  if (input === null || typeof input !== 'object' || Array.isArray(input) ||
      input.profile === null || typeof input.profile !== 'object' || Array.isArray(input.profile)) {
    throw new Error('bridge.args must be a JSON object with a "profile" object');
  }
  var profile = input.profile;
  var marker = typeof input.marker === 'string' && input.marker ? input.marker : 'due-marker-7391';
  var first = typeof input.first_prompt === 'string' ? input.first_prompt :
    'Remember this marker: ' + marker + '. Do not use tools or access files. Reply with only the word stored.';
  var second = typeof input.second_prompt === 'string' ? input.second_prompt :
    'What marker did I ask you to remember? Do not use tools or access files. Reply with only the marker.';
  var turnTimeout = input.turn_timeout_ms === undefined ? 300000 : input.turn_timeout_ms;

  function onItem(item) {
    bridge.log('item ' + JSON.stringify(item));
  }

  // Explicit denial in the provider's native shape. Nothing is ever approved.
  function onInteraction(item) {
    var provider = profile.provider;
    var request = item.data && item.data.request;
    var response;
    if (provider === 'claude') {
      response = { behavior: 'deny', message: 'This conversation is read-only and does not permit tools.' };
    } else if (provider === 'codex' && request && typeof request.method === 'string' &&
               /requestApproval$/.test(request.method)) {
      response = { decision: 'decline' };
    } else {
      throw new Error('unexpected interaction with no known denial shape: ' + JSON.stringify(item.data));
    }
    bridge.log('denying interaction ' + item.data.request_id + ' with ' + JSON.stringify(response));
    return response;
  }

  function checked(result, label) {
    if (result.status !== 'succeeded') {
      throw new Error(label + ' turn did not succeed: ' + result.status + ' ' + JSON.stringify(result.error));
    }
    if (typeof result.text !== 'string') throw new Error(label + ' turn has no text');
    return result;
  }

  var client = new DueClient({ onItem: onItem, onInteraction: onInteraction });
  var summary;
  try {
    var opened = client.open(profile);
    bridge.log('opened session ' + opened.session_id);
    var r1 = checked(client.turn(first, turnTimeout), 'first');
    var r2 = checked(client.turn(second, turnTimeout), 'second');
    if (r2.text.indexOf(marker) < 0) {
      throw new Error('second answer does not include marker ' + JSON.stringify(marker) + ': ' + JSON.stringify(r2.text));
    }
    summary = { ok: true, marker: marker, first: r1.text, second: r2.text };
  } catch (e) {
    // Report the failure; still try an orderly close unless the client already failed.
    if (!client.failure) {
      try { client.close(); } catch (ce) { bridge.log('close after error failed: ' + ce.message); }
    }
    throw e;
  }
  var closed = client.close();
  summary.close = closed;
  bridge.log('result ' + JSON.stringify(summary));
  return summary;
})();

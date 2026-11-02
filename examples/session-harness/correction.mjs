// Identical application logic for CLI-resume and resident conversations.
function answer(result, expected) {
  if (result.status !== 'succeeded') throw new Error(`turn failed: ${JSON.stringify(result.error)}`);
  let value;
  try { value = JSON.parse(result.text); }
  catch { throw new Error('answer was not a JSON object; execution success is separate from answer validity'); }
  if (!value || Array.isArray(value) || typeof value !== 'object' ||
      Object.keys(value).length !== Object.keys(expected).length ||
      Object.entries(expected).some(([key, wanted]) => value[key] !== wanted)) {
    throw new Error(`answer failed the caller's check: ${JSON.stringify(value)}`);
  }
  return value;
}

export async function correctionWorkflow(agent) {
  const draft = answer(await agent.turn({ id: 'draft', prompt:
    'A small conversation test. Do not use tools or access files. Remember marker larch-4821. ' +
    'The order has 2 items at 7 each, plus a flat shipping fee of 5. ' +
    'Reply only with a JSON object containing marker, revision (1), and total. No Markdown.' }),
    { marker: 'larch-4821', revision: 1, total: 19 });

  // The marker, quantities, and prices are deliberately absent from this follow-up.
  const corrected = answer(await agent.turn({ id: 'correction', prompt:
    'Correction: the flat shipping fee is 3. Keep the order details and marker from our conversation. ' +
    'Do not use tools or access files. Reply only with the same JSON fields, revision 2 and the corrected total.' }),
    { marker: 'larch-4821', revision: 2, total: 17 });
  return { draft, corrected };
}

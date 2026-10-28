export async function pipeline(due, stages, input) {
  let value = input;
  for (const stage of stages) value = await stage(due, value);
  return value;
}

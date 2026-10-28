import {stage} from "local/pipeline";

export async function review(due, config, name, previous) {
  return stage(due, {
    ...config,
    task: config.task + " Independently verify tests and effects."
  }, name, previous);
}

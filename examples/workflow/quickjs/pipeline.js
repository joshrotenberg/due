// This function is identical when called directly or in due.concurrent.
export async function stage(due, config, name, previous) {
  return due.run({id:config.prefix+"-"+name,profile:config.profile,
    prompt:name+": "+config.task+"\nPrior outcomes: "+JSON.stringify(previous)});
}
export async function pipeline(due, config) {
  const steps=[];
  for (const name of ["design","implement","review"]) {
    due.phase(name);
    const selected=config.stages?.[name] ?? stage;
    const outcome=await selected(due,config,name,steps);
    steps.push(outcome);
    if (!outcome.ok) return {ok:false,stopped_at:name,steps};
  }
  return {ok:true,steps};
}

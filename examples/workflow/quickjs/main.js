import {pipeline,stage} from "local/pipeline";
export default async function(due,input) {
  const outcomes=await due.concurrent("implementations",[
    child=>pipeline(child,{prefix:"a",profile:"workspace_a",task:input.task}),
    child=>pipeline(child,{prefix:"b",profile:"workspace_b",task:input.task,
      stages:{review:async (scope,config,name,previous)=>
        stage(scope,{...config,task:config.task+" Independently verify tests and effects."},name,previous)}})
  ]);
  const findings=outcomes.flatMap((p,component)=>p.steps.flatMap(step=>
    step.ok?step.answer.findings.map(text=>({component:component+1,text})):[]));
  findings.sort((a,b)=>a.text.localeCompare(b.text));
  due.phase("synthesis");
  const synthesis=await due.run({id:"synthesis",profile:"synthesizer",
    prompt:"Synthesize these outcomes; do not edit: "+JSON.stringify({outcomes,findings})});
  return {outcomes,findings,synthesis};
}

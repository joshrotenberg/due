(function(host, waitHost, isProxy) {
  'use strict';
  const parse = JSON.parse, stringify = JSON.stringify;
  const String = globalThis.String, Error = globalThis.Error, Set = globalThis.Set;
  const abs = Math.abs;
  const setHas = Function.prototype.call.bind(Set.prototype.has);
  const setAdd = Function.prototype.call.bind(Set.prototype.add);
  const setDelete = Function.prototype.call.bind(Set.prototype.delete);
  const ownKeys = Reflect.ownKeys, descriptor = Object.getOwnPropertyDescriptor;
  const proto = Object.getPrototypeOf, objectProto = Object.prototype, arrayProto = Array.prototype;
  const isArray = Array.isArray, isFinite = Number.isFinite, isInteger = Number.isInteger;
  const create = Object.create, freeze = Object.freeze;
  const handles = new WeakMap();
  const weakGet = Function.prototype.call.bind(WeakMap.prototype.get);
  const weakSet = Function.prototype.call.bind(WeakMap.prototype.set);
  const weakHas = Function.prototype.call.bind(WeakMap.prototype.has);
  const P = Promise, all = Promise.all.bind(P);
  // Protect the interpreter machinery used by Rust PromiseFuture and this library.
  freeze(P.prototype); freeze(P); freeze(Function.prototype);
  freeze(objectProto); freeze(arrayProto);
  function snapshot(value) {
    const seen = new Set();
    function copy(v, depth) {
      if (depth > 32) throw Error('JSON nesting exceeds 32');
      if (v === null || typeof v === 'boolean' || typeof v === 'string') return v;
      if (typeof v === 'number') {
        if (!isFinite(v) || (isInteger(v) && abs(v) > 9007199254740991)) throw Error('nonfinite or unsafe JSON number');
        return v;
      }
      if (typeof v !== 'object') throw Error('unsupported JSON value: '+typeof v);
      if (isProxy(v)) throw Error('Proxy values are not JSON');
      if (weakHas(handles,v)) throw Error('run handles cannot cross the JSON boundary');
      if (setHas(seen,v)) throw Error('cyclic JSON object');
      setAdd(seen,v);
      const array=isArray(v), keys=ownKeys(v);
      if (proto(v)!==(array?arrayProto:objectProto) && !(proto(v)===null&&!array)) throw Error('custom JSON prototype');
      if (array && v.length>4096) throw Error('array exceeds 4096 items');
      if (!array && keys.length>256) throw Error('object exceeds 256 entries');
      const out=array?[]:create(null);
      let count=0;
      for (const key of keys) {
        if (array&&key==='length') continue;
        if (typeof key!=='string') throw Error('symbol JSON key');
        const d=descriptor(v,key);
        if (!d || !('value' in d) || !d.enumerable) throw Error('JSON requires enumerable data properties');
        if (array && (String(count)!==key)) throw Error('sparse or mixed JSON array');
        out[key]=copy(d.value,depth+1); count++;
      }
      if (array && count!==v.length) throw Error('sparse JSON array');
      setDelete(seen,v);return out;
    }
    const raw=stringify(copy(value,0));
    if (raw.length>1048576) throw Error('JSON exceeds 1 MiB');
    return raw;
  }
  function message(error) {
    if(typeof error==='string') return error;
    if(error!==null && typeof error==='object' && !isProxy(error)) {
      const d=descriptor(error,'message');
      if(d && 'value' in d && typeof d.value==='string') return d.value;
    }
    return 'JavaScript host contract failure';
  }
  function make(scope) {
    function fatal(error) {host(scope,'fail',stringify(message(error)));throw error;}
    function call(op,value) {try{return parse(host(scope,op,snapshot(value)));}catch(e){return fatal(e);}}
    function handle(id) {const h=freeze(create(null));weakSet(handles,h,{scope,id});return h;}
    function ids(list) {
      if (!isArray(list) || isProxy(list) || ownKeys(list).length!==list.length+1) throw Error('wait requires dense handles array');
      const result=[];
      for(let i=0;i<list.length;i++) {
        const d=descriptor(list,String(i));
        if(!d || !('value' in d)) throw Error('wait requires data properties');
        const v=weakGet(handles,d.value);
        if(!v||v.scope!==scope)throw Error('run handle belongs to another function scope');
        result.push(v.id);
      }
      return result;
    }
    function waiting(op,list) {try{return waitHost(scope,op,stringify(ids(list))).then(parse);}catch(e){return fatal(e);}}
    const due={
      phase:value=>call('phase',value),log:value=>call('log',value),
      start:job=>handle(call('start',['run',[job]])[0]),
      wait:h=>waiting('wait',[h]),wait_any:hs=>waiting('wait',hs),cancel:h=>waiting('cancel',[h]),
      run:job=>{const h=due.start(job);return due.wait(h);},
      parallel:(group,jobs)=>{const hs=call('start',[group,jobs]).map(handle);return all(hs.map(due.wait)).then(values=>parse(snapshot(values)));},
      concurrent:(group,functions)=>{
        try {
          if(!isArray(functions)||isProxy(functions))throw Error('concurrent requires functions array');
          const keys=ownKeys(functions);
          if(keys.length!==functions.length+1)throw Error('concurrent requires dense functions');
          const fns=[];
          for(let i=0;i<functions.length;i++){const d=descriptor(functions,String(i));if(!d||!('value'in d)||typeof d.value!=='function')throw Error('concurrent requires functions');fns.push(d.value);}
          const scopes=call('open',[group,fns.length]);
          return all(fns.map((fn,i)=>P.resolve().then(async()=>{
            try {const value=parse(snapshot(await fn(make(scopes[i]))));await waitHost(scopes[i],'close','[]');return value;}
            catch(e){host(scopes[i],'fail',stringify(message(e)));throw e;}
          }))).then(values=>parse(snapshot(values)));
        }catch(e){return fatal(e);}
      }
    };
    return freeze(due);
  }
  return freeze({invoke:async(fn,input)=>snapshot(await fn(make(0),parse(input)))});
})

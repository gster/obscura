use crate::runtime::ObscuraJsRuntime;

fn runtime() -> ObscuraJsRuntime {
    let mut rt = ObscuraJsRuntime::new(obscura_net::EffectivePersona::builtin(
        obscura_net::StealthProfile::MacChrome153));
    rt.set_dom(obscura_dom::parse_html("<html><body></body></html>"));
    rt.set_url("https://example.com/test");
    rt.run_page_init();
    rt
}

#[tokio::test(flavor="current_thread")]
async fn wasm_streaming_compiles_and_rejects_invalid_inputs_without_aborting() {
    let mut rt = runtime();
    let result = rt.call_function_on_for_cdp(r#"async()=>{
        const bytes=new Uint8Array([0,97,115,109,1,0,0,0]);
        const response=()=>new Response(bytes,{headers:{'Content-Type':'application/wasm'}});
        const module=await WebAssembly.compileStreaming(Promise.resolve(response()));
        const instance=await WebAssembly.instantiateStreaming(response());
        const used=response();await used.arrayBuffer();
        const results=[];
        for(const source of [null,bytes,{arrayBuffer:()=>Promise.resolve(bytes.buffer)},
            Object.create(Response.prototype),new Response(bytes),used,
            new Response(bytes,{status:404,headers:{'Content-Type':'application/wasm'}})]) {
          for(const method of ['compileStreaming','instantiateStreaming']) {
            try { await WebAssembly[method](source);results.push(false); }
            catch(error) { results.push(error instanceof TypeError); }
          }
        }
        return [module instanceof WebAssembly.Module,instance.instance instanceof WebAssembly.Instance,
          results.length===14&&results.every(Boolean),
          !Object.hasOwn(globalThis,'__obscura_response_registry_handoff')];
    }"#,None,&[],true,true).await.unwrap();
    assert!(!result.thrown,"{}",result.description);
    let result=result.value;
    assert_eq!(result,Some(serde_json::json!([true,true,true,true])));
}

#[tokio::test(flavor="current_thread")]
async fn user_timing_records_clone_detail_clear_and_deliver_observers() {
    let mut rt = runtime();
    let result = rt.call_function_on_for_cdp(r#"async()=>{
        const seen=[];const observer=new PerformanceObserver(list=>seen.push(...list.getEntries()));
        observer.observe({entryTypes:['mark','measure']});
        const detail={n:7};const mark=performance.mark('start',{startTime:10,detail});detail.n=99;
        performance.mark('end',{startTime:20});
        const measure=performance.measure('span','start','end');
        const options=performance.measure('options',{end:'end',duration:5,detail:{ok:true}});
        const copy=new PerformanceMark('constructed',{startTime:3});
        const entries=performance.getEntriesByType('mark');
        const same=entries[0]===mark&&mark.detail.n===7&&copy.startTime===3&&entries.length===2;
        const stats=measure.startTime===10&&measure.duration===10&&options.startTime===15&&options.duration===5;
        const names=Object.keys(mark.toJSON()).sort().join(',')==='detail,duration,entryType,name,startTime';
        performance.clearResourceTimings();const retained=performance.getEntries().length===4;
        performance.clearMarks('start');performance.clearMeasures();
        await new Promise(resolve=>setTimeout(resolve,0));observer.disconnect();
        let invalid=false;try {performance.measure('missing','absent');} catch(error){invalid=error.name==='SyntaxError';}
        return [same,stats,names,retained,seen.length===4,
          performance.getEntriesByType('mark').length===1,performance.getEntriesByType('measure').length===0,invalid];
    }"#,None,&[],true,true).await.unwrap();
    assert!(!result.thrown,"{}",result.description);
    let result=result.value;
    assert_eq!(result,Some(serde_json::json!([true,true,true,true,true,true,true,true])));
}

#[tokio::test(flavor="current_thread")]
async fn user_timing_worker_entries_are_local_and_observed() {
    let mut rt=runtime();
    let result=rt.call_function_on_for_cdp(r#"async()=>{
      return new Promise((resolve,reject)=>{
        const worker=new Worker(URL.createObjectURL(new Blob([`
          const observer=new PerformanceObserver(list=>{
            postMessage([list.getEntries()[0].name,performance.getEntriesByType('mark').length,
              PerformanceObserver.supportedEntryTypes.includes('mark')]);
          });observer.observe({type:'mark'});performance.mark('worker');
        `],{type:'text/javascript'})));
        worker.onmessage=event=>{worker.terminate();resolve([event.data,performance.getEntries().length]);};
        worker.onerror=reject;
      });
    }"#,None,&[],true,true).await.unwrap();
    assert!(!result.thrown,"{}",result.description);
    let result=result.value;
    assert_eq!(result,Some(serde_json::json!([["worker",1,true],0])));
}

#[tokio::test(flavor="current_thread")]
async fn wasm_streaming_uses_private_response_state_and_separates_lock_from_disturbance() {
    let mut rt=runtime();
    let result=rt.call_function_on_for_cdp(r#"async()=>{
      const bytes=new Uint8Array([0,97,115,109,1,0,0,0]);
      const make=()=>new Response(bytes,{headers:{'content-type':'application/wasm'}});
      const locked=make(),reader=locked.body.getReader();
      const untouched=!locked.bodyUsed;
      let rejected=false;try { await WebAssembly.compileStreaming(locked); } catch(e) { rejected=e instanceof TypeError; }
      reader.releaseLock();const unlocked=await WebAssembly.compileStreaming(locked);
      const read=make();await read.body.getReader().read();
      let disturbed=false;try {await WebAssembly.compileStreaming(read);} catch(e) {disturbed=e instanceof TypeError;}
      const shadow=make();for(const name of ['headers','ok','status','bodyUsed','_bodyBytes'])
        Object.defineProperty(shadow,name,{get(){throw Error('author '+name);}});
      const original=Response.prototype.arrayBuffer,compile=WebAssembly.compile;
      Response.prototype.arrayBuffer=()=>{throw Error('author method');};WebAssembly.compile=()=>{throw Error('author compile');};
      let privateState=false;try {privateState=(await WebAssembly.compileStreaming(shadow)) instanceof WebAssembly.Module;}
      finally {Response.prototype.arrayBuffer=original;WebAssembly.compile=compile;}
      const nullBody=new Response(null);await nullBody.arrayBuffer();
      return [untouched,rejected,unlocked instanceof WebAssembly.Module,disturbed,privateState,!nullBody.bodyUsed];
    }"#,None,&[],true,true).await.unwrap();
    assert!(!result.thrown,"{}",result.description);
    let result=result.value;
    assert_eq!(result,Some(serde_json::json!([true,true,true,true,true,true])));
}

#[tokio::test(flavor="current_thread")]
async fn user_timing_latest_mark_observer_order_and_native_clone_graph() {
    let mut rt=runtime();
    let result=rt.call_function_on_for_cdp(r#"async()=>{
      const seen=[];const observer=new PerformanceObserver(list=>{
        seen.push(list.getEntries().map(e=>e.startTime),list.getEntriesByType('mark').map(e=>e.startTime),
          list.getEntriesByName('x').map(e=>e.startTime));
      });observer.observe({type:'mark'});
      performance.mark('x',{startTime:20});performance.mark('x',{startTime:10});
      const measure=performance.measure('latest',{start:'x',end:25});
      const buffer=new ArrayBuffer(4),view=new Uint8Array(buffer);view[0]=7;
      const detail=performance.mark('graph',{startTime:30,detail:{buffer,view,map:new Map([['x',new Date(3)]])}}).detail;
      view[0]=9;
      const cloned=detail.buffer===detail.view.buffer&&detail.view[0]===7&&detail.map.get('x').getTime()===3;
      let failures=0;for(const detail of [new WeakMap(),Promise.resolve(1),()=>{}]) {
        try {performance.mark('invalid',{detail});} catch(e) {if(e.name==='DataCloneError')failures++;}
      }
      await new Promise(resolve=>setTimeout(resolve,0));observer.disconnect();
      return [measure.startTime===10&&measure.duration===15,cloned,failures===3,
        seen[0].slice(0,2).join(',')==='10,20',seen[1].slice(0,2).join(',')==='10,20',seen[2].join(',')==='10,20'];
    }"#,None,&[],true,true).await.unwrap();
    assert!(!result.thrown,"{}",result.description);
    let result=result.value;
    assert_eq!(result,Some(serde_json::json!([true,true,true,true,true,true])));
}

#[tokio::test(flavor="current_thread")]
async fn indexed_db_upgrade_order_clone_abort_constraints_and_catalog() {
    let mut rt=runtime();
    let result=rt.call_function_on_for_cdp(r#"async()=>{
      const request=req=>new Promise((resolve,reject)=>{req.onsuccess=()=>resolve(req.result);req.onerror=()=>reject(req.error);});
      const done=tx=>new Promise((resolve,reject)=>{tx.oncomplete=()=>resolve();tx.onabort=()=>reject(tx.error);});
      const trace=[];const open=indexedDB.open('atomic',1);
      open.onupgradeneeded=()=>{
        trace.push('upgrade');const store=open.result.createObjectStore('items',{keyPath:'id',autoIncrement:true});
        store.createIndex('email','email',{unique:true});
        const put=store.add({email:'first'});put.onsuccess=()=>trace.push('put');
        open.transaction.oncomplete=()=>trace.push('complete');
      };
      const db=await request(open);trace.push('open');
      const order=trace.join(',')==='upgrade,put,complete,open';
      const names=Array.from(db.objectStoreNames).join(',')==='items'&&db.objectStoreNames.contains('items');
      const read=db.transaction('items'),readonly=read.objectStore('items');let readOnly=false,schema=false;
      try {readonly.put({id:2});}catch(e){readOnly=e.name==='ReadOnlyError';}
      try {readonly.createIndex('bad','x');}catch(e){schema=e.name==='InvalidStateError';}
      const first=await request(readonly.get(1));const generated=first.id===1&&first.email==='first';
      const write=db.transaction('items','readwrite'),store=write.objectStore('items'),finished=done(write);
      const value={id:2,email:'second',nested:{n:7}};const put=store.put(value);value.nested.n=99;
      await request(put);await finished;
      const check=db.transaction('items');const cloned=(await request(check.objectStore('items').get(2))).nested.n===7;
      const abort=db.transaction('items','readwrite'),pending=abort.objectStore('items').put({id:3,email:'third'});
      const aborted=new Promise(resolve=>abort.onabort=resolve);const error=new Promise(resolve=>pending.onerror=()=>resolve(pending.error.name));
      abort.abort();const abortError=await error;await aborted;
      const afterAbort=db.transaction('items');const rollback=await request(afterAbort.objectStore('items').get(3))===undefined;
      const duplicate=db.transaction('items','readwrite'),dupStore=duplicate.objectStore('items');
      const a=dupStore.add({id:4,email:'fourth'}),b=dupStore.add({id:4,email:'other'});
      const dupAbort=new Promise(resolve=>duplicate.onabort=()=>resolve(duplicate.error.name));
      const dupError=new Promise(resolve=>b.onerror=()=>resolve(b.error.name));await request(a);
      const constraint=await dupError;await dupAbort;
      const readDup=db.transaction('items');const dupRollback=await request(readDup.objectStore('items').get(4))===undefined;
      const unique=db.transaction('items','readwrite'),u=unique.objectStore('items').add({id:5,email:'first'});
      const uniqueAbort=new Promise(resolve=>unique.onabort=resolve);let uniqueError;u.onerror=()=>{uniqueError=u.error.name;};await uniqueAbort;
      const catalog=(await indexedDB.databases()).some(entry=>entry.name==='atomic'&&entry.version===1);
      db.close();
      return [order,names,readOnly,schema,generated,cloned,abortError==='AbortError',rollback,
        constraint==='ConstraintError',dupRollback,uniqueError==='ConstraintError',catalog];
    }"#,None,&[],true,true).await.unwrap();
    assert!(!result.thrown,"{}",result.description);
    assert_eq!(result.value,Some(serde_json::json!([true,true,true,true,true,true,true,true,true,true,true,true])));
}

#[tokio::test(flavor="current_thread")]
async fn indexed_db_fifo_and_uncaught_handler_rollback() {
    let mut rt=runtime();
    let result=rt.call_function_on_for_cdp(r#"async()=>{
      const request=req=>new Promise((resolve,reject)=>{req.onsuccess=()=>resolve(req.result);req.onerror=()=>reject(req.error);});
      const open=indexedDB.open('fifo');open.onupgradeneeded=()=>open.result.createObjectStore('s');const db=await request(open);
      const order=[];const first=db.transaction('s','readwrite');first.objectStore('s').put('first','key');
      const second=db.transaction('s','readwrite');second.objectStore('s').put('second','key');
      const completed=new Promise(resolve=>{
        first.oncomplete=()=>{
          order.push(1);const third=db.transaction('s','readwrite');third.objectStore('s').put('third','key');
          third.oncomplete=()=>{order.push(3);resolve();};
        };second.oncomplete=()=>order.push(2);
      });await completed;
      const read=db.transaction('s');const last=await request(read.objectStore('s').get('key'));
      const failed=db.transaction('s','readwrite'),put=failed.objectStore('s').put('bad','bad');
      const aborted=new Promise(resolve=>failed.onabort=resolve);put.addEventListener('success',()=>{throw Error('uncaught');});await aborted;
      const verify=db.transaction('s');const rollback=await request(verify.objectStore('s').get('bad'))===undefined;
      db.close();
      const upgrade=indexedDB.open('throws');upgrade.onupgradeneeded=()=>{upgrade.result.createObjectStore('bad');throw Error('uncaught upgrade');};
      let upgradeAbort=false;try{await request(upgrade);}catch(e){upgradeAbort=e.name==='AbortError';}
      const absent=!(await indexedDB.databases()).some(entry=>entry.name==='throws');
      return [order.join(',')==='1,2,3',last==='third',rollback,upgradeAbort,absent];
    }"#,None,&[],true,true).await.unwrap();
    assert!(!result.thrown,"{}",result.description);
    assert_eq!(result.value,Some(serde_json::json!([true,true,true,true,true])));
}

#[tokio::test(flavor="current_thread")]
async fn indexed_db_worker_upgrade_waits_for_parent_connection_close() {
    let mut rt=runtime();
    let result=rt.call_function_on_for_cdp(r#"async()=>{
      const open=indexedDB.open('workers',1);open.onupgradeneeded=()=>open.result.createObjectStore('s');
      const db=await new Promise((resolve,reject)=>{open.onsuccess=()=>resolve(open.result);open.onerror=()=>reject(open.error);});
      const trace=[];db.onversionchange=event=>{trace.push(['version',event.oldVersion,event.newVersion]);};
      return new Promise((resolve,reject)=>{
        const worker=new Worker(URL.createObjectURL(new Blob([`
          const req=indexedDB.open('workers',2);req.onblocked=()=>postMessage('blocked');
          req.onsuccess=()=>{req.result.close();postMessage('success');};req.onerror=()=>postMessage(req.error.name);
        `],{type:'text/javascript'})));
        worker.onmessage=event=>{
          trace.push(event.data);
          if(event.data==='blocked')setTimeout(()=>db.close(),0);
          else if(event.data==='success'){worker.terminate();resolve(trace);}
          else reject(Error(event.data));
        };worker.onerror=reject;
      });
    }"#,None,&[],true,true).await.unwrap();
    assert!(!result.thrown,"{}",result.description);
    let value=result.value.unwrap();
    assert!(value.as_array().unwrap().contains(&serde_json::json!(["version",1,2])),"{value}");
    assert_eq!(value.as_array().unwrap().last(),Some(&serde_json::json!("success")));
    assert!(value.as_array().unwrap().contains(&serde_json::json!("blocked")));
}

#[tokio::test(flavor="current_thread")]
async fn indexed_db_committed_values_survive_document_replacement_and_isolate_context_origin() {
    let shared=std::sync::Arc::new(crate::indexed_db::Store::default());
    let mut first=runtime();first.set_indexed_db(shared.clone());
    let result=first.call_function_on_for_cdp(r#"async()=>{
      const req=indexedDB.open('persist');req.onupgradeneeded=()=>req.result.createObjectStore('s');
      const db=await new Promise(resolve=>req.onsuccess=()=>resolve(req.result));
      const tx=db.transaction('s','readwrite');tx.objectStore('s').put(new Map([['n',7]]),'key');
      await new Promise(resolve=>tx.oncomplete=resolve);db.close();return true;
    }"#,None,&[],true,true).await.unwrap();assert_eq!(result.value,Some(serde_json::json!(true)));drop(first);
    let mut next=runtime();next.set_indexed_db(shared.clone());
    let result=next.call_function_on_for_cdp(r#"async()=>{
      const req=indexedDB.open('persist');let upgraded=false;req.onupgradeneeded=()=>upgraded=true;
      const db=await new Promise(resolve=>req.onsuccess=()=>resolve(req.result));
      const tx=db.transaction('s'),read=tx.objectStore('s').get('key');
      const value=await new Promise(resolve=>read.onsuccess=()=>resolve(read.result));db.close();return [upgraded,value.get('n')];
    }"#,None,&[],true,true).await.unwrap();assert!(!result.thrown,"{}",result.description);
    assert_eq!(result.value,Some(serde_json::json!([false,7])));drop(next);
    let mut isolated=runtime();
    assert_eq!(isolated.call_function_on_for_cdp("async()=>await indexedDB.databases()",None,&[],true,true).await.unwrap().value,Some(serde_json::json!([])));
    isolated.set_indexed_db(shared);isolated.set_url("https://different.test/path");
    assert_eq!(isolated.call_function_on_for_cdp("async()=>await indexedDB.databases()",None,&[],true,true).await.unwrap().value,Some(serde_json::json!([])));
    isolated.set_url("data:text/html,opaque");
    let result=isolated.call_function_on_for_cdp("async()=>{try{await indexedDB.databases();return false;}catch(e){return e.name==='SecurityError';}}",None,&[],true,true).await.unwrap();
    assert_eq!(result.value,Some(serde_json::json!(true)));
}

#[tokio::test(flavor="current_thread")]
async fn native_storage_clone_and_wasm_accept_genuine_child_realm_values() {
    let mut rt=runtime();
    let child=crate::frame::FrameRealm::new(&mut rt,91,0,"https://example.com/child",
        "<html><body></body></html>").unwrap();
    child.execute_script(&mut rt,r#"
      const buffer=new ArrayBuffer(4);const view=new Uint8Array(buffer);view[0]=7;
      globalThis.payload={map:new Map([['date',new Date(3)]]),buffer,view};
      globalThis.reply=new Response(new Uint8Array([0,97,115,109,1,0,0,0]),{headers:{'content-type':'application/wasm'}});
    "#).unwrap();
    let result=rt.call_function_on_for_cdp(r#"async()=>{
      const w=__obscura_frameObjects[91].window;
      const copy=performance.mark('child',{detail:w.payload}).detail;
      const module=await WebAssembly.compileStreaming(w.reply);
      const values=[document.createElement('div'),w.document,performance.getEntries()[0],new Response('x')];
      let rejected=0;for(const detail of values) {try {performance.mark('host',{detail});}catch(e){if(e.name==='DataCloneError')rejected++;}}
      return [copy.map instanceof Map,copy.map.get('date') instanceof Date,copy.map.get('date').getTime()===3,
        copy.view.buffer===copy.buffer,copy.view[0]===7,module instanceof WebAssembly.Module,rejected===values.length];
    }"#,None,&[],true,true).await.unwrap();
    assert!(!result.thrown,"{}",result.description);
    assert_eq!(result.value,Some(serde_json::json!([true,true,true,true,true,true,true])));
    drop(child);
}

#[tokio::test(flavor="current_thread")]
async fn indexed_db_upgrade_drains_connection_and_close_finishes_upgrade_but_rejects_open() {
    let mut rt=runtime();
    let result=rt.call_function_on_for_cdp(r#"async()=>{
      const request=req=>new Promise((resolve,reject)=>{req.onsuccess=()=>resolve(req.result);req.onerror=()=>reject(req.error);});
      const first=indexedDB.open('drain',1);first.onupgradeneeded=()=>first.result.createObjectStore('s');const db=await request(first);
      db.onversionchange=()=>{const tx=db.transaction('s','readwrite');tx.objectStore('s').put('old connection drained',1);db.close();};
      const second=indexedDB.open('drain',2);const next=await request(second);
      const read=next.transaction('s');const drained=await request(read.objectStore('s').get(1));next.close();
      const close=indexedDB.open('close-upgrade');const trace=[];close.onupgradeneeded=()=>{close.result.createObjectStore('s').put('committed',1);close.transaction.oncomplete=()=>trace.push('complete');close.result.close();};let aborted=false;
      try{await request(close);}catch(error){aborted=error.name==='AbortError';}
      return [drained==='old connection drained',aborted,trace.join(',')==='complete',
        (await indexedDB.databases()).some(db=>db.name==='close-upgrade'&&db.version===1)];
    }"#,None,&[],true,true).await.unwrap();
    assert!(!result.thrown,"{}",result.description);
    assert_eq!(result.value,Some(serde_json::json!([true,true,true,true])));
}

#[tokio::test(flavor="current_thread")]
async fn indexed_db_cancelled_error_bubbles_restores_generator_and_unrelated_timer_is_inactive() {
    let mut rt=runtime();
    let result=rt.call_function_on_for_cdp(r#"async()=>{
      const request=req=>new Promise((resolve,reject)=>{req.onsuccess=()=>resolve(req.result);req.onerror=()=>reject(req.error);});
      const open=indexedDB.open('error-recovery');open.onupgradeneeded=()=>{
        const store=open.result.createObjectStore('s',{autoIncrement:true});store.createIndex('email','email',{unique:true});
      };const db=await request(open);
      const tx=db.transaction('s','readwrite'),store=tx.objectStore('s'),events=[];
      const completed=new Promise((resolve,reject)=>{tx.oncomplete=resolve;tx.onabort=()=>reject(tx.error);});
      tx.onerror=event=>{events.push([event.target.error.name,event.currentTarget===tx,event.eventPhase]);event.preventDefault();};
      const first=store.add({email:'a'});const duplicate=store.add({email:'a'}),last=store.add({email:'b'});
      await completed;
      const check=db.transaction('s','readwrite');let inactive;
      const timer=new Promise(resolve=>setTimeout(()=>{
        try{check.objectStore('s').put('late',1);inactive=false;}catch(e){inactive=e.name==='TransactionInactiveError';}resolve();
      },0));await timer;db.close();
      return [first.result===1,duplicate.error.name==='ConstraintError',last.result===2,
        events.length===1&&events[0][0]==='ConstraintError'&&events[0][1]&&events[0][2]===3,inactive];
    }"#,None,&[],true,true).await.unwrap();
    assert!(!result.thrown,"{}",result.description);
    assert_eq!(result.value,Some(serde_json::json!([true,true,true,true,true])));
}

#[tokio::test(flavor="current_thread")]
async fn indexed_db_transaction_does_not_remain_active_across_native_script_tasks() {
    let mut rt=runtime();
    let result=rt.call_function_on_for_cdp(r#"async()=>{
      const open=indexedDB.open('native-tasks');open.onupgradeneeded=()=>open.result.createObjectStore('s');
      globalThis.taskDb=await new Promise(resolve=>open.onsuccess=()=>resolve(open.result));return true;
    }"#,None,&[],true,true).await.unwrap();assert_eq!(result.value,Some(serde_json::json!(true)));
    rt.execute_script("create-tx","globalThis.taskTx=taskDb.transaction('s','readwrite');").unwrap();
    assert_eq!(rt.evaluate("(()=>{try{taskTx.objectStore('s').put('late',1);return false;}catch(e){return e.name==='TransactionInactiveError';}})()").unwrap(),serde_json::json!(true));
}

#[tokio::test(flavor="current_thread")]
async fn indexed_db_synchronous_versionchange_close_does_not_report_blocked_for_open_or_delete() {
    let mut rt=runtime();
    let result=rt.call_function_on_for_cdp(r#"async()=>{
      const request=req=>new Promise((resolve,reject)=>{req.onsuccess=()=>resolve(req.result);req.onerror=()=>reject(req.error);});
      const first=indexedDB.open('notice',1);first.onupgradeneeded=()=>first.result.createObjectStore('s');const db=await request(first),trace=[];
      db.onversionchange=()=>{trace.push('versionchange');db.close();};
      const upgrade=indexedDB.open('notice',2);upgrade.onblocked=()=>trace.push('blocked');upgrade.onupgradeneeded=()=>trace.push('upgrade');
      const next=await request(upgrade);trace.push('success');
      const deleted=[];next.onversionchange=()=>{deleted.push('versionchange');next.close();};
      const deletion=indexedDB.deleteDatabase('notice');deletion.onblocked=()=>deleted.push('blocked');await request(deletion);deleted.push('success');
      return [trace,deleted];
    }"#,None,&[],true,true).await.unwrap();
    assert!(!result.thrown,"{}",result.description);
    assert_eq!(result.value,Some(serde_json::json!([["versionchange","upgrade","success"],["versionchange","success"]])));
}

#[tokio::test(flavor="current_thread")]
async fn indexed_db_cursor_identity_live_records_key_targets_and_pending_guards() {
    let mut rt=runtime();
    let result=rt.call_function_on_for_cdp(r#"async()=>{
  const request=req=>new Promise((resolve,reject)=>{req.onsuccess=()=>resolve(req.result);req.onerror=()=>reject(req.error);});
  const open=indexedDB.open('cursor-control');open.onupgradeneeded=()=>{
    const s=open.result.createObjectStore('s');s.createIndex('group','group');
    for(let i=1;i<=5;i++)s.put({group:i<4?'a':'b'},i);
  };const db=await request(open),checks=[];
  const tx=db.transaction('s','readwrite'),store=tx.objectStore('s'),req=store.openCursor(),seen=[];let first;
  await new Promise((resolve,reject)=>{
    req.onerror=()=>reject(req.error);req.onsuccess=()=>{
      const c=req.result;if(!c){resolve();return;}seen.push(c.key);
      if(!first){first=c;
        for(const f of [()=>c.advance(0),()=>c.advance(-1)]){try{f();checks.push(false);}catch(e){checks.push(e.name==='TypeError');}}
        for(const key of [NaN,{},[],1]){if(Array.isArray(key))key.push(key);try{c.continue(key);checks.push(false);}catch(e){checks.push(e.name==='DataError');}}
        store.delete(2);store.put({group:'a'},2.5);c.continue();
        try{c.continue();checks.push(false);}catch(e){checks.push(e.name==='InvalidStateError');}
        checks.push(c.key===1&&c.request===req);
      }else if(c.key===2.5){checks.push(c===first);c.continue(4);}
      else{checks.push(c===first);c.advance(2);}
    };
  });checks.push(JSON.stringify(seen)==='[1,2.5,4]');
  const indexReq=db.transaction('s').objectStore('s').index('group').openCursor();const tuples=[];let indexCursor;
  await new Promise((resolve,reject)=>{indexReq.onerror=()=>reject(indexReq.error);indexReq.onsuccess=()=>{
    const c=indexReq.result;if(!c){resolve();return;}tuples.push([c.key,c.primaryKey]);
    if(!indexCursor){indexCursor=c;c.continuePrimaryKey('a',3);}else{checks.push(c===indexCursor);c.advance(2);}
  };});checks.push(JSON.stringify(tuples)==='[["a",1],["a",3],["b",5]]');
  let illegal=false;try{IDBCursor.prototype.continue.call({});}catch(e){illegal=e.name==='TypeError';}checks.push(illegal);
  db.close();return {checks,seen,tuples};
}
"#,None,&[],true,true).await.unwrap();
    assert!(!result.thrown,"{}",result.description);
    let value=result.value.unwrap();
    assert!(value["checks"].as_array().unwrap().iter().all(|v|v==true),"{value}");
}

#[tokio::test(flavor="current_thread")]
async fn indexed_db_cursor_range_snapshot_key_only_write_sources_and_private_bounds() {
    let mut rt=runtime();
    let result=rt.call_function_on_for_cdp(r#"async()=>{
  const request=req=>new Promise((resolve,reject)=>{req.onsuccess=()=>resolve(req.result);req.onerror=()=>reject(req.error);});
  const open=indexedDB.open('cursor-edges');open.onupgradeneeded=()=>{
    const store=open.result.createObjectStore('s');store.createIndex('group','group');
    store.put({group:'a'},[1]);store.put({group:'a'},[2]);store.put({group:'a'},[2.5]);store.put({group:'b'},[3]);
  };const db=await request(open),checks=[];
  const tx=db.transaction('s','readwrite'),store=tx.objectStore('s'),query=[1],r=store.openCursor(query);let first;
  await new Promise((resolve,reject)=>{r.onerror=()=>reject(r.error);r.onsuccess=()=>{
    const c=r.result;if(!first){first=c;query[0]=2;
      checks.push(c.update({group:'a'}).source===c);c.continue();
    }else{checks.push(c===null,JSON.stringify(first.key)==='[1]',JSON.stringify(first.primaryKey)==='[1]',first.value===undefined);resolve();}
  };});
  const keyTx=db.transaction('s','readwrite'),keyReq=keyTx.objectStore('s').openKeyCursor();
  await new Promise(resolve=>{keyReq.onsuccess=()=>{const c=keyReq.result;
    for(const f of [()=>c.delete(),()=>c.update({})])try{f();checks.push(false);}catch(e){checks.push(e.name==='InvalidStateError');}resolve();
  };});
  const writeReq=db.transaction('s','readwrite').objectStore('s').openCursor();
  await new Promise(resolve=>{writeReq.onsuccess=()=>{const c=writeReq.result;checks.push(c.delete().source===c);resolve();};});
  const prev=db.transaction('s').objectStore('s').index('group').openCursor(null,'prevunique'),rows=[];
  await new Promise(resolve=>{prev.onsuccess=()=>{const c=prev.result;if(!c){resolve();return;}rows.push([c.key,c.primaryKey]);c.continue();};});
  checks.push(JSON.stringify(rows)==='[["b",[3]],["a",[2]]]');
  const boundary=[2],range=IDBKeyRange.only(boundary);boundary[0]=3;range.lower[0]=3;range.includes=()=>true;
  const rangeReq=db.transaction('s').objectStore('s').openCursor(range),rangeKeys=[];
  await new Promise(resolve=>{rangeReq.onsuccess=()=>{const c=rangeReq.result;if(!c){resolve();return;}rangeKeys.push(c.key);c.continue();};});
  checks.push(JSON.stringify(rangeKeys)==='[[2]]');db.close();return {checks,rows,rangeKeys};
}
"#,None,&[],true,true).await.unwrap();
    assert!(!result.thrown,"{}",result.description);
    let value=result.value.unwrap();
    assert!(value["checks"].as_array().unwrap().iter().all(|v|v==true),"{value}");
}

#[tokio::test(flavor="current_thread")]
async fn loaded_frame_ready_timers_keep_the_first_events_microtask_transaction_active() {
    let mut rt=runtime();
    let child=crate::frame::FrameRealm::new(&mut rt,92,0,"https://example.com/timers",
        "<html><body></body></html>").unwrap();
    child.execute_script(&mut rt,r#"
      const open=indexedDB.open('frame-timers');open.onupgradeneeded=()=>open.result.createObjectStore('s');
      open.onsuccess=()=>{globalThis.frameTimerDb=open.result;};
    "#).unwrap();
    tokio::time::timeout(std::time::Duration::from_secs(1),rt.run_event_loop()).await.expect("idle IDB watchers must be unreferenced in the loaded realm").unwrap();
    assert_eq!(child.evaluate(&mut rt,"!!globalThis.frameTimerDb").unwrap(),serde_json::json!(true));
    child.execute_script(&mut rt,r#"
      globalThis.frameTimerTrace=[];
      let cancelReady;
      setTimeout(()=>{
        clearTimeout(cancelReady);
        const store=frameTimerDb.transaction('s','readwrite').objectStore('s');
        Promise.resolve().then(()=>{try{store.put('first task',1);frameTimerTrace.push('microtask');}
          catch(error){frameTimerTrace.push(error.name);}});
      },0);
      setTimeout(()=>frameTimerTrace.push('second'),0);
      cancelReady=setTimeout(()=>frameTimerTrace.push('cancelled timer fired'),0);
    "#).unwrap();
    // Both native sleep completions are ready before V8 is pumped. No host
    // script evaluation between these event deliveries changes their epoch.
    tokio::time::sleep(std::time::Duration::from_millis(5)).await;
    tokio::time::timeout(std::time::Duration::from_secs(1),rt.run_event_loop()).await.expect("idle IDB watchers must be unreferenced in the loaded realm").unwrap();
    assert_eq!(child.evaluate(&mut rt,"frameTimerTrace").unwrap(),serde_json::json!(["microtask","second"]));
    drop(child);
}

#[test]
fn popover_native_state_agrees_with_selectors_and_ignores_author_expandos() {
    let mut rt = runtime();
    assert_eq!(rt.evaluate(r#"(() => {
        const container=document.createElement('div');
        const p=document.createElement('div');p.popover='manual';container.append(p);document.body.append(container);
        const state=()=>[p.matches(':popover-open'),document.querySelectorAll(':popover-open').length,
          p.matches(':not(:popover-open)'),container.querySelector(':is(:popover-open,.absent)')===p];
        const seen=[state()];p._popoverState='showing';seen.push(state());
        const cancel=e=>e.preventDefault();p.addEventListener('beforetoggle',cancel);
        p.showPopover();seen.push(state());p.removeEventListener('beforetoggle',cancel);
        p.showPopover();seen.push(state());p._popoverState='hidden';seen.push(state());
        p.hidePopover();seen.push(state());p.showPopover();container.remove();document.body.append(container);seen.push(state());
        return seen;
    })()"#).unwrap(),serde_json::json!([
        [false,0,true,false],[false,0,true,false],[false,0,true,false],
        [true,1,false,true],[true,1,false,true],[false,0,true,false],[false,0,true,false]
    ]));
}

#[cfg(feature="render")]
#[test]
fn popover_show_hide_invalidate_prepared_geometry_and_preserve_author_display() {
    let mut rt = runtime();
    assert_eq!(rt.evaluate(r#"(() => {
        document.body.style.margin='0';
        const p=document.createElement('div');p.popover='manual';p.textContent='Menu';
        p.style.cssText='width:80px;height:40px;padding:0;border:0';document.body.append(p);
        const next=document.createElement('div');next.textContent='Next';document.body.append(next);
        const states=[];const take=()=>states.push([getComputedStyle(p).display,p.getBoundingClientRect().width,next.getBoundingClientRect().top]);
        take();p.showPopover();take();p.hidePopover();take();p.style.display='block';take();
        p.style.display='';p.showPopover();p.remove();document.body.append(p);take();return states;
    })()"#).unwrap(),serde_json::json!([
        ["none",0,0],["block",80,0],["none",0,0],["block",80,0],["none",0,0]
    ]));
}

#[test]
fn popover_private_receivers_reject_spoofs_and_ignore_replaced_helpers() {
    let mut rt=runtime();
    assert_eq!(rt.evaluate(r#"(() => {
      const p=document.createElement('div');p.popover='manual';document.body.append(p);
      const cancel=e=>e.preventDefault();p.addEventListener('beforetoggle',cancel);
      const fake={_nid:p._nid,_checkPopoverValidity:()=>true,dispatchEvent:()=>true};
      const constructorCopy=new HTMLElement(p._nid);
      let constructorRejected=false;try{HTMLElement.prototype.showPopover.call(constructorCopy)}catch(e){constructorRejected=e instanceof TypeError}
      const rejects=[];for(const method of ['showPopover','hidePopover','togglePopover']) {
        try{HTMLElement.prototype[method].call(fake);rejects.push(false)}catch(e){rejects.push(e instanceof TypeError)}
      }
      p._checkPopoverValidity=()=>true;p.dispatchEvent=()=>true;p._popoverState='showing';
      HTMLElement.prototype.showPopover.call(p);const canceled=!p.matches(':popover-open');
      p.removeEventListener('beforetoggle',cancel);
      const originalId=p._nid;p._nid=-1;HTMLElement.prototype.showPopover.call(p);p._nid=originalId;
      const opened=p.matches(':popover-open');p._nid=-1;HTMLElement.prototype.hidePopover.call(p);p._nid=originalId;
      return [rejects,constructorRejected,canceled,opened,!p.matches(':popover-open')];
    })()"#).unwrap(),serde_json::json!([[true,true,true],true,true,true,true]));
}

#[test]
fn popover_borrowed_methods_use_original_frame_and_retired_document() {
    use crate::frame::FrameRealm;
    let mut rt=runtime();
    let html="<html><body><div id=p popover=manual style='width:80px;height:40px;padding:0;border:0'>Menu</div></body></html>";
    rt.set_dom(obscura_dom::parse_html(html));rt.run_page_init();
    let frame=FrameRealm::new(&mut rt,1,0,"https://example.com/frame",html).unwrap();
    rt.execute_script("popover-borrow",r#"
      globalThis.parentPopover=document.getElementById('p');
      globalThis.childPopover=__obscura_frameObjects[1].document.getElementById('p');
      globalThis.popoverEvents=[];
      parentPopover.addEventListener('beforetoggle',e=>popoverEvents.push(e.target===parentPopover?'parent':'wrong'));
      childPopover.addEventListener('beforetoggle',e=>popoverEvents.push(e.target===childPopover?'child':'wrong'));
    "#).unwrap();
    assert_eq!(rt.evaluate(r#"(() => {
      const child=__obscura_frameObjects[1];
      const sameId=parentPopover._nid===childPopover._nid;
      HTMLElement.prototype.showPopover.call(childPopover);
      const first=[parentPopover.matches(':popover-open'),childPopover.matches(':popover-open')];
      child.window.HTMLElement.prototype.showPopover.call(parentPopover);
      const second=[parentPopover.matches(':popover-open'),childPopover.matches(':popover-open')];
      child.window.HTMLElement.prototype.hidePopover.call(parentPopover);
      HTMLElement.prototype.hidePopover.call(childPopover);
      return [sameId,first,second,popoverEvents,
        parentPopover.matches(':popover-open'),childPopover.matches(':popover-open')];
    })()"#).unwrap(),serde_json::json!([true,[false,true],[true,true],["child","parent","parent","child"],false,false]));
    rt.execute_script("save-old-popover","globalThis.savedPopover=parentPopover").unwrap();
    rt.set_dom(obscura_dom::parse_html(html));rt.run_page_init();
    assert_eq!(rt.evaluate(r#"(() => {
      const fresh=document.getElementById('p');let rejected=false;
      try{HTMLElement.prototype.showPopover.call(savedPopover)}catch(e){rejected=e.name==='InvalidStateError'}
      return [savedPopover._nid===fresh._nid,rejected,fresh.matches(':popover-open')];
    })()"#).unwrap(),serde_json::json!([true,true,false]));
    drop(frame);
}

#[test]
fn popover_cancellation_keeps_original_listener_identity_under_nid_accessors() {
    let mut rt=runtime();
    assert_eq!(rt.evaluate(r#"(() => {
      const p=document.createElement('div');p.id='actual-popover';p.popover='manual';document.body.append(p);
      let calls=0;const cancel=e=>{calls++;e.preventDefault()};p.addEventListener('beforetoggle',cancel);
      const id=p._nid;const other=document.createElement('div');p._nid=other._nid;
      p._resolveInlineHandler=()=>{throw Error('public helper')};p.showPopover();
      Object.defineProperty(p,'_nid',{configurable:true,get(){throw Error('public nid')}});p.showPopover();
      const closed=document.querySelector('#actual-popover:popover-open')===null;
      Object.defineProperty(p,'_nid',{configurable:true,writable:true,value:id});p.removeEventListener('beforetoggle',cancel);
      p.onbeforetoggle=cancel;p.showPopover();const idlClosed=document.querySelector('#actual-popover:popover-open')===null;
      p.onbeforetoggle=null;p.setAttribute('onbeforetoggle','event.preventDefault()');
      Object.defineProperty(p,'_nid',{configurable:true,get(){throw Error('public nid')}});p.showPopover();
      const inlineClosed=document.querySelector('#actual-popover:popover-open')===null;
      return [calls,closed,idlClosed,inlineClosed];
    })()"#).unwrap(),serde_json::json!([3,true,true,true]));
}

#[test]
fn popover_internal_events_ignore_replaced_global_constructor() {
    let mut rt=runtime();
    assert_eq!(rt.evaluate(r#"(() => {
      const p=document.createElement('div');p.popover='manual';document.body.append(p);
      const original=ToggleEvent;let actual=false;
      p.addEventListener('beforetoggle',e=>{actual=e instanceof original && e.oldState==='closed' && e.newState==='open';e.preventDefault()});
      Object.defineProperty(globalThis,'ToggleEvent',{configurable:true,get(){throw Error('public constructor')}});
      p.showPopover();return [actual,!p.matches(':popover-open')];
    })()"#).unwrap(),serde_json::json!([true,true]));
}

#[cfg(feature="render")]
#[test]
fn popover_hidden_dialogs_keep_hidden_even_when_native_state_is_open() {
    let mut rt=runtime();
    assert_eq!(rt.evaluate(r#"(() => {
      document.body.innerHTML='<dialog id=a popover=manual open hidden>Hidden dialog</dialog><dialog id=b popover=manual hidden>Hidden popover</dialog><div id=c popover=manual hidden>Hidden div</div>';
      const result=[];for(const id of ['a','b','c']) {
        const e=document.getElementById(id);const state=()=>[e.matches(':popover-open'),getComputedStyle(e).display,e.getBoundingClientRect().width,e.getBoundingClientRect().height];
        result.push(state());e.showPopover();result.push(state());
      }return result;
    })()"#).unwrap(),serde_json::json!([
      [false,"none",0,0],[true,"none",0,0],[false,"none",0,0],[true,"none",0,0],[false,"none",0,0],[true,"none",0,0]
    ]));
}

#[test]
fn popover_foreign_attribute_writes_are_ordinary_and_stale_caps_do_not_mint() {
    let mut rt=runtime();
    assert_eq!(rt.evaluate(r#"(() => {
      const svg=document.createElementNS('http://www.w3.org/2000/svg','svg');
      svg.setAttribute('popover','manual');const written=svg.getAttribute('popover');
      svg.removeAttribute('popover');return [written,svg.getAttribute('popover')];
    })()"#).unwrap(),serde_json::json!(["manual",null]));
    rt.set_dom(obscura_dom::parse_html("<html><body><span></span><div id=p popover=manual>New</div></body></html>"));
    // A native host DOM replacement alone does not install the new page cap.
    assert_eq!(rt.evaluate("(() => {globalThis.pendingPopover=document.getElementById('p');return pendingPopover.textContent})()").unwrap(),serde_json::json!("New"));
    rt.run_page_init();
    assert_eq!(rt.evaluate("(() => {const p=document.getElementById('p');let rejected=false;try{pendingPopover.showPopover()}catch(e){rejected=e instanceof TypeError}p.showPopover();return [p!==pendingPopover,rejected,p.matches(':popover-open')]})()").unwrap(),serde_json::json!([true,true,true]));
}

#[test]
fn popover_document_handoff_evicts_retired_cache_without_rebinding_saved_receivers() {
    let mut rt=runtime();
    rt.set_dom(obscura_dom::parse_html("<html><body><div id=p popover=manual>Old</div></body></html>"));
    rt.run_page_init();
    assert_eq!(rt.evaluate("(() => {globalThis.oldPopover=document.getElementById('p');return oldPopover.textContent})()").unwrap(),serde_json::json!("Old"));
    rt.set_dom(obscura_dom::parse_html("<html><body><div id=p popover=manual>New</div></body></html>"));
    rt.run_page_init();
    assert_eq!(rt.evaluate(r#"(() => {
      const p=document.getElementById('p');let retired=false;
      try{oldPopover.showPopover()}catch(e){retired=e.name==='InvalidStateError'}
      const untouched=!p.matches(':popover-open');p.showPopover();
      return [p!==oldPopover,retired,untouched,p.matches(':popover-open'),p.textContent];
    })()"#).unwrap(),serde_json::json!([true,true,true,true,"New"]));
}

#[test]
fn popover_document_handoff_replaces_colliding_svg_cache_with_html_factory() {
    let mut rt=runtime();
    assert_eq!(rt.evaluate("(() => {globalThis.oldSvg=document.createElementNS('http://www.w3.org/2000/svg','svg');return oldSvg instanceof SVGElement})()").unwrap(),serde_json::json!(true));
    rt.set_dom(obscura_dom::parse_html("<html><body><div id=p popover=manual>New</div></body></html>"));
    rt.run_page_init();
    assert_eq!(rt.evaluate("(() => {const p=document.getElementById('p');p.showPopover();return [p!==oldSvg,p instanceof HTMLElement,p.matches(':popover-open')]})()").unwrap(),serde_json::json!([true,true,true]));
}


#[test]
fn popover_html_interface_reflection_and_receiver_brands_match_chrome() {
    let mut rt = runtime();
    assert_eq!(rt.evaluate(r#"(() => {
      const p=document.createElement('div'),svg=document.createElementNS('http://www.w3.org/2000/svg','svg');
      const d=Object.getOwnPropertyDescriptor(HTMLElement.prototype,'popover');
      const values=[];for(const value of [null,undefined,'','AUTO','hint','invalid',3]) {
        p.popover=value;values.push([p.getAttribute('popover'),p.popover]);
      }
      const rejects=[];for(const receiver of [{},svg,new Proxy(p,{})]) {
        for(const call of [()=>d.get.call(receiver),()=>d.set.call(receiver,'manual')]) {
          try{call();rejects.push(false)}catch(e){rejects.push(e instanceof TypeError)}
        }
      }
      let symbol=false;try{p.popover=Symbol()}catch(e){symbol=e instanceof TypeError}
      return [Object.hasOwn(HTMLElement.prototype,'popover'),Object.hasOwn(Element.prototype,'popover'),
        'popover' in svg,['showPopover','hidePopover','togglePopover'].map(k=>Object.hasOwn(HTMLElement.prototype,k)),
        [d.enumerable,d.configurable,typeof d.get,typeof d.set],values,rejects,symbol];
    })()"#).unwrap(),serde_json::json!([true,false,false,[true,true,true],[true,true,"function","function"],
      [[null,null],[null,null],["","auto"],["AUTO","auto"],["hint","hint"],["invalid","manual"],["3","manual"]],
      [true,true,true,true,true,true],true]));
}

#[test]
fn popover_reflection_borrowed_accessors_keep_original_document_under_nid_mutation() {
    let mut rt = runtime();
    rt.set_dom(obscura_dom::parse_html("<html><body><div id=p popover=manual></div></body></html>"));
    rt.set_url("http://example.com/root");rt.run_page_init();
    let child=crate::frame::FrameRealm::new(&mut rt,81,0,"http://example.com/child",
      "<html><body><div id=p popover=hint></div></body></html>").unwrap();
    rt.execute_script("save-reflection",r#"
      globalThis.savedPopoverReflection=document.getElementById('p');
      globalThis.childPopoverReflection=__obscura_frameObjects[81].document.getElementById('p');
      globalThis.childPopoverAccessor=Object.getOwnPropertyDescriptor(__obscura_frameObjects[81].window.HTMLElement.prototype,'popover');
    "#).unwrap();
    assert_eq!(rt.evaluate(r#"(() => {
      const p=savedPopoverReflection,id=p._nid;p._nid=0;
      childPopoverAccessor.set.call(p,'AUTO');const reflected=childPopoverAccessor.get.call(p);p._nid=id;
      return [reflected,p.getAttribute('popover'),childPopoverReflection.popover];
    })()"#).unwrap(),serde_json::json!(["auto","AUTO","hint"]));
    drop(child);
    rt.set_dom(obscura_dom::parse_html("<html><body><div id=p popover=manual>New</div></body></html>"));
    rt.set_url("http://example.com/new");rt.run_page_init();
    assert_eq!(rt.evaluate(r#"(() => {
      const d=Object.getOwnPropertyDescriptor(HTMLElement.prototype,'popover');
      d.set.call(savedPopoverReflection,'hint');d.set.call(childPopoverReflection,'invalid');
      return [d.get.call(savedPopoverReflection),d.get.call(childPopoverReflection),document.getElementById('p').popover];
    })()"#).unwrap(),serde_json::json!(["hint","manual","manual"]));
}


#[cfg(feature = "render")]
#[test]
fn dialog_open_changes_invalidate_geometry_and_preserve_author_display() {
    let mut rt = runtime();
    assert_eq!(rt.evaluate(r#"(() => {
      document.body.innerHTML='<dialog id=p style="width:80px;height:40px;padding:0;border:0">Dialog</dialog><div id=next>Next</div>';
      document.body.style.margin='0';const p=document.getElementById('p');
      const take=()=>[getComputedStyle(p).display,getComputedStyle(p).position,p.getBoundingClientRect().width,document.getElementById('next').getBoundingClientRect().top];
      const states=[take()];p.open=true;states.push(take());p.open=false;states.push(take());
      p.style.display='block';states.push(take());return states;
    })()"#).unwrap(),serde_json::json!([
      ["none","absolute",0,0],["block","absolute",80,0],["none","absolute",0,0],["block","absolute",80,0]
    ]));
}


#[test]
fn dialog_open_reflector_rejects_spoofs_and_keeps_original_attribute_identity() {
    let mut rt = runtime();
    assert_eq!(rt.evaluate(r#"(() => {
      const p=document.createElement('dialog'),div=document.createElement('div');document.body.append(p,div);
      const d=Object.getOwnPropertyDescriptor(HTMLDialogElement.prototype,'open');
      const rejects=[];for(const receiver of [div,{},new Proxy(p,{})]) {
        for(const call of [()=>d.get.call(receiver),()=>d.set.call(receiver,true)]) {
          try{call();rejects.push(false)}catch(e){rejects.push(e instanceof TypeError)}
        }
      }
      const states=[];const take=()=>states.push([p.open,p.getAttribute('open'),p.hasAttribute('open')]);
      take();p.open=true;take();p.setAttribute('open','authored');take();p.open=false;take();
      const nid=p._nid;p._nid=div._nid;d.set.call(p,true);const privateRead=d.get.call(p);p._nid=nid;
      return [rejects,[d.enumerable,d.configurable],states,privateRead,p.getAttribute('open'),div.hasAttribute('open')];
    })()"#).unwrap(),serde_json::json!([[true,true,true,true,true,true],[true,true],
      [[false,null,false],[true,"",true],[true,"authored",true],[false,null,false]],true,"",false]));
}

#[test]
fn html_open_reflectors_have_separate_brands_and_no_element_exposure() {
    let mut rt = runtime();
    assert_eq!(rt.evaluate(r#"(() => {
      const details=document.createElement('details'),dialog=document.createElement('dialog');
      const div=document.createElement('div'),svg=document.createElementNS('http://www.w3.org/2000/svg','g');
      const own=type=>Object.getOwnPropertyDescriptor(type.prototype,'open');
      const exposure=[!!own(Element),!!own(HTMLElement),!!own(HTMLDetailsElement),!!own(HTMLDialogElement),'open' in div,'open' in svg];
      const descriptors=[],rejects=[],states=[];
      for(const [type,element,other] of [[HTMLDetailsElement,details,dialog],[HTMLDialogElement,dialog,details]]) {
        const d=own(type);descriptors.push([d.enumerable,d.configurable]);
        for(const receiver of [other,div,svg,{},new Proxy(element,{}),Object.create(type.prototype)]) {
          for(const call of [()=>d.get.call(receiver),()=>d.set.call(receiver,true)]) {
            try{call();rejects.push(false)}catch(e){rejects.push(e instanceof TypeError)}
          }
        }
        const take=()=>[d.get.call(element),element.getAttribute('open')];
        const row=[take()];element.open=Symbol('truthy');row.push(take());
        element.setAttribute('OPEN','authored');row.push(take());element.open=0;row.push(take());
        element.setAttribute('open','');element.removeAttribute('open');row.push(take());states.push(row);
      }
      return [exposure,descriptors,rejects.length===24&&rejects.every(Boolean),states];
    })()"#).unwrap(),serde_json::json!([[false,false,true,true,false,false],[[true,true],[true,true]],true,
      [[[false,null],[true,""],[true,"authored"],[false,null],[false,null]],
       [[false,null],[true,""],[true,"authored"],[false,null],[false,null]]]]));
}

#[test]
fn details_open_borrowed_realm_keeps_original_document_and_node_generation() {
    let mut rt=runtime();
    rt.set_dom(obscura_dom::parse_html("<html><body><details id=d></details><div id=x></div></body></html>"));
    rt.set_url("http://example.com/root");rt.run_page_init();
    let child=crate::frame::FrameRealm::new(&mut rt,82,0,"http://example.com/child",
      "<html><body><details id=d open=child></details></body></html>").unwrap();
    rt.execute_script("save-details",r#"
      globalThis.savedDetails=document.getElementById('d');
      globalThis.childDetails=__obscura_frameObjects[82].document.getElementById('d');
      globalThis.childDetailsAccessor=Object.getOwnPropertyDescriptor(__obscura_frameObjects[82].window.HTMLDetailsElement.prototype,'open');
    "#).unwrap();
    assert_eq!(rt.evaluate(r#"(() => {
      const d=savedDetails,nid=d._nid;d._nid=document.getElementById('x')._nid;
      childDetailsAccessor.set.call(d,true);const read=childDetailsAccessor.get.call(d);d._nid=nid;
      return [read,d.getAttribute('open'),document.getElementById('x').hasAttribute('open'),childDetails.getAttribute('open')];
    })()"#).unwrap(),serde_json::json!([true,"",false,"child"]));
    drop(child);
    rt.set_dom(obscura_dom::parse_html("<html><body><details id=d open=new></details></body></html>"));
    rt.set_url("http://example.com/new");rt.run_page_init();
    assert_eq!(rt.evaluate(r#"(() => {
      const d=Object.getOwnPropertyDescriptor(HTMLDetailsElement.prototype,'open');
      d.set.call(savedDetails,false);d.set.call(childDetails,false);
      return [d.get.call(savedDetails),d.get.call(childDetails),savedDetails.getAttribute('open'),childDetails.getAttribute('open'),document.getElementById('d').getAttribute('open')];
    })()"#).unwrap(),serde_json::json!([false,false,null,null,"new"]));
}

#[cfg(feature = "render")]
#[test]
fn details_open_updates_prepared_child_geometry() {
    let mut rt=runtime();
    assert_eq!(rt.evaluate(r#"(() => {
      document.body.innerHTML='<details id=d style="width:100px;padding:0;border:0"><summary style="height:20px">Summary</summary><div id=c style="height:40px;width:80px">Content</div></details>';
      const d=document.getElementById('d'),c=document.getElementById('c');
      const take=()=>[d.getBoundingClientRect().height,c.getBoundingClientRect().width];
      const states=[take()];d.open=true;states.push(take());d.open=false;states.push(take());return states;
    })()"#).unwrap(),serde_json::json!([[20,0],[60,80],[20,0]]));
}

#[test]
fn html_open_accessors_are_real_native_callbacks_with_chrome_metadata() {
    let mut rt=runtime();
    assert_eq!(rt.evaluate(r#"(() => {
      const ds=[HTMLDetailsElement,HTMLDialogElement].map(type=>Object.getOwnPropertyDescriptor(type.prototype,'open'));
      const fs=ds.flatMap(d=>[d.get,d.set]);
      return [fs.map(f=>f.name),fs.map(f=>f.length),fs.map(f=>Object.hasOwn(f,'prototype')),
        fs.map(f=>{try{Reflect.construct(f,[]);return false}catch(e){return e instanceof TypeError}}),
        fs.map(f=>{const d=Object.getOwnPropertyDescriptor(f,'name');return [d.writable,d.enumerable,d.configurable]}),
        fs.map(f=>{const d=Object.getOwnPropertyDescriptor(f,'length');return [d.writable,d.enumerable,d.configurable]}),
        !Object.hasOwn(globalThis,'__obscura_open_bindings_handoff')];
    })()"#).unwrap(),serde_json::json!([
      ["get open","set open","get open","set open"],[0,1,0,1],[false,false,false,false],[true,true,true,true],
      [[false,false,true],[false,false,true],[false,false,true],[false,false,true]],
      [[false,false,true],[false,false,true],[false,false,true],[false,false,true]],true]));
    // Query V8's native source directly. A public toString replacement or
    // a name-only JS wrapper cannot make these callbacks pass this check.
    use deno_core::v8;
    let mut entered=rt.runtime();
    let context=entered.main_context();
    let scope=&mut v8::HandleScope::with_context(entered.v8_isolate(),context);
    let code=v8::String::new(scope,r#"(() => {
      const fs=[HTMLDetailsElement,HTMLDialogElement].flatMap(type=>{
        const d=Object.getOwnPropertyDescriptor(type.prototype,'open');return [d.get,d.set];
      });return [...fs,function javascriptControl(){}];
    })()"#).unwrap();
    let values=v8::Script::compile(scope,code,None).unwrap().run(scope).unwrap();
    let values=v8::Local::<v8::Array>::try_from(values).unwrap();
    for index in 0..4 {
        let value=values.get_index(scope,index).unwrap();
        let function=v8::Local::<v8::Function>::try_from(value).unwrap();
        assert_eq!(function.script_id(),0,"open must be a native callback");
        let source=value.to_detail_string(scope).unwrap().to_rust_string_lossy(scope);
        assert!(source.contains("[native code]"),"actual V8 source: {source}");
    }
    let control=values.get_index(scope,4).unwrap();
    let control=v8::Local::<v8::Function>::try_from(control).unwrap();
    assert_ne!(control.script_id(),0);
}

#[test]
fn html_open_native_installation_preserves_author_changes_and_child_function_realm() {
    let mut rt=runtime();
    rt.execute_script("retain-native-open",r#"
      globalThis.originalDetails=document.createElement('details');
      globalThis.originalDetailsOpen=Object.getOwnPropertyDescriptor(HTMLDetailsElement.prototype,'open');
      Object.defineProperty(HTMLDetailsElement.prototype,'open',{configurable:true,enumerable:true,get(){return 'author'},set(){throw Error('author setter')}});
    "#).unwrap();
    rt.run_page_init();
    assert_eq!(rt.evaluate(r#"(() => {
      originalDetailsOpen.set.call(originalDetails,true);
      return [originalDetails.open,originalDetailsOpen.get.call(originalDetails),originalDetails.getAttribute('open')];
    })()"#).unwrap(),serde_json::json!(["author",true,""]));
    let child=crate::frame::FrameRealm::new(&mut rt,83,0,"https://example.com/child",
      "<html><body><details id=d></details><dialog id=g></dialog></body></html>").unwrap();
    assert_eq!(rt.evaluate(r#"(() => {
      const win=__obscura_frameObjects[83].window,d=win.document.getElementById('d'),g=win.document.getElementById('g');
      const descriptor=Object.getOwnPropertyDescriptor(win.HTMLDetailsElement.prototype,'open');
      originalDetailsOpen.set.call(d,true);descriptor.set.call(originalDetails,false);
      const rejects=[];for(const value of [null,undefined,0,'x',Symbol(),g,{},new Proxy(d,{})]) {
        try{descriptor.get.call(value);rejects.push(false)}catch(e){rejects.push(e instanceof win.TypeError)}
      }
      return [d.open,originalDetailsOpen.get.call(originalDetails),Object.getPrototypeOf(descriptor.get)===win.Function.prototype,
        descriptor.get.name,descriptor.set.name,rejects.every(Boolean),!Object.hasOwn(win,'__obscura_open_bindings_handoff')];
    })()"#).unwrap(),serde_json::json!([true,false,true,"get open","set open",true,true]));
    drop(child);
    rt.set_dom(obscura_dom::parse_html("<html><body></body></html>"));rt.run_page_init();
    assert_eq!(rt.evaluate("originalDetails.open").unwrap(),serde_json::json!("author"));
}

#[test]
fn html_open_cached_brand_cannot_access_a_recycled_native_node() {
    let mut rt=runtime();
    for (tag, replacement_tag) in [("details", "dialog"), ("dialog", "details"), ("details", "details"), ("dialog", "dialog")] {
        let dom=obscura_dom::parse_html(&format!("<html><body><{tag} id=old open=original></{tag}><{replacement_tag} id=seed open=replacement></{replacement_tag}></body></html>"));
        let old=dom.get_element_by_id("old").unwrap();
        let generation=dom.node_generation(old).unwrap();
        let seed=dom.get_node(dom.get_element_by_id("seed").unwrap()).unwrap().data;
        rt.set_dom(dom);rt.run_page_init();
        assert_eq!(rt.evaluate("(() => {globalThis.savedOpenElement=document.getElementById('old');return savedOpenElement.open;})()").unwrap(),serde_json::json!(true));
        let replacement={
            let state=rt.state.borrow();let dom=state.dom.as_ref().unwrap();
            dom.remove(old);let replacement=dom.new_node(seed);
            assert_eq!(replacement,old);
            assert_ne!(dom.node_generation(replacement),Some(generation));
            replacement
        };
        assert_eq!(rt.evaluate(r#"(() => {
          const descriptor=Object.getOwnPropertyDescriptor(Object.getPrototypeOf(savedOpenElement),'open');
          const rejected=[];for(const call of [()=>descriptor.get.call(savedOpenElement),()=>descriptor.set.call(savedOpenElement,false)]) {
            try{call();rejected.push(false)}catch(e){rejected.push(e instanceof TypeError)}
          }return rejected;
        })()"#).unwrap(),serde_json::json!([true,true]));
        let state=rt.state.borrow();
        assert_eq!(state.dom.as_ref().unwrap().get_node(replacement).unwrap().get_attribute("open"),Some("replacement"));
    }
}

#[test]
fn html_open_stale_borrowed_getters_throw_in_the_accessor_realm() {
    let mut rt=runtime();
    let html="<html><body><details id=old open=original></details><details id=seed open=replacement></details></body></html>";
    rt.set_dom(obscura_dom::parse_html(html));rt.run_page_init();
    let _child=crate::frame::FrameRealm::new(&mut rt,84,0,"https://example.com/child",html).unwrap();
    rt.execute_script("save-open-realms",r#"
      globalThis.rootOpenReceiver=document.getElementById('old');
      globalThis.childOpenWindow=__obscura_frameObjects[84].window;
      globalThis.childOpenReceiver=__obscura_frameObjects[84].document.getElementById('old');
      globalThis.rootOpenGetter=Object.getOwnPropertyDescriptor(HTMLDetailsElement.prototype,'open').get;
      globalThis.childOpenGetter=Object.getOwnPropertyDescriptor(childOpenWindow.HTMLDetailsElement.prototype,'open').get;
    "#).unwrap();
    assert_eq!(rt.evaluate("[rootOpenGetter.call(childOpenReceiver),childOpenGetter.call(rootOpenReceiver)]").unwrap(),serde_json::json!([true,true]));
    let child_state=rt.realm_states().borrow().by_frame_id(84).unwrap();
    let states=[rt.state.clone(),child_state];let mut replacements=Vec::new();
    for state in &states {
        let state=state.borrow();let dom=state.dom.as_ref().unwrap();
        let old=dom.get_element_by_id("old").unwrap();let generation=dom.node_generation(old).unwrap();
        let seed=dom.get_node(dom.get_element_by_id("seed").unwrap()).unwrap().data;
        dom.remove(old);let replacement=dom.new_node(seed);
        assert_eq!(replacement,old);assert_ne!(dom.node_generation(replacement),Some(generation));
        replacements.push(replacement);
    }
    assert_eq!(rt.evaluate(r#"(() => {
      const results=[];
      for(const [get,receiver,Expected] of [[rootOpenGetter,rootOpenReceiver,TypeError],
        [childOpenGetter,childOpenReceiver,childOpenWindow.TypeError],
        [rootOpenGetter,childOpenReceiver,TypeError],[childOpenGetter,rootOpenReceiver,childOpenWindow.TypeError]]) {
        try{get.call(receiver);results.push(false)}catch(error){results.push(Object.getPrototypeOf(error)===Expected.prototype)}
      }return results;
    })()"#).unwrap(),serde_json::json!([true,true,true,true]));
    for (state,replacement) in states.iter().zip(replacements) {
        let state=state.borrow();
        assert_eq!(state.dom.as_ref().unwrap().get_node(replacement).unwrap().get_attribute("open"),Some("replacement"));
    }
}

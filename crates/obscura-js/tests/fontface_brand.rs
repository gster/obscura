//! Genuine FontFace receivers. Font decoding and CSS descriptor grammar are separate work.
use obscura_js::{frame::FrameRealm, runtime::ObscuraJsRuntime};

fn runtime() -> ObscuraJsRuntime {
    let persona = obscura_net::EffectivePersona::builtin(obscura_net::StealthProfile::MacChrome153);
    let mut rt = ObscuraJsRuntime::with_base_url("https://fontface.test/page", persona);
    rt.set_dom(obscura_dom::parse_html("<html><body></body></html>"));
    rt.set_url("https://fontface.test/page");
    rt.run_page_init();
    rt
}

#[test]
fn fontface_subclass_keeps_genuine_state_and_constructor_identity() {
    let mut rt=runtime();
    let actual=rt.evaluate(r#"(() => {
        class Child extends FontFace {}
        const face=new Child('Arial','local(Arial)');
        face.family='Menlo';face.weight='700';
        const get=Object.getOwnPropertyDescriptor(FontFace.prototype,'family').get;
        return {child:face instanceof Child,base:face instanceof FontFace,
            family:get.call(face),weight:face.weight,status:face.status,
            promise:face.loaded instanceof Promise,samePromise:face.loaded===face.loaded};
    })()"#).unwrap();
    assert_eq!(actual,serde_json::json!({"child":true,"base":true,"family":"Menlo","weight":"700",
        "status":"unloaded","promise":true,"samePromise":true}));
}

#[tokio::test(flavor = "current_thread")]
async fn fontface_worker_keeps_private_brand_and_local_promise() {
    let mut rt=runtime();
    let source=serde_json::to_string(r#"
        const face=new FontFace('Arial','local(Arial)');
        const family=Object.getOwnPropertyDescriptor(FontFace.prototype,'family');
        const loaded=Object.getOwnPropertyDescriptor(FontFace.prototype,'loaded').get;
        const own=Reflect.ownKeys(face);
        face._family='forged';Object.setPrototypeOf(face,null);
        family.set.call(face,'Menlo');
        let rejected=false;
        try{family.get.call(new Proxy(face,{}));}catch(e){rejected=e instanceof TypeError;}
        postMessage({own,family:family.get.call(face),rejected,
            promise:loaded.call(face) instanceof Promise,samePromise:loaded.call(face)===loaded.call(face)});
    "#).unwrap();
    rt.execute_script("worker-fontface",&format!(r#"
        globalThis.workerFontface=null;
        const url=URL.createObjectURL(new Blob([{}],{{type:'application/javascript'}}));
        const worker=new Worker(url);
        worker.onmessage=e=>{{workerFontface=e.data;worker.terminate();URL.revokeObjectURL(url);}};
    "#,source)).unwrap();
    rt.run_event_loop_bounded(1000).await.unwrap();
    assert_eq!(rt.evaluate("workerFontface").unwrap(),serde_json::json!({"own":[],"family":"Menlo",
        "rejected":true,"promise":true,"samePromise":true}));
}

#[test]
fn fontface_all_attributes_reject_forged_receivers_before_conversion() {
    let mut rt=runtime();
    let actual=rt.evaluate(r#"(() => {
        const face=new FontFace('Arial','local(Arial)');
        const keys=['family','style','weight','stretch','unicodeRange','variant','featureSettings',
            'variationSettings','display','ascentOverride','descentOverride','lineGapOverride','status','loaded'];
        const receivers=[{},FontFace.prototype,Object.create(FontFace.prototype),new Proxy(face,{}),
            {_family:'Arial',_status:'loaded',_sets:new Set()},null,undefined,1];
        let conversions=0;
        const value={toString(){conversions++;return 'Arial';}};
        const rejects=fn=>{try{fn();return false;}catch(e){return e instanceof TypeError;}};
        return {rows:keys.map(key=>{
            const d=Object.getOwnPropertyDescriptor(FontFace.prototype,key);
            return [key,receivers.every(r=>rejects(()=>d.get.call(r))),
                !d.set||receivers.every(r=>rejects(()=>d.set.call(r,value)))];
        }),conversions};
    })()"#).unwrap();
    assert_eq!(actual["conversions"],0);
    let rows=actual["rows"].as_array().unwrap();
    assert_eq!(rows.len(),14);
    for row in rows { assert_eq!(row[1],true,"getter: {row}"); assert_eq!(row[2],true,"setter: {row}"); }
}

#[test]
fn fontface_attributes_have_idl_enumerability_and_nonconstructible_accessors() {
    let mut rt=runtime();
    let actual=rt.evaluate(r#"(() => {
        const keys=['family','style','weight','stretch','unicodeRange','variant','featureSettings',
            'variationSettings','display','ascentOverride','descentOverride','lineGapOverride','status','loaded'];
        const nonconstructor=fn=>{try{Reflect.construct(function(){},[],fn);return false;}catch(e){return e instanceof TypeError;}};
        return keys.map(key=>{
            const d=Object.getOwnPropertyDescriptor(FontFace.prototype,key);
            return [key,d.enumerable,d.configurable,d.get.name==='get '+key,d.get.length===0,
                !Object.hasOwn(d.get,'prototype'),nonconstructor(d.get),
                !d.set||(d.set.name==='set '+key&&d.set.length===1&&!Object.hasOwn(d.set,'prototype')&&nonconstructor(d.set))];
        });
    })()"#).unwrap();
    for row in actual.as_array().unwrap() {
        for value in &row.as_array().unwrap()[1..] { assert_eq!(value,true,"{row}"); }
    }
}

#[test]
fn fontface_state_is_private_and_survives_public_fields_and_prototype_changes() {
    let mut rt=runtime();
    let actual=rt.evaluate(r#"(() => {
        const proto=FontFace.prototype;
        const family=Object.getOwnPropertyDescriptor(proto,'family');
        const status=Object.getOwnPropertyDescriptor(proto,'status').get;
        const weight=Object.getOwnPropertyDescriptor(proto,'weight');
        const loaded=Object.getOwnPropertyDescriptor(proto,'loaded').get;
        let descriptorReads=0;
        const face=new FontFace({toString(){return 'Arial';}},'local(Arial)',{
            get weight(){descriptorReads++;return 400;}});
        const originalOwn=Reflect.ownKeys(face);
        face._family='forged';face._status='error';face._source='poison';face._sets=null;
        face._loadedPromise=Promise.resolve('forged');
        face._setDescriptor=()=>{throw Error('public helper');};
        face._changed=()=>{throw Error('public helper');};
        Object.setPrototypeOf(face,null);
        family.set.call(face,{toString(){return 'Menlo';}});
        weight.set.call(face,700);
        let symbolError=false;
        try{weight.set.call(face,Symbol());}catch(e){symbolError=e instanceof TypeError;}
        let conversionError=false;
        const marker={};try{family.set.call(face,{toString(){throw marker;}});}catch(e){conversionError=e===marker;}
        return {originalOwn,descriptorReads,family:family.get.call(face),weight:weight.get.call(face),
            status:status.call(face),samePromise:loaded.call(face)===loaded.call(face),symbolError,conversionError};
    })()"#).unwrap();
    assert_eq!(actual,serde_json::json!({"originalOwn":[],"descriptorReads":1,"family":"Menlo","weight":"700",
        "status":"unloaded","samePromise":true,"symbolError":true,"conversionError":true}));
}

#[test]
fn fontface_real_frames_borrow_both_directions_and_use_accessor_exception_realm() {
    let mut rt=runtime();
    let _frame=FrameRealm::new(&mut rt,831,0,"https://fontface.test/child","<html><body></body></html>").unwrap();
    let actual=rt.evaluate(r#"(() => {
        const child=__obscura_frameObjects[831].window;
        const local=new FontFace('Arial','local(Arial)'),foreign=new child.FontFace('Menlo','local(Menlo)');
        const a=Object.getOwnPropertyDescriptor(FontFace.prototype,'family');
        const b=Object.getOwnPropertyDescriptor(child.FontFace.prototype,'family');
        const parentLoaded=Object.getOwnPropertyDescriptor(FontFace.prototype,'loaded').get;
        const childLoaded=Object.getOwnPropertyDescriptor(child.FontFace.prototype,'loaded').get;
        a.set.call(foreign,'Arial');b.set.call(local,'Menlo');
        const rejected=(fn,receiver,Type)=>{try{fn.call(receiver);return false;}catch(e){return e instanceof Type;}};
        const invalid=[{},Object.create(child.FontFace.prototype),new Proxy(foreign,{})];
        const errors=invalid.map(r=>[rejected(a.get,r,TypeError),rejected(b.get,r,child.TypeError)]);
        let setterRealm=false;try{b.set.call({},'Arial');}catch(e){setterRealm=e instanceof child.TypeError&&!(e instanceof TypeError);}
        const promiseRealms=[parentLoaded.call(foreign) instanceof child.Promise,childLoaded.call(local) instanceof Promise];
        Object.setPrototypeOf(local,null);Object.setPrototypeOf(foreign,null);
        b.set.call(local,'Arial');a.set.call(foreign,'Menlo');
        return {values:[a.get.call(local),b.get.call(foreign),a.get.call(foreign),b.get.call(local)],
            errors,setterRealm,promiseRealms};
    })()"#).unwrap();
    assert_eq!(actual,serde_json::json!({"values":["Arial","Menlo","Menlo","Arial"],
        "errors":[[true,true],[true,true],[true,true]],"setterRealm":true,"promiseRealms":[true,true]}));
}

#[test]
fn fontface_private_registry_survives_both_realms_public_intrinsic_poisoning() {
    let mut rt=runtime();
    let _frame=FrameRealm::new(&mut rt,832,0,"https://fontface.test/child","<html><body></body></html>").unwrap();
    let actual=rt.evaluate(r#"(() => {
        const child=__obscura_frameObjects[832].window;
        const a=Object.getOwnPropertyDescriptor(FontFace.prototype,'family');
        const b=Object.getOwnPropertyDescriptor(child.FontFace.prototype,'family');
        const prototypes=[WeakMap.prototype,child.WeakMap.prototype];
        const old=prototypes.map(p=>[p.get,p.set]);
        const functions=[Function.prototype,child.Function.prototype],binds=functions.map(p=>p.bind);
        let calls=0,result;
        const poison=()=>{calls++;throw Error('public intrinsic');};
        try {
            for(const p of prototypes){p.get=poison;p.set=poison;}
            for(const p of functions)p.bind=poison;
            const x=new FontFace('Arial','local(Arial)'),y=new child.FontFace('Menlo','local(Menlo)');
            b.set.call(x,'Menlo');a.set.call(y,'Arial');
            let rejected=false;try{b.get.call(new Proxy(x,{}));}catch(e){rejected=e instanceof child.TypeError;}
            result={values:[a.get.call(x),b.get.call(y)],calls,rejected};
        } finally {
            for(let i=0;i<prototypes.length;i++){prototypes[i].get=old[i][0];prototypes[i].set=old[i][1];functions[i].bind=binds[i];}
        }
        return result;
    })()"#).unwrap();
    assert_eq!(actual,serde_json::json!({"values":["Menlo","Arial"],"calls":0,"rejected":true}));
}

#[tokio::test(flavor="current_thread")]
async fn fontface_load_invalid_receiver_rejects_in_called_realm_without_sync_throw() {
    let mut rt=runtime();
    let _frame=FrameRealm::new(&mut rt,833,0,"https://fontface.test/child","<html><body></body></html>").unwrap();
    let actual=rt.call_function_on_for_cdp(r#"async () => {
        const child=__obscura_frameObjects[833].window;
        const face=new FontFace('Arial','local(Arial)');
        const rows=[];
        for(const [fn,P,T] of [[FontFace.prototype.load,Promise,TypeError],
            [child.FontFace.prototype.load,child.Promise,child.TypeError]]) {
            for(const receiver of [null,undefined,{},Object.create(FontFace.prototype),new Proxy(face,{})]) {
                let p,sync=false;try{p=fn.call(receiver);}catch(e){sync=true;}
                let rejected=false;if(p)try{await p;}catch(e){rejected=e instanceof T;}
                rows.push([sync,p instanceof P,rejected]);
            }
        }
        return rows;
    }"#,None,&[],true,true).await.unwrap().value.unwrap();
    assert_eq!(actual,serde_json::json!(vec![vec![false,true,true];10]));
}

#[test]
fn fontface_sets_keep_authored_faces_and_accept_prototype_changed_foreign_faces() {
    let mut rt=runtime();
    let _frame=FrameRealm::new(&mut rt,834,0,"https://fontface.test/child","<html><body></body></html>").unwrap();
    let actual=rt.evaluate(r#"(() => {
        const style=document.createElement('style');
        style.textContent='@font-face{font-family:Authored;src:local(Arial)}';document.head.appendChild(style);
        const set=document.fonts,authored=Array.from(set)[0];
        const child=__obscura_frameObjects[834].window;
        const foreign=new child.FontFace('Menlo','local(Menlo)');
        Object.setPrototypeOf(foreign,null);
        const added=set.add(foreign)===set,again=set.add(foreign)===set;
        const before=[set.size,set.has(foreign),set.check('16px Menlo')];
        const values=Array.from(set),iterated=[];set.forEach((v,k,s)=>iterated.push(v===k&&s===set));
        const fake={_family:'Arial',_sets:new Set()};let rejected=false;
        try{set.add(fake);}catch(e){rejected=e instanceof TypeError;}
        const ownField=new FontFace('Arial','local(Arial)');ownField._cssConnected=true;set.add(ownField);
        const fieldIgnored=set.delete(ownField);
        const cssDelete=set.delete(authored);set.clear();
        const after=[set.size,set.has(authored),set.has(foreign)];
        style.remove();const finalSize=set.size;
        return {added,again,before,values:values[0]===authored&&values[1]===foreign,
            iterated,rejected,fieldIgnored,cssDelete,after,finalSize};
    })()"#).unwrap();
    assert_eq!(actual,serde_json::json!({"added":true,"again":true,"before":[2,true,false],"values":true,
        "iterated":[true,true],"rejected":true,"fieldIgnored":true,"cssDelete":false,"after":[1,true,false],"finalSize":0}));
}

#[test]
fn fontface_handoff_is_consumed_in_main_frame_and_after_document_reset() {
    let mut rt=runtime();
    let frame=FrameRealm::new(&mut rt,835,0,"https://fontface.test/child","<html><body></body></html>").unwrap();
    let probe=r#"(() => ({core:Object.hasOwn(globalThis,'__obscura_core_handoff'),
        registry:Object.getOwnPropertyNames(globalThis).filter(k=>/fontface.*(slot|registry|handoff)/i.test(k)),
        faceKeys:Reflect.ownKeys(new FontFace('Arial','local(Arial)'))}))()"#;
    let expected=serde_json::json!({"core":false,"registry":[],"faceKeys":[]});
    assert_eq!(rt.evaluate(probe).unwrap(),expected);
    assert_eq!(frame.evaluate(&mut rt,probe).unwrap(),expected);
    drop(frame);
    rt.set_dom(obscura_dom::parse_html("<html><body></body></html>"));
    rt.set_url("https://fontface.test/next");rt.run_page_init();
    assert_eq!(rt.evaluate(probe).unwrap(),expected);
}

// Preserve the existing shim lifecycle without claiming the local() resource
// was decoded. Backend loading is explicitly outside this private-state slice.
#[tokio::test(flavor="current_thread")]
async fn fontface_private_load_state_preserves_existing_set_lifecycle() {
    let mut rt=runtime();
    let _frame=FrameRealm::new(&mut rt,836,0,"https://fontface.test/child","<html><body></body></html>").unwrap();
    let actual=rt.call_function_on_for_cdp(r#"async () => {
        const child=__obscura_frameObjects[836].window;
        const face=new child.FontFace('Menlo','local(Menlo)');
        const status=Object.getOwnPropertyDescriptor(FontFace.prototype,'status').get;
        const loaded=Object.getOwnPropertyDescriptor(FontFace.prototype,'loaded').get;
        const set=new FontFaceSet([face]),events=[];
        set.onloading=event=>events.push(event.type);
        set.onloadingdone=event=>events.push(event.type);
        Object.setPrototypeOf(face,null);
        Object.defineProperty(face,'status',{get(){throw Error('public status');}});
        Object.defineProperty(face,'loaded',{get(){throw Error('public promise');}});
        Object.defineProperty(face,'load',{value(){throw Error('public load');}});
        const before=status.call(face),pending=loaded.call(face);
        const matches=await set.load('16px Menlo');
        const ready=await set.ready;
        return {before,after:status.call(face),events,sameFace:matches.length===1&&matches[0]===face,
            ready:ready===set,promiseIdentity:FontFace.prototype.load.call(face)===pending,
            resultIdentity:await pending===face,checked:set.check('16px Menlo')};
    }"#,None,&[],true,true).await.unwrap().value.unwrap();
    assert_eq!(actual,serde_json::json!({"before":"unloaded","after":"loaded","events":["loading","loadingdone"],
        "sameFace":true,"ready":true,"promiseIdentity":true,"resultIdentity":true,"checked":true}));
}

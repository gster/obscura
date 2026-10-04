// Root-owned baseline repros, copied privately; candidate tests are UNRUN.
use obscura_js::{frame::FrameRealm, runtime::ObscuraJsRuntime};

fn runtime(html: &str) -> ObscuraJsRuntime {
    let persona = obscura_net::EffectivePersona::builtin(obscura_net::StealthProfile::MacChrome153);
    let mut runtime = ObscuraJsRuntime::with_base_url("https://canvas.test/page", persona);
    runtime.set_dom(obscura_dom::parse_html(html));
    runtime.set_url("https://canvas.test/page");
    runtime.set_viewport(20.0, 20.0);
    runtime.run_page_init();
    runtime
}

#[test]
fn methods_have_webidl_constructor_and_receiver_semantics() {
    let mut rt = runtime("<html><body></body></html>");
    let actual = rt.evaluate(r#"(() => {
        const proto = HTMLCanvasElement.prototype;
        const methods = ['getContext','toDataURL','toBlob'].map(name => {
            const fn = proto[name];
            let constructible = true;
            try { Reflect.construct(function(){}, [], fn); } catch (e) { constructible = false; }
            return [fn.name,fn.length,constructible,Object.hasOwn(fn,'prototype')];
        });
        const getter = Object.getOwnPropertyDescriptor(proto,'width').get;
        const invalid = [proto,{},Object.create(proto),new Proxy(document.createElement('canvas'),{})];
        const rejects = invalid.map(receiver => {
            try { getter.call(receiver); return false; } catch(e) { return e instanceof TypeError; }
        });
        const context = document.createElement('canvas').getContext('2d');
        return {methods,rejects,realInterface:context instanceof CanvasRenderingContext2D,
            canonicalPrototype:Object.getPrototypeOf(context)===CanvasRenderingContext2D.prototype};
    })()"#).unwrap();
    assert_eq!(actual,serde_json::json!({"methods":[["getContext",1,false,false],["toDataURL",0,false,false],["toBlob",1,false,false]],"rejects":[true,true,true,true],"realInterface":true,"canonicalPrototype":true}));
}

#[test]
fn real_child_canvas_owns_a_surface_when_parent_has_no_canvas() {
    let mut rt = runtime("<html><body><div>parent</div></body></html>");
    let frame = FrameRealm::new(&mut rt,71,0,"https://canvas.test/child","<html><body><canvas id='paint' width='2' height='1'></canvas></body></html>").unwrap();
    let actual = frame.evaluate(&mut rt,r#"(() => {
        const canvas=document.getElementById('paint'),ctx=canvas.getContext('2d');
        if (!ctx) return {context:false,pixel:null};
        ctx.fillStyle='#ff0000';ctx.fillRect(0,0,2,1);
        return {context:true,pixel:Array.from(ctx.getImageData(0,0,1,1).data)};
    })()"#).unwrap();
    assert_eq!(actual,serde_json::json!({"context":true,"pixel":[255,0,0,255]}));
}

fn first_pixel(bytes: Vec<u8>) -> Vec<u8> {
    let mut reader=png::Decoder::new(std::io::Cursor::new(bytes)).read_info().unwrap();
    let mut pixels=vec![0;reader.output_buffer_size().unwrap()];
    let info=reader.next_frame(&mut pixels).unwrap();
    assert_eq!(info.color_type,png::ColorType::Rgba);
    pixels[..4].to_vec()
}

#[cfg(feature = "render")]
#[test]
fn child_node_id_collision_does_not_replace_parent_canvas_backing() {
    let html="<html><body style='margin:0'><canvas id='paint' width='2' height='1' style='display:block;width:2px;height:1px'></canvas></body></html>";
    let mut rt=runtime(html);
    rt.evaluate(r#"(() => {const ctx=document.getElementById('paint').getContext('2d');ctx.fillStyle='#ff0000';ctx.fillRect(0,0,2,1);return true;})()"#).unwrap();
    let before=first_pixel(rt.screenshot_prepared((20.0,20.0),Some("https://canvas.test/page")).unwrap());
    assert_eq!(before,vec![255,0,0,255],"positive parent paint control");
    let frame=FrameRealm::new(&mut rt,72,0,"https://canvas.test/child",html).unwrap();
    let actual=frame.evaluate(&mut rt,r#"(() => {const ctx=document.getElementById('paint').getContext('2d');if(!ctx)return false;ctx.fillStyle='#00ff00';ctx.fillRect(0,0,2,1);return true;})()"#).unwrap();
    assert_eq!(actual,serde_json::json!(true),"child's own context exists");
    let after=first_pixel(rt.screenshot_prepared((20.0,20.0),Some("https://canvas.test/page")).unwrap());
    assert_eq!(after,before,"same numeric node id must not alias another document's surface");
}

#[test]
fn retained_canvas_does_not_select_replacement_frame_document() {
    let mut rt=runtime("<html><body><canvas id='paint' width='2' height='1'></canvas></body></html>");
    let old=FrameRealm::new(&mut rt,77,0,"https://canvas.test/old","<html><body><canvas id='paint' width='2' height='1'></canvas></body></html>").unwrap();
    rt.evaluate("(() => {globalThis.retiredCanvas=__obscura_frameObjects[77].window.document.getElementById('paint');return retiredCanvas.width;})()").unwrap();
    drop(old);
    let replacement=FrameRealm::new(&mut rt,77,0,"https://canvas.test/new","<html><body><canvas id='paint' width='5' height='1'></canvas></body></html>").unwrap();
    let actual=rt.evaluate(r#"(() => {
        const next=__obscura_frameObjects[77].window.document.getElementById('paint');
        const before=[retiredCanvas.width,next.width];
        retiredCanvas.width=7;
        return {before,after:[retiredCanvas.width,next.width]};
    })()"#).unwrap();
    assert_eq!(actual,serde_json::json!({"before":[2,5],"after":[7,5]}));
    assert_eq!(replacement.evaluate(&mut rt,"document.getElementById('paint').width").unwrap(),serde_json::json!(5));
}

#[test]
fn serializing_blank_canvas_does_not_choose_a_context() {
    let mut rt=runtime("<html><body></body></html>");
    let actual=rt.evaluate(r#"(() => {
        const canvas=document.createElement('canvas');canvas.width=1;canvas.height=1;
        const png=canvas.toDataURL();
        const gl=canvas.getContext('webgl');
        return {png:png.startsWith('data:image/png;base64,'),webglAvailable:gl!==null,
            exclusive:gl!==null && canvas.getContext('2d')===null};
    })()"#).unwrap();
    assert_eq!(actual,serde_json::json!({"png":true,"webglAvailable":true,"exclusive":true}));
}

#[tokio::test(flavor="current_thread")]
async fn to_blob_queues_callback_and_snapshots_actual_png() {
    let mut rt=runtime("<html><body></body></html>");
    let actual=rt.call_function_on_for_cdp(r#"async () => {
        const canvas=document.createElement('canvas');canvas.width=1;canvas.height=1;
        const ctx=canvas.getContext('2d');ctx.fillStyle='#ff0000';ctx.fillRect(0,0,1,1);
        let returned=false,calledBeforeReturn=null;
        const pending=new Promise(resolve=>canvas.toBlob(blob=>{
            calledBeforeReturn=!returned;resolve(blob);
        },'image/not-supported'));
        returned=true;ctx.fillStyle='#00ff00';ctx.fillRect(0,0,1,1);
        const blob=await pending;
        return {calledBeforeReturn,mime:blob && blob.type,
            bytes:blob ? Array.from(new Uint8Array(await blob.arrayBuffer())) : null};
    }"#,None,&[],true,true).await.unwrap().value.unwrap();
    let bytes:Vec<u8>=serde_json::from_value(actual["bytes"].clone()).unwrap();
    let summary=serde_json::json!({"calledBeforeReturn":actual["calledBeforeReturn"],"mime":actual["mime"],"pixel":first_pixel(bytes)});
    assert_eq!(summary,serde_json::json!({"calledBeforeReturn":false,"mime":"image/png","pixel":[255,0,0,255]}));
}

#[test]
fn width_reflection_matches_unsigned_idl_conversion_and_content_parsing() {
    let mut rt=runtime("<html><body></body></html>");
    let actual=rt.evaluate(r#"(() => {
        const assignments=[];
        for(const value of [-1,2147483647,2147483648,4294967295,4294967296,1.9,-1.9,NaN,Infinity,undefined,1n,Symbol()]){
            const canvas=document.createElement('canvas');canvas.height=0;
            let error=null;try{canvas.width=value;}catch(e){error=e.name;}
            assignments.push([canvas.width,canvas.getAttribute('width'),error]);
        }
        const attributes=['2147483648','4294967295','-1','+5','  +5foo','12e2','3.5','0x10',''].map(value=>{
            const canvas=document.createElement('canvas');canvas.height=0;canvas.setAttribute('width',value);
            return [canvas.width,canvas.getAttribute('width')];
        });
        return {assignments,attributes};
    })()"#).unwrap();
    assert_eq!(actual,serde_json::json!({"assignments":[[300,"300",null],[2147483647,"2147483647",null],[300,"300",null],[300,"300",null],[0,"0",null],[1,"1",null],[300,"300",null],[0,"0",null],[0,"0",null],[0,"0",null],[300,null,"TypeError"],[300,null,"TypeError"]],"attributes":[[300,"2147483648"],[300,"4294967295"],[300,"-1"],[5,"+5"],[5,"  +5foo"],[12,"12e2"],[3,"3.5"],[0,"0x10"],[300,""]]}));
}

#[test]
fn get_context_uses_case_sensitive_id_and_required_domstring() {
    let mut rt=runtime("<html><body></body></html>");
    let actual=rt.evaluate(r#"(() => {
        const canvas=document.createElement('canvas');canvas.width=1;canvas.height=1;
        let missing=false,symbol=false;
        try{canvas.getContext();}catch(e){missing=e instanceof TypeError;}
        try{canvas.getContext(Symbol());}catch(e){symbol=e instanceof TypeError;}
        const upper=canvas.getContext('2D');
        const lower=canvas.getContext('2d');
        return {missing,symbol,upperIsNull:upper===null,lowerIsReal:lower!==null,
            repeatedIsSame:canvas.getContext('2d')===lower};
    })()"#).unwrap();
    assert_eq!(actual,serde_json::json!({"missing":true,"symbol":true,"upperIsNull":true,"lowerIsReal":true,"repeatedIsSame":true}));
}

#[test]
fn offscreen_2d_owns_its_dimensions_context_and_pixels() {
    let mut rt=runtime("<html><body></body></html>");
    let actual=rt.evaluate(r#"(() => {
        const canvas=new OffscreenCanvas(2,3),ctx=canvas.getContext('2d');
        if (!ctx) return {context:false};
        ctx.fillStyle='#ff0000';ctx.fillRect(0,0,2,3);
        const before=Array.from(ctx.getImageData(0,0,1,1).data);
        const ownCanvas=ctx.canvas===canvas, dimensions=[ctx.canvas.width,ctx.canvas.height];
        const same=canvas.getContext('2d')===ctx, exclusive=canvas.getContext('webgl')===null;
        canvas.width=4;
        return {context:true,ownCanvas,dimensions,same,exclusive,before,
            resized:[ctx.canvas.width,ctx.canvas.height],after:Array.from(ctx.getImageData(0,0,1,1).data)};
    })()"#).unwrap();
    assert_eq!(actual,serde_json::json!({"context":true,"ownCanvas":true,"dimensions":[2,3],"same":true,"exclusive":true,"before":[255,0,0,255],"resized":[4,3],"after":[0,0,0,0]}));
}

#[tokio::test(flavor="current_thread")]
async fn offscreen_blob_is_a_regular_operation_with_promise_errors_and_call_time_pixels() {
    let mut rt=runtime("<html><body></body></html>");
    let actual=rt.call_function_on_for_cdp(r#"async () => {
        const fn=OffscreenCanvas.prototype.convertToBlob;
        let constructible=true;
        try { Reflect.construct(function(){},[],fn); } catch(e) { constructible=false; }
        let syncError=null,promise=false,error=null;
        try {
            const invalid=fn.call({});promise=invalid instanceof Promise;
            try { await invalid; } catch(e) { error=e.name; }
        } catch(e) { syncError=e.name; }
        const canvas=new OffscreenCanvas(1,1),ctx=canvas.getContext('2d');
        ctx.fillStyle='red';ctx.fillRect(0,0,1,1);
        const pending=canvas.convertToBlob();
        ctx.fillStyle='green';ctx.fillRect(0,0,1,1);
        const blob=await pending;
        return {tag:Object.prototype.toString.call(fn),name:fn.name,length:fn.length,
            constructible,ownPrototype:Object.hasOwn(fn,'prototype'),promise,error,syncError,
            mime:blob.type,bytes:Array.from(new Uint8Array(await blob.arrayBuffer()))};
    }"#,None,&[],true,true).await.unwrap().value.unwrap();
    let mut actual=actual;
    let bytes:Vec<u8>=serde_json::from_value(actual.as_object_mut().unwrap().remove("bytes").unwrap()).unwrap();
    actual["pixel"]=serde_json::json!(first_pixel(bytes));
    assert_eq!(actual,serde_json::json!({"tag":"[object Function]","name":"convertToBlob","length":0,
        "constructible":false,"ownPrototype":false,"promise":true,"error":"TypeError","syncError":null,
        "mime":"image/png","pixel":[255,0,0,255]}));
}


#[test]
fn canvas_cross_realm_borrowing_preserves_owner_and_private_brand() {
    let mut rt=runtime("<html><body></body></html>");
    let _child=FrameRealm::new(&mut rt,78,0,"https://canvas.test/child","<html><body></body></html>").unwrap();
    let actual=rt.evaluate(r#"(() => {
        const child=__obscura_frameObjects[78].window;
        const canvas=child.document.createElement('canvas');canvas.width=2;canvas.height=1;
        const parentCanvas=document.createElement('canvas');parentCanvas.width=3;parentCanvas.height=1;
        const context=HTMLCanvasElement.prototype.getContext.call(canvas,'2d');
        const ownRealm=Object.getPrototypeOf(context)===child.CanvasRenderingContext2D.prototype;
        CanvasRenderingContext2D.prototype.fillRect.call(context,0,0,1,1);
        const opposite=child.HTMLCanvasElement.prototype.getContext.call(parentCanvas,'2d');
        const reverseRealm=Object.getPrototypeOf(opposite)===CanvasRenderingContext2D.prototype;
        const width=Object.getOwnPropertyDescriptor(HTMLCanvasElement.prototype,'width').get;
        const fill=CanvasRenderingContext2D.prototype.fillRect;
        const fake=[{},Object.create(child.HTMLCanvasElement.prototype),new Proxy(canvas,{})].map(value=>{
            try{width.call(value);return false;}catch(e){return e instanceof TypeError;}
        });
        Object.setPrototypeOf(canvas,null);Object.setPrototypeOf(context,null);
        const poisonedGet=WeakMap.prototype.get,poisonedBind=Function.prototype.bind;
        let poisonCalls=0;
        try {
            WeakMap.prototype.get=function(){poisonCalls++;throw Error('public get');};
            Function.prototype.bind=function(){poisonCalls++;throw Error('public bind');};
            fill.call(context,1,0,1,1);
            return {ownRealm,reverseRealm,fake,width:width.call(canvas),poisonCalls,
                native:Function.prototype.toString.call(child.HTMLCanvasElement.prototype.getContext)==='function getContext() { [native code] }',
                pixel:Array.from(CanvasRenderingContext2D.prototype.getImageData.call(context,1,0,1,1).data)};
        } finally {WeakMap.prototype.get=poisonedGet;Function.prototype.bind=poisonedBind;}
    })()"#).unwrap();
    assert_eq!(actual,serde_json::json!({"ownRealm":true,"reverseRealm":true,"fake":[true,true,true],"width":2,"poisonCalls":0,"native":true,"pixel":[0,0,0,255]}));
}

#[test]
fn canvas_reads_embedding_mutations_and_take_dom_detaches_latest_attributes() {
    let mut rt=runtime("<html><body><canvas id='paint' width='2' height='1'></canvas></body></html>");
    rt.evaluate("(() => {globalThis.retained=document.getElementById('paint');return retained.width;})()").unwrap();
    rt.with_dom(|dom| {let id=dom.get_element_by_id("paint").unwrap();dom.with_node_mut(id,|node|node.set_attribute("width","5".into()));}).unwrap();
    assert_eq!(rt.evaluate("[retained.width,retained.getAttribute('width'),retained.getContext('2d').canvas.width]").unwrap(),serde_json::json!([5,"5",5]));
    rt.with_dom(|dom| {let id=dom.get_element_by_id("paint").unwrap();dom.with_node_mut(id,|node|node.set_attribute("width","6".into()));}).unwrap();
    let transferred=rt.take_dom().unwrap();
    let id=transferred.get_element_by_id("paint").unwrap();
    transferred.with_node_mut(id,|node|node.set_attribute("width","9".into()));
    assert_eq!(rt.evaluate("(() => {const before=retained.width;retained.width=7;return [before,retained.width];})()").unwrap(),serde_json::json!([6,7]));
    assert_eq!(transferred.get_node(id).unwrap().get_attribute("width"),Some("9"));
}

#[test]
fn canvas_set_dom_keeps_retained_bitmap_and_attributes_separate() {
    let mut rt=runtime("<html><body><canvas id='paint' width='2' height='1'></canvas></body></html>");
    rt.evaluate("(() => {globalThis.retained=document.getElementById('paint');globalThis.retainedCtx=retained.getContext('2d');retainedCtx.fillStyle='red';retainedCtx.fillRect(0,0,2,1);return true;})()").unwrap();
    rt.set_dom(obscura_dom::parse_html("<html><body><canvas id='paint' width='5' height='1'></canvas></body></html>"));
    assert_eq!(rt.evaluate("(() => {const pixel=Array.from(retainedCtx.getImageData(0,0,1,1).data);const before=retained.width;retained.width=7;return [before,retained.width,pixel,Array.from(retainedCtx.getImageData(0,0,1,1).data)];})()").unwrap(),serde_json::json!([2,7,[255,0,0,255],[0,0,0,0]]));
    assert_eq!(rt.with_dom(|dom|dom.get_node(dom.get_element_by_id("paint").unwrap()).unwrap().get_attribute("width").map(str::to_owned)).unwrap(),Some("5".into()));
}

#[cfg(feature="render")]
#[tokio::test(flavor="current_thread")]
async fn canvas_attribute_change_invalidates_style_and_resize_observation() {
    let mut rt=runtime("<html><body><canvas id='paint' width='2' height='1' style='display:block'></canvas></body></html>");
    rt.evaluate("(() => {globalThis.canvas=document.getElementById('paint');globalThis.widths=[];globalThis.observer=new ResizeObserver(entries=>widths.push(entries[0].contentRect.width));observer.observe(canvas);return getComputedStyle(canvas).width;})()").unwrap();
    rt.run_event_loop_bounded(40).await.unwrap();
    assert_eq!(rt.evaluate("(() => {const before=getComputedStyle(canvas).width;canvas.width=7;return [before,getComputedStyle(canvas).width];})()").unwrap(),serde_json::json!(["2px","7px"]));
    rt.run_event_loop_bounded(40).await.unwrap();
    assert_eq!(rt.evaluate("widths").unwrap(),serde_json::json!([2,7]));
}

#[tokio::test(flavor="current_thread")]
async fn borrowed_to_blob_queues_in_canvas_realm_and_cancels_retired_owner() {
    let mut rt=runtime("<html><body></body></html>");
    let old=FrameRealm::new(&mut rt,79,0,"https://canvas.test/old","<html><body><canvas id='paint' width='1' height='1'></canvas></body></html>").unwrap();
    rt.evaluate("(() => {globalThis.blobs=[];globalThis.oldCanvas=__obscura_frameObjects[79].window.document.getElementById('paint');HTMLCanvasElement.prototype.toBlob.call(oldCanvas,b=>blobs.push(['old',b.type]));return true;})()").unwrap();
    drop(old);
    let _fresh=FrameRealm::new(&mut rt,79,0,"https://canvas.test/new","<html><body><canvas id='paint' width='1' height='1'></canvas></body></html>").unwrap();
    rt.evaluate("(() => {const fresh=__obscura_frameObjects[79].window.document.getElementById('paint');HTMLCanvasElement.prototype.toBlob.call(fresh,b=>blobs.push(['fresh',b.type]));return true;})()").unwrap();
    rt.run_event_loop_bounded(40).await.unwrap();
    assert_eq!(rt.evaluate("blobs").unwrap(),serde_json::json!([["fresh","image/png"]]));
}

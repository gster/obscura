//! Source-prepared tests only. Run individually with release nextest.
use super::*;
use obscura_dom::parse_html;
use serde_json::json;

fn runtime(html: &str) -> ObscuraJsRuntime {
    let mut rt = ObscuraJsRuntime::new(obscura_net::EffectivePersona::builtin(
        obscura_net::StealthProfile::WindowsChrome145));
    rt.set_dom(parse_html(html));
    rt.set_url("https://image-fixture.invalid/page");
    rt.run_page_init();
    rt
}

#[tokio::test(flavor = "current_thread")]
async fn image_idl_attributes_brand_conversion_and_private_handoff() {
    let mut rt = runtime("<body><img id='i' style='display:none'></body>");
    assert_eq!(rt.evaluate(r#"(() => {
        const i=document.getElementById('i');
        const get=Object.getOwnPropertyDescriptor(HTMLImageElement.prototype,'width').get;
        const values=[];
        for (const raw of ['  +17px','-0junk','-1','4294967295','4294967296','\u00a09']) {
            i.setAttribute('width',raw); values.push(i.width);
        }
        for (const input of [-1,4294967297,3.9,Infinity,NaN]) {
            i.width=input; values.push([i.width,i.getAttribute('width')]);
        }
        let wrongBrand=false,bigint=false,rawConstructor=false;
        try { get.call({_nid:i._nid,getAttribute:()=>900}); } catch(e) { wrongBrand=e instanceof TypeError; }
        try { i.width=1n; } catch(e) { bigint=e instanceof TypeError; }
        try { new HTMLImageElement(i._nid); } catch(e) { rawConstructor=e instanceof TypeError; }
        return [values,wrongBrand,bigint,rawConstructor,Object.hasOwn(globalThis,'__obscura_image_dimensions_handoff')];
    })()"#).unwrap(),json!([[17,0,0,4294967295u64,0,0,
        [4294967295u64,"4294967295"],[1,"1"],[3,"3"],[0,"0"],[0,"0"]],true,true,true,false]));
}

#[cfg(feature = "render")]
#[tokio::test(flavor = "current_thread")]
async fn image_idl_css_content_box_loaded_broken_and_cache_mutation() {
    let mut rt = runtime(r#"<style>
      img{display:block;padding:4px 7px;border:3px solid;width:80px;height:30px}
      #border{box-sizing:border-box;width:100px;height:60px;transform:scale(3)}
      #fraction{padding:0;border:0;width:11.75px;height:9.25px}
      #hidden{display:none;width:999px} #gone{display:none}
    </style><img id='loaded' src='/ok.svg'><img id='broken' src='/bad' alt=''>
    <img id='border'><img id='fraction'><img id='hidden' width='37'>
    <div id='gone'><img id='descendant' width='41'></div>"#);
    {
        let mut state=rt.state.borrow_mut();
        state.render_resources.seed_image("https://image-fixture.invalid/ok.svg".into(),
            obscura_render::ImageRequestProfile::NoCorsInclude,
            br#"<svg xmlns='http://www.w3.org/2000/svg' width='24' height='12'/>"#.to_vec());
        state.render_resources.seed_image_missing("https://image-fixture.invalid/bad".into(),
            obscura_render::ImageRequestProfile::NoCorsInclude);
    }
    assert_eq!(rt.evaluate(r#"['loaded','broken','border','fraction','hidden','descendant']
        .map(id=>{const i=document.getElementById(id);return [i.width,i.height]})"#).unwrap(),
        json!([[80,30],[80,30],[80,46],[12,9],[37,0],[41,0]]));
    assert!(rt.state.borrow().prepared_render.is_some());
    assert_eq!(rt.evaluate(r#"(() => {
        const i=document.getElementById('loaded'); i.style.width='123px';
        const first=i.width; const again=i.width;
        i.style.width=''; i.width=51;
        // The stylesheet's width still wins over the presentation attribute.
        return [first,again,i.width,i.getAttribute('width')];
    })()"#).unwrap(),json!([123,123,80,"51"]));
}

#[cfg(feature = "render")]
#[tokio::test(flavor = "current_thread")]
async fn image_idl_no_layout_uses_raw_cached_size_not_srcset_density() {
    let mut rt=runtime("<img id='i' style='display:none' srcset='/dense.svg 2x'>");
    rt.state.borrow_mut().render_resources.seed_image("https://image-fixture.invalid/dense.svg".into(),
        obscura_render::ImageRequestProfile::NoCorsInclude,
        br#"<svg xmlns='http://www.w3.org/2000/svg' width='24' height='12'/>"#.to_vec());
    assert_eq!(rt.evaluate(r#"(() => {
        const i=document.getElementById('i'); const hidden=[i.width,i.height,i.naturalWidth,i.naturalHeight];
        i.remove(); i.style.display='block'; i.style.width='500px';
        return [hidden,[i.width,i.height]];
    })()"#).unwrap(),json!([[24,12,12,6],[24,12]]));
}

#[tokio::test(flavor = "current_thread")]
async fn image_idl_record_ignores_public_nid_and_methods() {
    let mut rt=runtime("<img id='a' width='17' style='display:none'><img id='b' width='93' style='display:none'>");
    assert_eq!(rt.evaluate(r#"(() => {
        const a=document.getElementById('a'),b=document.getElementById('b');
        a._nid=b._nid; a.getAttribute=()=>800; a._imageNaturalWidth=900;
        const old=a.width; a.width=29;
        return [old,a.width,b.width];
    })()"#).unwrap(),json!([17,29,93]));
}

#[tokio::test(flavor = "current_thread")]
async fn image_idl_old_wrapper_and_canvas_share_original_tree_after_replacement() {
    let mut rt=runtime("<img id='i' width='17' style='display:none'><canvas id='c' width='21'></canvas>");
    rt.execute_script("retain", "globalThis.savedImage=document.getElementById('i');globalThis.savedCanvas=document.getElementById('c'); void savedCanvas.width;").unwrap();
    rt.set_dom(parse_html("<img id='i' width='93' style='display:none'><canvas id='c' width='94'></canvas>"));
    rt.run_page_init();
    assert_eq!(rt.evaluate(r#"(() => {
        Element.prototype.setAttribute.call(savedImage,'width','27'); savedCanvas.width=31;
        const fresh=document.getElementById('i');
        const result=[savedImage.width,savedImage.getAttribute('width'),savedCanvas.width,fresh.width,savedImage!==fresh];
        savedImage.width=29; result.push(Element.prototype.getAttribute.call(savedImage,'width'));
        result.push(savedImage.getAttributeNames().includes('width'),savedImage.attributes.getNamedItem('width').value);
        return result;
    })()"#).unwrap(),json!([27,"27",31,93,true,"29",true,"29"]));
    assert_eq!(rt.state.borrow().dom.as_ref().unwrap().get_node(
        rt.state.borrow().dom.as_ref().unwrap().get_element_by_id("c").unwrap()).unwrap().get_attribute("width"),Some("94"));
}

#[cfg(feature = "render")]
#[tokio::test(flavor = "current_thread")]
async fn image_idl_loaded_realm_borrow_uses_receiver_document() {
    let mut rt=runtime("<iframe id='f'></iframe><img id='p' style='width:97px;height:11px'>");
    let frame_number=rt.evaluate("document.getElementById('f')._nid").unwrap().as_f64().unwrap();
    assert!(frame_number.is_finite() && frame_number.fract()==0.0
        && (0.0..=f64::from(u32::MAX)).contains(&frame_number));
    let frame_id=frame_number as u32;
    // Host-created loaded-realm fixture: reproduce the existing frame-ready
    // attachment used by the qualified document-lifecycle test. This is not
    // network navigation, a synthetic child global, or a ParentWindowProxy fix.
    rt.execute_script("attach-loaded-image-frame",&format!(r#"(() => {{
        const iframe=document.getElementById('f');
        void iframe.contentDocument;
        iframe._frameId={frame_id};
        globalThis.__obscura_frameElements[{frame_id}]=iframe;
    }})()"#)).unwrap();
    let frame=crate::frame::FrameRealm::new(&mut rt,frame_id,0,
        "https://image-fixture.invalid/child", "<img id='c' style='width:31px;height:19px'>").unwrap();
    // A real child script creates these sentinels. Identity checks below make
    // a blank-window facade or copied descriptor an invalid test substitute.
    frame.execute_script(&mut rt,"globalThis.imageFixtureDocument=document;globalThis.imageFixtureConstructor=HTMLImageElement;").unwrap();
    assert_eq!(rt.evaluate(r#"(() => {
        const iframe=document.getElementById('f');
        const child=iframe.contentWindow;
        const childDocument=iframe.contentDocument;
        const published=globalThis.__obscura_frameObjects[iframe._frameId];
        if(childDocument!==published.document || child.document!==childDocument
            || child.imageFixtureDocument!==childDocument
            || child.imageFixtureConstructor!==published.window.HTMLImageElement
            || child.HTMLImageElement!==child.imageFixtureConstructor)
            throw new Error('expected the actual published loaded child realm');
        const parentGet=Object.getOwnPropertyDescriptor(HTMLImageElement.prototype,'width').get;
        const childGet=Object.getOwnPropertyDescriptor(child.HTMLImageElement.prototype,'width').get;
        if(parentGet===childGet || Object.getPrototypeOf(parentGet)!==Function.prototype
            || Object.getPrototypeOf(childGet)!==child.Function.prototype)
            throw new Error('expected getters created in distinct actual realms');
        const childImage=childDocument.getElementById('c');
        const parentImage=document.getElementById('p');
        globalThis.retainedChildImage=childImage;
        return [parentGet.call(childImage),parentGet.call(parentImage),
            childGet.call(parentImage),childGet.call(childImage)];
    })()"#).unwrap(),json!([31,97,97,31]));
    drop(frame);
    assert_eq!(rt.evaluate("(() => { retainedChildImage.width=43;return [retainedChildImage.width,retainedChildImage.getAttribute('width')]; })()").unwrap(),json!([43,"43"]));
}

#[cfg(not(feature = "render"))]
#[tokio::test(flavor = "current_thread")]
async fn image_idl_no_render_reports_attribute_only_boundary() {
    let mut rt=runtime("<img id='i' width='17' height='9' style='width:300px;height:200px'>");
    assert_eq!(rt.evaluate("(() => {const i=document.getElementById('i');return [i.width,i.height]})()").unwrap(),json!([17,9]));
}

#[tokio::test(flavor = "current_thread")]
async fn image_idl_document_open_preserves_original_and_new_image_identity() {
    let mut rt=runtime("<img id='i' width='17' style='display:none'>");
    assert_eq!(rt.evaluate(r#"(() => {
        const original=document.getElementById('i');
        document.open(); document.write('<img id="next" width="93" style="display:none">'); document.close();
        const next=document.getElementById('next');original.setAttribute('width','29');
        return [original.width,next.width,original!==next,original.isConnected];
    })()"#).unwrap(),json!([29,93,true,false]));
}

#[cfg(feature = "render")]
#[tokio::test(flavor = "current_thread")]
async fn image_retained_content_survives_cache_eviction_without_prior_dimension_read() {
    let mut rt=runtime("<img id='i' src='/original.svg' style='display:none'>");
    rt.execute_script("retain-image","globalThis.savedImage=document.getElementById('i')").unwrap();
    rt.seed_render_image_resource("https://image-fixture.invalid/original.svg".into(),
        obscura_render::ImageRequestProfile::NoCorsInclude,
        Some(br#"<svg xmlns='http://www.w3.org/2000/svg' width='24' height='12'/>"#.to_vec()));
    rt.state.borrow_mut().render_resources=obscura_render::RenderResourceCache::with_loader(|_: &str| None);
    rt.set_dom(parse_html("<img id='i' width='99'>"));rt.run_page_init();
    assert_eq!(rt.evaluate(r#"(() => {
        const before=[savedImage.width,savedImage.height];
        Element.prototype.setAttributeNS.call(savedImage,null,'width','35'); const explicit=savedImage.width;
        Element.prototype.removeAttributeNS.call(savedImage,null,'width');
        return [before,explicit,savedImage.width];
    })()"#).unwrap(),json!([[24,12],35,24]));
}

#[cfg(feature = "render")]
#[tokio::test(flavor = "current_thread")]
async fn image_retired_pending_request_keeps_last_committed_metadata_without_new_fetch() {
    let mut rt=runtime("<img id='i' src='/old.svg' style='display:none'>");
    rt.execute_script("retain-image","globalThis.savedImage=document.getElementById('i')").unwrap();
    rt.seed_render_image_resource("https://image-fixture.invalid/old.svg".into(),
        obscura_render::ImageRequestProfile::NoCorsInclude,
        Some(br#"<svg xmlns='http://www.w3.org/2000/svg' width='24' height='12'/>"#.to_vec()));
    rt.execute_script("pending-source","savedImage.setAttribute('src','/pending.svg');globalThis.beforeRetire=[savedImage.width,savedImage.height]").unwrap();
    rt.set_dom(parse_html("<body>new</body>"));rt.run_page_init();
    assert_eq!(rt.evaluate(r#"(() => {
        const before=[savedImage.width,savedImage.height];
        savedImage.src='/inactive.svg';const unchanged=savedImage.width;
        savedImage.src='';return [before[0]===beforeRetire[0],before[1]===beforeRetire[1],unchanged===beforeRetire[0],savedImage.width===beforeRetire[0]];
    })()"#).unwrap(),json!([true,true,true,true]));
    assert!(!rt.has_pending_render_resources());
}

#[tokio::test(flavor = "current_thread")]
async fn image_popover_attribute_paths_preserve_hide_events() {
    let mut rt=runtime("<body><img id='i'></body>");
    rt.execute_script("popover-image",r#"
        globalThis.imagePopoverEvents=[];
        const i=document.getElementById('i');
        for(const type of ['beforetoggle','toggle']) i.addEventListener(type,e=>imagePopoverEvents.push([type,e.oldState,e.newState]));
        globalThis.popoverImage=i;
    "#).unwrap();
    for action in ["popoverImage.setAttribute('popover','manual')",
        "popoverImage.removeAttribute('popover')",
        "Element.prototype.setAttribute.call(popoverImage,'popover','manual')",
        "Element.prototype.removeAttribute.call(popoverImage,'popover')",
        "popoverImage.popover='manual'", "popoverImage.popover=null"] {
        rt.execute_script("show-image","popoverImage.popover='auto';popoverImage.showPopover()").unwrap();
        rt.run_event_loop_bounded(1000).await.unwrap();
        rt.execute_script("reset-image-events","imagePopoverEvents.length=0").unwrap();
        rt.execute_script("change-image-type",action).unwrap();
        assert_eq!(rt.evaluate("imagePopoverEvents").unwrap(),json!([["beforetoggle","open","closed"]]),"{action}");
        rt.run_event_loop_bounded(1000).await.unwrap();
        assert_eq!(rt.evaluate("imagePopoverEvents").unwrap(),json!([["beforetoggle","open","closed"],["toggle","open","closed"]]),"{action}");
        // A second hide must have no additional event after type-change hide.
        rt.execute_script("already-hidden","popoverImage.popover='auto';popoverImage.hidePopover()").unwrap();
        assert_eq!(rt.evaluate("imagePopoverEvents.length").unwrap().as_f64(),Some(2.0));
    }
}

#[cfg(feature = "render")]
#[tokio::test(flavor = "current_thread")]
async fn image_completed_inactive_source_mutations_keep_public_metadata() {
    let mut rt=runtime("<img id='i' style='display:none'>");
    rt.seed_render_image_resource("https://image-fixture.invalid/ready.svg".into(),
        obscura_render::ImageRequestProfile::NoCorsInclude,
        Some(br#"<svg xmlns='http://www.w3.org/2000/svg' width='24' height='12'/>"#.to_vec()));
    rt.execute_script("completed-image","globalThis.savedImage=document.getElementById('i');savedImage.src='/ready.svg'").unwrap();
    rt.run_event_loop_bounded(1000).await.unwrap();
    assert_eq!(rt.evaluate("[savedImage.width,savedImage.height,savedImage.complete,savedImage.naturalWidth]").unwrap(),json!([24,12,true,24]));
    rt.set_dom(parse_html("<body>replacement</body>"));rt.run_page_init();
    for action in ["savedImage.src=''", "savedImage.removeAttribute('src')", "savedImage.src='/inactive.svg'",
        "Element.prototype.setAttributeNS.call(savedImage,null,'src','')"] {
        rt.execute_script("inactive-source",action).unwrap();
        assert_eq!(rt.evaluate("[savedImage.width,savedImage.height,savedImage.complete,savedImage.naturalWidth]").unwrap(),json!([24,12,true,24]),"{action}");
        assert!(!rt.has_pending_render_resources());
    }
}

#[cfg(feature = "render")]
#[test]
fn image_new_document_handoff_gc_reclaims_registry_but_keeps_saved_content() {
    use deno_core::v8;
    use std::sync::Arc;
    let html=format!("<body>{}</body>",(0..512).map(|n|format!("<img id='i{n}' src='/ready.svg' style='display:none'>")).collect::<String>());
    let mut rt=runtime(&html);
    let body: Arc<[u8]>=Arc::from(&b"<svg xmlns='http://www.w3.org/2000/svg' width='24' height='12'/>"[..]);
    let weak_body=Arc::downgrade(&body);
    rt.state.borrow_mut().render_resources.seed_image_shared("https://image-fixture.invalid/ready.svg".into(),
        obscura_render::ImageRequestProfile::NoCorsInclude,body);
    rt.execute_script("bind-image-cohort",r#"for(let n=0;n<512;n++) document.getElementById('i'+n).width;
        globalThis.savedImage=document.getElementById('i0');globalThis.gcImage=document.getElementById('i1');"#).unwrap();
    let storage=rt.state.borrow().original_document.as_ref().unwrap().clone();
    assert_eq!(storage.image_registry_stats().0,512);
    let main=rt.runtime().main_context();
    let weak;
    {
        let mut entered=rt.runtime();
        let scope=&mut v8::HandleScope::with_context(entered.v8_isolate(),main);
        let global=scope.get_current_context().global(scope);
        let key=v8::String::new(scope,"gcImage").unwrap();
        let value=global.get(scope,key.into()).unwrap();
        let object=v8::Local::<v8::Object>::try_from(value).unwrap();
        weak=v8::Weak::new(scope,object);
        assert_eq!(global.delete(scope,key.into()),Some(true));
    }
    // The product's actual document handoff drops old image entries from the
    // strong wrapper cache. Same-document author-reference drop is not claimed.
    rt.set_dom(parse_html("<body>replacement</body>"));rt.run_page_init();
    rt.state.borrow_mut().render_resources=obscura_render::RenderResourceCache::default();
    {
        let mut entered=rt.runtime();
        let scope=&mut v8::HandleScope::new(entered.v8_isolate());
        for _ in 0..3 { scope.low_memory_notification(); }
    }
    assert!(weak.is_empty(),"unreachable old image wrapper must be collected after the host handoff");
    let (live,capacity,reclaimed)=storage.image_registry_stats();
    assert_eq!(live,1);assert!(capacity<=64);assert_eq!(reclaimed,511);
    assert!(weak_body.upgrade().is_some());
    assert_eq!(rt.evaluate("[savedImage.width,savedImage.height]").unwrap(),json!([24,12]));
    rt.execute_script("release-last-image","delete globalThis.savedImage").unwrap();
    {
        let mut entered=rt.runtime();
        let scope=&mut v8::HandleScope::new(entered.v8_isolate());
        for _ in 0..3 { scope.low_memory_notification(); }
    }
    assert_eq!(storage.image_registry_stats().0,0);
    assert!(weak_body.upgrade().is_none(),"retired storage must not retain image content after its last wrapper dies");
}

use super::support::*;

#[tokio::test(flavor = "current_thread")]
async fn inline_wrapper_around_block_content_has_client_geometry() {
    let mut page = input_fixture(r#"<!doctype html>
      <style>body{margin:0;font:16px/20px Arial}#container{width:240px;padding:10px}</style>
      <div id="container"><span id="wrapper"><div style="height:5px"></div><div style="height:10px"></div><div id="last" style="height:15px"></div></span></div>"#).await;
    let geometry = "(()=>{const e=document.querySelector('#wrapper');const r=e.getBoundingClientRect();return {height:r.height,width:r.width,x:r.x,y:r.y,rects:Array.from(e.getClientRects()).map(r=>({height:r.height,width:r.width}))}})()";
    assert_eq!(page.evaluate(geometry), json!({"height":30,"width":240,"x":10,"y":10,
        "rects":[{"height":0,"width":0},{"height":30,"width":240},{"height":0,"width":0}]}));
    page.evaluate("document.querySelector('#last').style.height='35px'");
    assert_eq!(page.evaluate(geometry)["height"], 50);
    page.evaluate("document.querySelector('#container').style.display='none'");
    assert_eq!(page.evaluate(geometry)["height"], 0);
    page.evaluate("document.querySelector('#container').style.display='block'");
    assert_eq!(page.evaluate(geometry)["height"], 50);
    page.evaluate("document.querySelector('#wrapper').style.display='contents'");
    assert_eq!(page.evaluate(geometry)["height"], 0);
}

#[tokio::test(flavor = "current_thread")]
async fn window_open_self_drives_navigation_from_native_submit() {
    let mut page = input_fixture(r#"<!doctype html><base href="http://127.0.0.1/booking/">
      <form id="search" novalidate><button id="submit">Search flights</button></form>"#).await;
    page.js.as_mut().unwrap().execute_script("<search-submit>", r#"
      document.querySelector('#search').addEventListener('submit', event => {
        event.preventDefault();
        window.openedSelf = window.open('flights?route=LGA-LAS', '_SeLf') === window;
      });
    "#).unwrap();
    page.js.as_mut().unwrap().native_click("#submit").unwrap();
    let navigation = page.js.as_ref().unwrap().take_pending_navigation_request()
        .expect("the submit handler must navigate the current window");
    assert_eq!(navigation.url, "http://127.0.0.1/booking/flights?route=LGA-LAS");
    assert_eq!(navigation.method, "GET");
    assert_eq!(page.evaluate("openedSelf"), json!(true));
}

#[tokio::test(flavor = "current_thread")]
async fn window_open_self_empty_url_returns_current_window() {
    let mut page = input_fixture("<!doctype html><body>current window</body>").await;
    assert_eq!(page.evaluate("window.open('', '_self') === window"), json!(true));
    assert!(!page.js.as_ref().unwrap().has_pending_navigation());
    assert_eq!(page.evaluate("(()=>{try { window.open('http://[', '_self'); return false; } catch(e) { return e.name === 'SyntaxError'; }})()"), json!(true));
    assert!(!page.js.as_ref().unwrap().has_pending_navigation());
}

#[tokio::test(flavor = "current_thread")]
async fn window_open_keeps_unsupported_targets_and_features_inactive() {
    let mut page = input_fixture("<!doctype html><body>one window</body>").await;
    assert_eq!(page.evaluate("[window.open('/new'), window.open('/new', '_blank'), window.open('/new', 'other'), window.open('/new', '_self', 'noreferrer')]"), json!([null,null,null,null]));
    assert!(!page.js.as_ref().unwrap().has_pending_navigation());
}

#[tokio::test(flavor = "current_thread")]
async fn navigator_interface_is_exposed_to_page_scripts() {
    let mut page = input_fixture("<!doctype html><body>navigator</body>").await;
    assert_eq!(page.evaluate("typeof window.Navigator"), json!("function"));
    assert_eq!(page.evaluate("navigator instanceof Navigator"), json!(true));
    assert_eq!(page.evaluate("Navigator.prototype.isPrototypeOf(navigator)"), json!(true));
    assert_eq!(page.evaluate("(()=>{try{new Navigator();return false}catch(e){return e instanceof TypeError}})()"), json!(true));
}

#[tokio::test(flavor = "current_thread")]
async fn window_name_exists_as_a_string_in_page_scope() {
    let mut page = input_fixture("<!doctype html><body>name</body>").await;
    assert_eq!(page.evaluate("typeof name"), json!("string"));
    assert_eq!(page.evaluate("[name,window.name]"), json!(["",""]));
    assert_eq!(page.evaluate("(()=>{window.name=42;return [name,window.name]})()"), json!(["42","42"]));
}

#[tokio::test(flavor = "current_thread")]
async fn script_src_reflection_does_not_reenter_page_instrumentation() {
    let mut page = input_fixture("<!doctype html><base href='https://example.com/assets/'><script id='script' type='application/json' src='app.js'></script>").await;
    page.js.as_mut().unwrap().execute_script("<instrumentation>", r#"
      const script=document.querySelector('#script');
      const original=String.prototype.charCodeAt;
      window.sourceReads=0;
      String.prototype.charCodeAt=function(index){sourceReads++;const source=script.src;return original.call(this,index)};
      window.source=script.src;
      String.prototype.charCodeAt=original;
    "#).unwrap();
    assert_eq!(page.evaluate("source"), json!("https://example.com/assets/app.js"));
    assert_eq!(page.evaluate("sourceReads"), json!(0.0));
}

#[tokio::test(flavor = "current_thread")]
async fn html_interfaces_keep_prototype_hooks_on_their_own_elements() {
    let mut page = input_fixture("<!doctype html><input id='input'><button id='button'>Search</button><script id='script' type='application/json'>{}</script>").await;
    assert_eq!(page.evaluate("HTMLScriptElement === HTMLInputElement"), json!(false));
    assert_eq!(page.evaluate("[document.querySelector('#input') instanceof HTMLInputElement, document.querySelector('#input') instanceof HTMLButtonElement, document.querySelector('#script') instanceof HTMLScriptElement, document.createElement('button') instanceof HTMLButtonElement]"), json!([true,false,true,true]));
    page.js.as_mut().unwrap().execute_script("<script-hook>", "Object.defineProperty(HTMLScriptElement.prototype,'src',{get(){return 'script hook'}})").unwrap();
    assert_eq!(page.evaluate("[document.querySelector('#script').src,document.querySelector('#input').src]"), json!(["script hook",""]));
}

#[tokio::test(flavor = "current_thread")]
async fn file_list_is_a_browser_interface_not_an_array() {
    let mut page = input_fixture("<!doctype html><input id='file' type='file'>").await;
    assert_eq!(page.evaluate("typeof FileList"), json!("function"));
    assert_eq!(page.evaluate("(()=>{const f=document.querySelector('#file').files;return [f instanceof FileList,Array.isArray(f),f.length,f.item(0),Array.from(f).length,Object.prototype.toString.call(f)]})()"), json!([true,false,0,null,0,"[object FileList]"]));
    assert_eq!(page.evaluate("(()=>{try{new FileList();return false}catch(e){return e instanceof TypeError}})()"), json!(true));
}

#[tokio::test(flavor = "current_thread")]
async fn svg_script_interface_matches_only_svg_script_nodes() {
    let mut page = input_fixture("<!doctype html><svg><script id='script' type='application/json'>{}</script><circle id='circle'/></svg>").await;
    assert_eq!(page.evaluate("typeof SVGScriptElement"), json!("function"));
    assert_eq!(page.evaluate("[document.querySelector('#script') instanceof SVGScriptElement,document.querySelector('#circle') instanceof SVGScriptElement,document.createElementNS('http://www.w3.org/2000/svg','script') instanceof SVGScriptElement]"), json!([true,false,true]));
}

#[tokio::test(flavor = "current_thread")]
async fn data_transfer_items_preserve_data_and_disable_removed_items() {
    let mut page = input_fixture("<!doctype html><body>transfer</body>").await;
    assert_eq!(page.evaluate("typeof DataTransferItem"), json!("function"));
    page.js.as_mut().unwrap().execute_script("<transfer>", r#"
      const transfer = new DataTransfer();
      const item = transfer.items.add('query', 'TEXT/PLAIN');
      window.transferBefore = [item instanceof DataTransferItem, item.kind, item.type, transfer.getData('text'), transfer.items.length];
      window.duplicate = ''; try {transfer.items.add('duplicate', 'text/plain')} catch(e) {duplicate=e.name}
      window.callbackData = []; item.getAsString(value => callbackData.push(value));
      const file = new File(['abc'], 'query.txt', {type:'text/plain'});
      transfer.items.add(file);
      window.fileResult = [transfer.files instanceof FileList, transfer.files[0].name, transfer.files.item(0).size, transfer.items[1].getAsFile() === file];
      transfer.items.remove(0);
      window.transferAfter = [item.kind, item.type, item.getAsFile(), transfer.getData('text'), transfer.items.length];
      item.getAsString(value => callbackData.push('WRONG'));
    "#).unwrap();
    assert_eq!(page.evaluate("transferBefore"), json!([true,"string","text/plain","query",1]));
    assert_eq!(page.evaluate("duplicate"), json!("NotSupportedError"));
    assert_eq!(page.evaluate("fileResult"), json!([true,"query.txt",3,true]));
    assert_eq!(page.evaluate("transferAfter"), json!(["","",null,"",1]));
    assert_eq!(page.evaluate("callbackData"), json!([]));
    page.settle(20).await;
    assert_eq!(page.evaluate("callbackData"), json!(["query"]));
}

#[tokio::test(flavor = "current_thread")]
async fn chinese_fallback_paints_distinct_glyphs_in_text_and_native_controls() {
    for (html, set) in [
        ("<!doctype html><div id='v' style='font:32px sans-serif'>中</div>", "document.getElementById('v').textContent="),
        ("<!doctype html><input id='v' style='font:32px sans-serif' value='中'>", "document.getElementById('v').value="),
        ("<!doctype html><textarea id='v' style='font:32px sans-serif'>中</textarea>", "document.getElementById('v').value="),
    ] {
        let mut page = input_fixture(html).await;
        let chinese = page.screenshot(page.viewport).unwrap();
        page.js.as_mut().unwrap().execute_script("<fixture>", &format!("{set}'文'")).unwrap();
        let other = page.screenshot(page.viewport).unwrap();
        assert_ne!(chinese, other, "Chinese glyphs must not collapse to the same missing box");
        page.js.as_mut().unwrap().execute_script("<fixture>", &format!("{set}'\\u0378'")).unwrap();
        let missing = page.screenshot(page.viewport).unwrap();
        assert_ne!(chinese, missing);
        assert_ne!(other, missing);
    }
}

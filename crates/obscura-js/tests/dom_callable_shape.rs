// Callable-shape regression coverage; receiver/algorithm conformance is a separate scope.
use obscura_js::runtime::ObscuraJsRuntime;

fn runtime() -> ObscuraJsRuntime {
    let persona = obscura_net::EffectivePersona::builtin(obscura_net::StealthProfile::MacChrome153);
    let mut rt = ObscuraJsRuntime::with_base_url("https://dom-callable.test/page", persona);
    rt.set_dom(obscura_dom::parse_html("<html><body></body></html>"));
    rt.set_url("https://dom-callable.test/page");
    rt.run_page_init();
    rt
}

fn check_shape(expression: &str, name: &str, length: usize) {
    let mut rt = runtime();
    let script = format!(r#"(() => {{
        try {{
            const proto = {expression};
            const descriptor = Object.getOwnPropertyDescriptor(proto, '{name}');
            const fn = descriptor.value;
            const isTypeError = run => {{ try {{ run(); return false; }} catch (e) {{ return e instanceof TypeError; }} }};
            const flags = d => [d.writable, d.enumerable, d.configurable];
            return {{name:fn.name,length:fn.length,
                newThrows:isTypeError(() => new fn()),
                extendsThrows:isTypeError(() => {{ class Derived extends fn {{}} }}),
                // Prove non-constructibility independently of whether the operation body throws.
                newTargetThrows:isTypeError(() => Reflect.construct(function(){{}}, [], fn)),
                ownKeys:Reflect.ownKeys(fn),ownNames:Object.getOwnPropertyNames(fn),
                descriptorKeys:Object.keys(Object.getOwnPropertyDescriptors(fn)),
                noPrototype:!('prototype' in fn),memberFlags:flags(descriptor),
                nameFlags:flags(Object.getOwnPropertyDescriptor(fn,'name')),
                lengthFlags:flags(Object.getOwnPropertyDescriptor(fn,'length')),
                source:Function.prototype.toString.call(fn)}};
        }} catch (e) {{ return {{error:e.name,message:e.message,stack:e.stack}}; }}
    }})()"#);
    let actual = rt.evaluate(&script).unwrap();
    assert_eq!(actual, serde_json::json!({
        "name":name,"length":length,"newThrows":true,"extendsThrows":true,
        "newTargetThrows":true,"ownKeys":["length","name"],"ownNames":["length","name"],
        "descriptorKeys":["length","name"],"noPrototype":true,"memberFlags":[true,true,true],
        "nameFlags":[false,false,true],"lengthFlags":[false,false,true],
        "source":format!("function {name}() {{ [native code] }}")
    }));
}

#[test]
fn document_namespace_operation_has_nonconstructible_webidl_shape() {
    check_shape("Document.prototype", "getElementsByTagNameNS", 2);
}

#[test]
fn element_replace_with_has_nonconstructible_webidl_shape() {
    check_shape("Element.prototype", "replaceWith", 0);
}

#[test]
fn character_data_replace_with_has_nonconstructible_webidl_shape() {
    check_shape("CharacterData.prototype", "replaceWith", 0);
}

#[test]
fn callable_shape_change_preserves_ordinary_namespace_results() {
    let mut rt = runtime();
    let actual = rt.evaluate(r#"(() => {
        try {
            const ns='urn:dom-callable',root=document.createElementNS(ns,'item');
            const nested=document.createElementNS(ns,'item');
            const other=document.createElementNS('urn:other','item');
            root.appendChild(nested);document.body.appendChild(root);document.body.appendChild(other);
            const matches=Document.prototype.getElementsByTagNameNS.call(document,ns,'item');
            const all=Document.prototype.getElementsByTagNameNS.apply(document,['*','item']);
            return {count:matches.length,order:matches[0]===root && matches[1]===nested,
                item:matches.item(1)===nested && matches.item(2)===null,
                wildcard:all.length===3 && all[2]===other,
                documentElement:Array.from(document.getElementsByTagNameNS('*','*')).includes(document.documentElement)};
        } catch(e) { return {error:e.name,message:e.message,stack:e.stack}; }
    })()"#).unwrap();
    assert_eq!(actual,serde_json::json!({"count":2,"order":true,"item":true,"wildcard":true,"documentElement":true}));
}

#[test]
fn callable_shape_change_preserves_connected_childnode_controls() {
    let mut rt = runtime();
    let actual = rt.evaluate(r#"(() => {
        try {
            const host=document.createElement('div');document.body.appendChild(host);
            const a=document.createElement('a');host.appendChild(a);
            a.before('before');a.after('after');
            const b=document.createElement('b');host.replaceChild(b,a);
            const adjacent=host.textContent==='beforeafter' && b.isConnected && !a.isConnected;
            const c=document.createElement('i');c.textContent='C';
            Element.prototype.replaceWith.call(b,'left',c,'right');
            const replace=host.textContent==='beforeleftCrightafter' && !b.isConnected && c.isConnected;
            const text=document.createTextNode('old');host.appendChild(text);
            CharacterData.prototype.replaceWith.apply(text,['new']);
            const character=host.textContent.endsWith('new') && !text.isConnected;
            const comment=document.createComment('anchor');host.appendChild(comment);comment.replaceWith('comment');
            const comments=host.textContent.endsWith('newcomment') && !comment.isConnected;
            const empty=document.createElement('u');host.appendChild(empty);empty.replaceWith();
            const removed=!empty.isConnected;
            const cycleHost=document.createElement('section'),child=document.createElement('span');
            document.body.appendChild(cycleHost);cycleHost.appendChild(child);
            let cycle=false;try { child.replaceWith(cycleHost); } catch(e) { cycle=e.name==='HierarchyRequestError'; }
            const usable=cycleHost.parentNode===document.body && child.parentNode===cycleHost;
            child.after('still usable');
            return {adjacent,replace,character,comments,removed,cycle,usable,
                continued:cycleHost.textContent==='still usable',
                shared:CharacterData.prototype.replaceWith===Element.prototype.replaceWith};
        } catch(e) { return {error:e.name,message:e.message,stack:e.stack}; }
    })()"#).unwrap();
    assert_eq!(actual,serde_json::json!({"adjacent":true,"replace":true,"character":true,
        "comments":true,"removed":true,"cycle":true,"usable":true,"continued":true,"shared":true}));
}

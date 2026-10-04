//! Explicit real native inventory qualification. No playback/permission changes.
use crate::runtime::ObscuraJsRuntime;
use obscura_net::{EffectivePersona, StealthProfile};
use serde_json::json;
use std::time::Instant;

#[cfg(target_os = "macos")]
#[tokio::test(flavor = "current_thread")]
#[ignore = "root explicitly qualifies real native discovery through public Speech bindings"]
async fn speech_inventory_real_native_cold_warm_document_bindings() {
    let mut runtime = ObscuraJsRuntime::new(EffectivePersona::builtin(StealthProfile::MacChrome153));
    let mut observations = Vec::new();
    for stage in ["cold", "warm"] {
        runtime.set_dom(obscura_dom::parse_html("<html><body>Speech inventory qualification</body></html>"));
        runtime.set_url("https://example.test/speech");
        runtime.run_page_init();
        let start = Instant::now();
        let initial = runtime.evaluate(r#"(() => {
            const began = performance.now();
            const service = speechSynthesis;
            globalThis.nativeSpeechEvents = [];
            let previousVoice = null, microtask = 0;
            service.addEventListener('voiceschanged', function(event) {
                const voices = service.getVoices();
                nativeSpeechEvents.push({ trusted: event.isTrusted, receiver: this === service,
                    target: event.target === service, count: voices.length,
                    sinceFirstAccessMs:performance.now()-began, priorMicrotasks:microtask,
                    freshWrappers:previousVoice === null || previousVoice !== voices[0] });
                previousVoice = voices[0] || null;
                Promise.resolve().then(() => { microtask++; });
            });
            const voices = service.getVoices();
            return {count:voices.length, stableService:service === speechSynthesis};
        })()"#).unwrap();
        runtime.run_event_loop().await.unwrap();
        let elapsed_ms = start.elapsed().as_secs_f64() * 1000.0;
        let rows = runtime.evaluate(r#"speechSynthesis.getVoices().map(voice => ({
            voiceURI:voice.voiceURI,name:voice.name,lang:voice.lang,localService:voice.localService,default:voice.default
        }))"#).unwrap();
        let mut subscription = crate::speech_provider::Provider::shared().subscribe().unwrap();
        let snapshot = subscription.ready().await.unwrap();
        let expected = serde_json::to_value(snapshot.voices.iter().map(|voice|voice.web.clone()).collect::<Vec<_>>()).unwrap();
        assert_eq!(rows, expected);
        assert!(!snapshot.voices.is_empty(), "real inventory was empty; this is not nonempty qualification");
        assert_eq!(runtime.evaluate(r#"(() => {
            const a=speechSynthesis.getVoices(), b=speechSynthesis.getVoices();
            const voice=a[0], getter=Object.getOwnPropertyDescriptor(SpeechSynthesisVoice.prototype,'name').get;
            let invalid=false;try{Reflect.apply(getter,{name:voice.name},[]);}catch(e){invalid=e instanceof TypeError;}
            globalThis.savedNativeVoice = voice; globalThis.savedNativeName=voice.name;
            return [a!==b,a[0]===b[0],Object.getPrototypeOf(voice)===SpeechSynthesisVoice.prototype,
                Object.prototype.toString.call(voice)==='[object SpeechSynthesisVoice]',invalid,
                Function.prototype.toString.call(getter).includes('[native code]'),nativeSpeechEvents.length>=1,
                nativeSpeechEvents[0].trusted,nativeSpeechEvents[0].receiver,nativeSpeechEvents[0].target];
        })()"#).unwrap(),json!([true,true,true,true,true,true,true,true,true,true]));
        let events = runtime.evaluate("nativeSpeechEvents").unwrap();
        for (index, event) in events.as_array().unwrap().iter().enumerate() {
            assert_eq!(event["freshWrappers"], json!(true));
            assert_eq!(event["priorMicrotasks"], json!(index));
        }
        observations.push(json!({"stage":stage,"initial":initial,"elapsed_ms":elapsed_ms,"voices":snapshot.voices.len(),"default_branch":snapshot.default_branch,"events":events}));
    }
    runtime.set_dom(obscura_dom::parse_html("<p>replacement without a new inventory request</p>"));
    runtime.set_url("https://example.test/replaced");
    runtime.run_page_init();
    assert_eq!(runtime.evaluate("savedNativeVoice.name === savedNativeName").unwrap(),json!(true));
    eprintln!("NATIVE_SPEECH_BINDINGS={}",serde_json::to_string(&observations).unwrap());
}

use super::*;
use serde_json::{json, Value};
fn source(frequency: f32) -> Value { json!({"kind":"oscillator","shape":"sine","start":0,"params":{"frequency":{"value":frequency}}}) }
fn graph(nodes: Vec<Value>, length: usize, rate: f32, channels: usize) -> Value { json!({"rate":rate,"length":length,"channels":channels,"nodes":nodes}) }
fn run(graph: &Value) -> Value { serde_json::from_str(&render(&graph.to_string()).unwrap()).unwrap() }
fn samples(result: &Value, channel: usize) -> Vec<f32> {
    base64::engine::general_purpose::STANDARD.decode(result["channels"][channel].as_str().unwrap()).unwrap().chunks_exact(4).map(|b| f32::from_le_bytes(b.try_into().unwrap())).collect()
}
fn energy(data: &[f32]) -> f64 { data.iter().map(|x| (*x as f64).powi(2)).sum() }

#[test]
fn audio_empty_disconnected_and_zero_gain_are_silent() {
    for nodes in [vec![json!({"kind":"destination"})], vec![json!({"kind":"destination"}), source(440.0)],
        vec![json!({"kind":"destination","inputs":[2]}), source(440.0), json!({"kind":"gain","inputs":[1],"params":{"gain":{"value":0}}})]] {
        assert!(samples(&run(&graph(nodes, 257, 48000.0, 2)), 0).iter().all(|x| *x == 0.0));
    }
}
#[test]
fn audio_sine_tracks_frequency_rate_channels_and_short_output() {
    for (rate, frequency) in [(44100.0, 440.0), (48000.0, 880.0)] {
        let result = run(&graph(vec![json!({"kind":"destination","inputs":[1]}), source(frequency)], 100, rate, 2));
        let data = samples(&result, 0);
        assert_eq!(data.len(), 100);
        assert_eq!(data, samples(&result, 1));
        for (i, actual) in data.iter().enumerate() {
            let expected = (std::f64::consts::TAU * frequency as f64 * i as f64 / rate as f64).sin();
            assert!((*actual as f64 - expected).abs() < 2e-5, "sample {i}: {actual} != {expected}");
        }
        assert_eq!(result["frames"], 128);
    }
}
#[test]
fn audio_fanout_and_automatic_pull_share_one_source_clock() {
    let base = graph(vec![json!({"kind":"destination","inputs":[1]}), source(1000.0)], 5000, 48000.0, 1);
    let branched = graph(vec![json!({"kind":"destination","inputs":[2,3]}), source(1000.0),
        json!({"kind":"gain","inputs":[1],"params":{"gain":{"value":0.25}}}),
        json!({"kind":"gain","inputs":[1],"params":{"gain":{"value":0.75}}}), json!({"kind":"analyser","inputs":[1]})], 5000, 48000.0, 1);
    let original = samples(&run(&base), 0);
    let result = run(&branched);
    for (a, b) in original.iter().zip(samples(&result, 0)) { assert!((*a - b).abs() < 1e-7); }
    assert_eq!(result["frames"], 5120);
    let history = base64::engine::general_purpose::STANDARD.decode(result["nodes"][4]["history"].as_str().unwrap()).unwrap();
    let last = f32::from_le_bytes(history[history.len() - 4..].try_into().unwrap());
    let expected = (std::f64::consts::TAU * 1000.0 * 5119.0 / 48000.0).sin();
    assert!((last as f64 - expected).abs() < 2e-5);
}
#[test]
fn audio_compressor_lookahead_ratio_and_meter_are_signal_driven() {
    let compressed = |ratio| run(&graph(vec![json!({"kind":"destination","inputs":[2]}), source(1000.0),
        json!({"kind":"compressor","inputs":[1],"params":{"threshold":{"value":-40},"knee":{"value":0},"ratio":{"value":ratio},"attack":{"value":0}}})], 8192, 44100.0, 1));
    let strong = compressed(12.0);
    let weak = compressed(2.0);
    let data = samples(&strong, 0);
    assert!(data[..264].iter().all(|x| *x == 0.0));
    assert!(energy(&data[264..]) > 0.0);
    assert!(strong["nodes"][2]["reduction"].as_f64().unwrap() < -1.0);
    assert_ne!(samples(&weak, 0), data);
    assert_ne!(strong["nodes"][2]["reduction"], weak["nodes"][2]["reduction"]);
}
#[test]
fn audio_start_stop_and_sample_automation_gate_real_frames() {
    let mut oscillator = source(500.0); oscillator["start"] = json!(0.01); oscillator["stop"] = json!(0.03);
    let result = run(&graph(vec![json!({"kind":"destination","inputs":[2]}), oscillator,
        json!({"kind":"gain","inputs":[1],"params":{"gain":{"value":0,"events":[{"kind":"set","time":0.02,"value":1}]}}})], 2000, 48000.0, 1));
    let data = samples(&result, 0);
    assert!(data[..960].iter().all(|x| *x == 0.0));
    assert!(energy(&data[960..1440]) > 1.0);
    assert!(data[1440..].iter().all(|x| *x == 0.0));
}
#[test]
fn audio_analyser_fft_finds_input_frequency_and_silence() {
    let n = 2048;
    let input: Vec<f32> = (0..n).map(|i| (std::f64::consts::TAU * 32.0 * i as f64 / n as f64).sin() as f32).collect();
    let bins = fft::magnitudes(&input);
    let peak = bins.iter().enumerate().max_by(|a,b| a.1.total_cmp(b.1)).unwrap().0;
    assert_eq!(peak, 32);
    assert!((bins[peak] - 0.21).abs() < 1e-5);
    assert!(fft::magnitudes(&vec![0.0; n]).iter().all(|x| *x == 0.0));
}
#[test]
fn audio_graph_budgets_and_cycles_fail_explicitly() {
    for input in [graph(vec![json!({"kind":"destination"})], MAX_SAMPLES + 1, 48000.0, 1),
        graph(vec![json!({"kind":"destination","inputs":[1]}), json!({"kind":"gain","inputs":[1]})], 128, 48000.0, 1),
        graph(vec![json!({"kind":"destination"}), json!({"kind":"biquad"})], 128, 48000.0, 1),
        graph(vec![json!({"kind":"destination","inputs":[99]})], 128, 48000.0, 1)] {
        assert!(render(&input.to_string()).is_err());
    }
    assert!(analyse(&[0.0; 33]).is_err());
}
fn runtime() -> crate::runtime::ObscuraJsRuntime {
    let mut runtime = crate::runtime::ObscuraJsRuntime::new(obscura_net::EffectivePersona::builtin(obscura_net::StealthProfile::MacChrome153));
    runtime.set_dom(obscura_dom::parse_html("<html></html>"));
    runtime.set_url("https://example.com/audio"); runtime.run_page_init(); runtime
}
#[test]
fn audio_buffer_copy_preserves_float_bits_and_untouched_tail() {
    let mut runtime = runtime();
    assert_eq!(runtime.evaluate(r#"(() => {
        const b = new AudioBuffer({length: 3, sampleRate: 48000});
        const src = new Float32Array([-0, NaN, 2]); b.copyToChannel(src, 0);
        const out = new Float32Array([9,9,9,9,9]); b.copyFromChannel(out, 0);
        const tail = new Float32Array([7,8]); b.copyFromChannel(tail, 0, 10);
        let bad = false, empty = false, branded = false;
        try { b.getChannelData(1); } catch(e) { bad = e.name === 'IndexSizeError'; }
        try { new AudioBuffer({length: 0, sampleRate: 48000}); } catch(e) { empty = e.name === 'NotSupportedError'; }
        try { AudioBuffer.prototype.getChannelData.call(Object.create(AudioBuffer.prototype), 0); } catch(e) { branded = e instanceof TypeError; }
        return [Object.is(out[0], -0), Number.isNaN(out[1]), out[2], out[3], out[4], ...tail, bad, empty, branded];
    })()"#).unwrap(), json!([true,true,2,9,9,7,8,true,true,true]));
}
#[tokio::test(flavor = "current_thread")]
async fn audio_offline_lifecycle_graph_disconnect_and_analyser() {
    let mut runtime = runtime();
    runtime.execute_script("audio-lifecycle", r#"
        globalThis.audioResult = null;
        (async () => {
            const c = new OfflineAudioContext(2, 5000, 48000), o = c.createOscillator(), g = c.createGain(), a = c.createAnalyser();
            o.frequency.value = 1500; a.fftSize = 1024; a.smoothingTimeConstant = 0;
            const chain = o.connect(g) === g && g.connect(c.destination) === c.destination;
            g.disconnect(c.destination); o.connect(a); o.start();
            let completed, events = [];
            c.oncomplete = e => { completed = e.renderedBuffer; events.push('complete'); };
            const initial = c.state, p = c.startRendering(), during = c.state;
            const repeated = await c.startRendering().then(() => false, e => e.name === 'InvalidStateError');
            const b = await p; events.push('promise');
            const frequency = new Float32Array(a.frequencyBinCount), again = new Float32Array(a.frequencyBinCount), times = new Float32Array(a.fftSize);
            a.getFloatFrequencyData(frequency); a.getFloatFrequencyData(again); a.getFloatTimeDomainData(times);
            const peak = frequency.indexOf(Math.max(...frequency));
            audioResult = [chain, initial, during, c.state, c.currentTime, b.length, b.sampleRate, b.numberOfChannels,
                b.getChannelData(0).every(x => x === 0), completed === b, repeated, events.join(','), peak,
                frequency.every((x,i) => Object.is(x, again[i])), times.some(x => x !== 0),
                c instanceof AudioContext, c instanceof BaseAudioContext];
        })().catch(e => { audioResult = String(e.stack); });
    "#).unwrap();
    runtime.run_event_loop_bounded(5000).await.unwrap();
    assert_eq!(runtime.evaluate("audioResult").unwrap(), json!([true,"suspended","running","closed",5120.0/48000.0,5000,48000,2,true,true,true,"complete,promise",32,true,true,false,true]));
}
#[test]
fn audio_webidl_graph_validation_and_silent_analyser() {
    let mut runtime = runtime();
    assert_eq!(runtime.evaluate(r#"(() => {
        const c = new OfflineAudioContext(1,128,44100), other = new OfflineAudioContext(1,128,44100), o = c.createOscillator(), a = c.createAnalyser();
        const errors = [];
        for (const f of [() => o.connect(other.destination), () => o.connect(c.destination, 1), () => a.fftSize = 33,
            () => o.frequency.setValueAtTime(NaN,0), () => o.start(-1), () => c.createBufferSource(),
            () => new OfflineAudioContext(1, 1000000000, 44100)]) { try { f(); errors.push('none'); } catch(e) { errors.push(e.name); } }
        o.start(); try { o.start(); } catch(e) { errors.push(e.name); }
        const frequency = new Float32Array(1026).fill(9), time = new Float32Array(2050).fill(9);
        a.getFloatFrequencyData(frequency); a.getFloatTimeDomainData(time);
        return [errors, frequency.slice(0,1024).every(x => x === -Infinity), frequency[1024], time.slice(0,2048).every(x => x === 0), time[2048]];
    })()"#).unwrap(), json!([["InvalidAccessError","IndexSizeError","IndexSizeError","TypeError","RangeError","NotSupportedError","NotSupportedError","InvalidStateError"],true,9,true,9]));
}
#[test]
fn audio_cancelled_job_and_work_budget_do_not_render() {
    let input = graph(vec![json!({"kind":"destination","inputs":[1]}), source(440.0)], 128, 44100.0, 1);
    assert!(render_cancellable(&input.to_string(), &AtomicBool::new(true)).unwrap_err().starts_with("AbortError"));
    let mut nodes = vec![json!({"kind":"destination","inputs":[1]}), source(440.0)];
    for _ in 0..32 { nodes.push(json!({"kind":"analyser","inputs":[1]})); }
    let input = graph(nodes, 1_000_000, 48000.0, 2);
    assert!(render(&input.to_string()).unwrap_err().contains("work budget"));
}
#[test]
fn audio_band_limited_triangle_changes_spectrum_without_aliasing_partials() {
    let result = run(&graph(vec![json!({"kind":"destination","inputs":[1]}),
        json!({"kind":"oscillator","shape":"triangle","start":0,"params":{"frequency":{"value":12000}}})], 4096, 48000.0, 1));
    let data = samples(&result, 0);
    let bins = fft::magnitudes(&data);
    assert!(bins[1024] > 0.1);
    // Nyquist excludes the next triangle harmonic; the retained wave is a sine.
    for i in 1..1022 { assert!(bins[i] < 1e-5); }
    assert!(data[0].abs() < 1e-5);
    assert!(data[1] > 0.0);
}
#[test]
fn audio_fft_byte_boundary_preserves_offset_and_rejects_invalid_sizes() {
    let input: Vec<f32> = (0..128).map(|i| (std::f64::consts::TAU * 8.0 * i as f64 / 128.0).sin() as f32).collect();
    let mut storage = vec![0x7f; 3];
    storage.extend(input.iter().flat_map(|value| value.to_ne_bytes()));
    storage.extend([0x55; 5]);
    let bins: Vec<f32> = serde_json::from_str(&analyse_bytes(&storage[3..515]).unwrap()).unwrap();
    assert_eq!(bins.iter().enumerate().max_by(|a,b| a.1.total_cmp(b.1)).unwrap().0, 8);
    assert!((bins[8] - 0.21).abs() < 1e-5);
    for length in [0, 4, 127, 129, 131076] {
        assert!(analyse_bytes(&vec![0; length]).is_err());
    }
}
#[tokio::test(flavor = "current_thread")]
async fn audio_active_offline_snapshot_rejects_graph_mutations() {
    let mut runtime = runtime();
    runtime.execute_script("audio-active-graph", r#"
        globalThis.audioMutationResult = null;
        (async () => {
            const c = new OfflineAudioContext(1,128,48000), o = c.createOscillator();
            o.frequency.value = 1000; o.connect(c.destination); o.start();
            const rendering = c.startRendering(), errors = [];
            for (const edit of [() => o.type = 'triangle', () => o.channelCount = 1,
                () => c.createGain(), () => new OscillatorNode(c), () => o.frequency.value = 2000,
                () => o.frequency.setValueAtTime(2000, 0), () => o.disconnect(), () => o.stop()]) {
                try { edit(); errors.push('accepted'); } catch(e) { errors.push(e.name); }
            }
            const b = await rendering;
            const sine = b.getChannelData(0).every((x,i) => Math.abs(x - Math.sin(2 * Math.PI * 1000 * i / 48000)) < 2e-5);
            audioMutationResult = [errors, o.type, o.channelCount, o.frequency.value, sine, c.state];
        })().catch(e => { audioMutationResult = String(e.stack); });
    "#).unwrap();
    runtime.run_event_loop_bounded(5000).await.unwrap();
    assert_eq!(runtime.evaluate("audioMutationResult").unwrap(), json!([
        vec!["NotSupportedError"; 8], "sine", 2, 1000, true, "closed"
    ]));
}
#[test]
fn audio_fractional_source_start_and_stop_use_the_sample_clock() {
    for (rate, frequency, detune, start_frame, stop_frame) in [
        (48000.0_f64, 6000.0_f64, 0.0_f64, 0.5_f64, 40.25_f64),
        (48000.0, -6000.0, 0.0, 127.5, 190.25),
        (44100.0, 1000.0, 1200.0, 2.5, 200.25),
    ] {
        let result = run(&graph(vec![json!({"kind":"destination","inputs":[1]}),
            json!({"kind":"oscillator","shape":"sine","start":start_frame / rate,"stop":stop_frame / rate,
                "params":{"frequency":{"value":frequency},"detune":{"value":detune}}})], 256, rate as f32, 1));
        let effective = frequency * 2.0_f64.powf(detune / 1200.0);
        for (i, actual) in samples(&result, 0).into_iter().enumerate() {
            let expected = if (i as f64) < start_frame || (i as f64) >= stop_frame.ceil() { 0.0 }
                else { (std::f64::consts::TAU * effective * (i as f64 - start_frame) / rate).sin() };
            assert!((actual as f64 - expected).abs() < 2e-5, "frame {i}: {actual} != {expected}");
        }
    }
}
#[test]
fn audio_scheduled_parameter_values_obey_nominal_ranges_in_dsp() {
    let render_with = |frequency: f32, threshold: f32| run(&graph(vec![json!({"kind":"destination","inputs":[2]}),
        json!({"kind":"oscillator","shape":"sine","start":0,"params":{"frequency":{"value":440,"events":[{"kind":"set","value":frequency,"time":0}]}}}),
        json!({"kind":"compressor","inputs":[1],"params":{"threshold":{"value":-24,"events":[{"kind":"set","value":threshold,"time":0}]}}})], 512, 48000.0, 1));
    assert_eq!(samples(&render_with(48000.0, -200.0), 0), samples(&render_with(24000.0, -100.0), 0));
    assert_eq!(samples(&render_with(1000.0, 20.0), 0), samples(&render_with(1000.0, 0.0), 0));
}
#[test]
fn audio_parameter_public_values_clamp_and_use_finite_float_conversion() {
    let mut runtime = runtime();
    assert_eq!(runtime.evaluate(r#"(() => {
        const c = new OfflineAudioContext(1,128,48000), o = c.createOscillator(), comp = c.createDynamicsCompressor();
        o.frequency.value = 48000; comp.threshold.value = -200; comp.ratio.value = 100;
        const direct = [o.frequency.value, o.frequency.maxValue, comp.threshold.value, comp.ratio.value];
        o.frequency.setValueAtTime(-48000, 0); comp.threshold.setValueAtTime(20, 0);
        const scheduled = [o.frequency.value, comp.threshold.value];
        const rejected = [Infinity, NaN, 1e100].every(value => { try { o.frequency.value = value; return false; } catch(e) { return e instanceof TypeError; } });
        comp.attack.value = 0.123456789;
        return [direct, scheduled, rejected, comp.attack.value === Math.fround(0.123456789),
            comp.attack.defaultValue === Math.fround(0.003), comp.attack.minValue === Math.fround(0), comp.attack.maxValue === Math.fround(1)];
    })()"#).unwrap(), json!([[24000,24000,-100,20],[-24000,0],true,true,true,true,true]));
}

#[test]
fn audio_biquad_connected_graph_uses_real_filter_and_persistent_channels() {
    for rate in [44100.0,48000.0,96000.0] {
        let reference = samples(&run(&graph(vec![json!({"kind":"destination","inputs":[1]}),source(1000.0)],5000,rate,2)),0);
        for shape in biquad::TYPES {
            let result=run(&graph(vec![json!({"kind":"destination","inputs":[2]}),source(1000.0),
                json!({"kind":"biquad","shape":shape,"inputs":[1],"params":{"frequency":{"value":2000},"Q":{"value":1},"gain":{"value":6}}})],5000,rate,2));
            let data=samples(&result,0);assert_eq!(data.len(),5000);assert_eq!(data,samples(&result,1));
            assert!(data.iter().all(|x| x.is_finite()));assert_ne!(data,reference);assert_eq!(result["frames"],5120);
        }
        let unity=run(&graph(vec![json!({"kind":"destination","inputs":[2]}),source(1000.0),
            json!({"kind":"biquad","shape":"lowpass","inputs":[1],"params":{"frequency":{"value":rate/2.0}}})],5000,rate,2));
        assert_eq!(samples(&unity,0),reference);
    }
    let silent=run(&graph(vec![json!({"kind":"destination","inputs":[1]}),json!({"kind":"biquad","shape":"lowpass"})],257,48000.0,1));
    assert!(samples(&silent,0).iter().all(|x| *x==0.0));
}

#[test]
fn audio_biquad_a_rate_and_k_rate_use_sample_and_quantum_clocks() {
    let reference=samples(&run(&graph(vec![json!({"kind":"destination","inputs":[1]}),source(1000.0)],257,48000.0,1)),0);
    for (rate, boundary) in [("a-rate",64),("k-rate",128)] {
        let result=run(&graph(vec![json!({"kind":"destination","inputs":[2]}),source(1000.0),
            json!({"kind":"biquad","shape":"lowshelf","inputs":[1],"params":{
                "frequency":{"value":24000},"gain":{"value":0,"rate":rate,"events":[{"kind":"set","time":64.0/48000.0,"value":6.020599913}]}}})],257,48000.0,1));
        for (i,x) in samples(&result,0).iter().enumerate() { let expected=reference[i]*if i<boundary {1.0} else {2.0}; assert!((*x-expected).abs()<2e-6,"{rate} sample {i}"); }
        assert_eq!(result["frames"],384);
    }
}

#[test]
fn audio_biquad_response_byte_views_nan_and_budget_are_bounded() {
    let request=json!({"shape":"lowpass","rate":48000,"values":[24000,0,1,0]}).to_string();
    let mut bytes=vec![0x55;3];for hz in [0.0_f32,1000.0,24000.0,-1.0,24001.0,f32::NAN] {bytes.extend(hz.to_ne_bytes());}
    let result: Vec<String>=serde_json::from_str(&biquad_response(&request,&bytes[3..]).unwrap()).unwrap();
    let decode=|s:&str| base64::engine::general_purpose::STANDARD.decode(s).unwrap().chunks_exact(4).map(|b|f32::from_le_bytes(b.try_into().unwrap())).collect::<Vec<_>>();
    let magnitude=decode(&result[0]);let phase=decode(&result[1]);assert_eq!(&magnitude[..3],&[1.0;3]);assert!(phase[..3].iter().all(|x|x.abs()<1e-6));
    assert!(magnitude[3..].iter().chain(&phase[3..]).all(|x|x.is_nan()));
    assert!(biquad_response(&request,&[0;3]).is_err());assert!(biquad_response(&request,&vec![0;131076]).is_err());
    assert_eq!(biquad_response(&request,&[]).unwrap(),"[\"\",\"\"]");
}

#[test]
fn audio_biquad_keeps_graph_budget_cycle_and_cancellation_guards() {
    let base=|length|graph(vec![json!({"kind":"destination","inputs":[2]}),source(1000.0),json!({"kind":"biquad","shape":"lowpass","inputs":[1]})],length,48000.0,1);
    assert!(render(&base(4_000_000).to_string()).unwrap_err().contains("work budget"));
    assert!(render_cancellable(&base(128).to_string(),&AtomicBool::new(true)).unwrap_err().contains("AbortError"));
    let cycle=graph(vec![json!({"kind":"destination","inputs":[1]}),json!({"kind":"biquad","shape":"lowpass","inputs":[1]})],128,48000.0,1);
    assert!(render(&cycle.to_string()).unwrap_err().contains("cycles"));
    for patch in [json!({"shape":"unsupported"}),json!({"params":{"frequency":{"value":350,"rate":"unsupported"}}})] {
        let mut g=base(128);for (key,value) in patch.as_object().unwrap(){g["nodes"][2][key]=value.clone();}assert!(render(&g.to_string()).is_err());
    }
}

#[test]
fn audio_biquad_public_metadata_response_and_validation() {
    let mut runtime=runtime();
    let result=runtime.evaluate(r#"(()=>{
        const c=new OfflineAudioContext(1,256,48000), f=c.createBiquadFilter();
        const rejects=(fn,name)=>{try{fn();return false}catch(e){return e.name===name}};
        const defaults=f.type==='lowpass'&&f.frequency.value===350&&f.frequency.minValue===0&&f.frequency.maxValue===24000&&f.Q.defaultValue===1&&f.gain.defaultValue===0&&f.gain.maxValue===Math.fround(40*Math.fround(Math.log10(Math.fround(3.4028234663852886e38))))&&f.detune.minValue===-153600&&f.detune.maxValue===153600;
        const rates=['frequency','detune','Q','gain'].every(key=>{const p=f[key];if(p.automationRate!=='a-rate')return false;p.automationRate='k-rate';return p.automationRate==='k-rate'});
        const metadata=BiquadFilterNode.length===1&&BiquadFilterNode.prototype.getFrequencyResponse.length===3&&Object.prototype.toString.call(f)==='[object BiquadFilterNode]'&&['type','frequency','detune','Q','gain'].every(key=>{const d=Object.getOwnPropertyDescriptor(BiquadFilterNode.prototype,key);return d.enumerable&&d.get.name==='get '+key&&Function.prototype.toString.call(d.get)==='function get '+key+'() { [native code] }'});
        const getter=Object.getOwnPropertyDescriptor(BiquadFilterNode.prototype,'frequency').get;
        const branded=[{},Object.create(BiquadFilterNode.prototype),c.createGain(),new Proxy(f,{})].every(x=>rejects(()=>getter.call(x),'TypeError'));
        for(let i=0;i<80;i++) if(!rejects(()=>new BiquadFilterNode(c,{type:'invalid'}),'TypeError'))return false;
        const validAfterFailures=new BiquadFilterNode(c,{type:'highpass',frequency:0}) instanceof BiquadFilterNode;
        f.frequency.value=24000;const hz=new Float32Array([0,1000,24000,-1,NaN]),m=new Float32Array(5),p=new Float32Array(5);
        f.getFrequencyResponse(hz,m,p);const response=m.slice(0,3).every(x=>x===1)&&m.slice(3).every(Number.isNaN)&&p.slice(3).every(Number.isNaN)&&c.currentTime===0;
        const alias=new Float32Array([0,1000,24000]),phase=new Float32Array(3);f.getFrequencyResponse(alias,alias,phase);
        const rejected=rejects(()=>f.getFrequencyResponse(hz,new Float32Array(4),p),'InvalidAccessError')&&rejects(()=>f.getFrequencyResponse([],m,p),'TypeError')&&rejects(()=>f.type='invalid','TypeError')&&rejects(()=>f.Q.value=Infinity,'TypeError')&&rejects(()=>f.Q.value=1n,'TypeError')&&rejects(()=>new BiquadFilterNode(c,{channelCountMode:'explicit'}),'NotSupportedError');
        f.Q.setTargetAtTime(2,0.01,0.01);const unsupported=rejects(()=>f.Q.linearRampToValueAtTime(3,0.02),'NotSupportedError');
        f.getFrequencyResponse(new Float32Array(),new Float32Array(),new Float32Array());
        return [defaults,rates,metadata,branded,validAfterFailures,response,alias.every(x=>x===1),rejected&&unsupported];
    })()"#).unwrap();
    assert_eq!(result,json!([true,true,true,true,true,true,true,true]));
}

#[tokio::test(flavor = "current_thread")]
async fn audio_biquad_offline_mutations_reject_without_changing_snapshot() {
    let mut runtime=runtime();
    runtime.execute_script("biquad-active",r#"
        globalThis.biquadResult=null;
        (async()=>{const c=new OfflineAudioContext(1,256,48000),o=c.createOscillator(),f=new BiquadFilterNode(c,{frequency:24000});o.connect(f);f.connect(c.destination);o.start();const promise=c.startRendering(),names=[];
            for(const change of [()=>f.type='highpass',()=>f.frequency.value=0,()=>f.Q.automationRate='k-rate',()=>new BiquadFilterNode(c)]){try{change();names.push('accepted')}catch(e){names.push(e.name)}}
            const b=await promise;biquadResult=[names,f.type,f.frequency.value,f.Q.automationRate,b.getChannelData(0).some(x=>x!==0),c.state];
        })().catch(e=>biquadResult=String(e.stack));
    "#).unwrap();
    runtime.run_event_loop_bounded(5000).await.unwrap();
    assert_eq!(runtime.evaluate("biquadResult").unwrap(),json!([["NotSupportedError","NotSupportedError","NotSupportedError","NotSupportedError"],"lowpass",24000,"a-rate",true,"closed"]));
}

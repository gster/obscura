//! Bounded offline Web Audio graph processing, independent of the render feature.
//! Every reachable node is evaluated once per 128-frame quantum, including
//! automatic-pull analysers. No persona, site, fingerprint or expected hash inputs.
mod biquad;
mod compressor;
mod fft;
mod oscillator;
use base64::Engine;
use serde::{Deserialize, Serialize};
use std::collections::BTreeMap;
use std::time::{Duration, Instant};
use std::sync::atomic::{AtomicBool, Ordering};
const QUANTUM: usize = 128;
const MAX_NODES: usize = 64;
const MAX_SAMPLES: usize = 8_388_608;
const MAX_WORK: usize = 64_000_000;

#[derive(Clone, Deserialize)]
struct Event { kind: String, time: f64, value: f32, #[serde(default)] constant: f64 }
#[derive(Clone, Deserialize)]
struct Param { value: f32, #[serde(default)] events: Vec<Event>, #[serde(default)] rate: String }
impl Param {
    fn at(&self, time: f64) -> f32 {
        let mut previous_time = 0.0;
        let mut previous = self.value;
        let mut target: Option<(f32, f64)> = None;
        for event in &self.events {
            if time < event.time {
                if event.kind == "linear" || event.kind == "exponential" {
                    let fraction = ((time - previous_time) / (event.time - previous_time)).clamp(0.0, 1.0);
                    return if event.kind == "linear" { (previous as f64 + fraction * (event.value as f64 - previous as f64)) as f32 }
                        else if previous != 0.0 && previous.signum() == event.value.signum() { (previous as f64 * (event.value as f64 / previous as f64).powf(fraction)) as f32 }
                        else { previous };
                }
                break;
            }
            if let Some((value, constant)) = target {
                previous = (value as f64 + (previous as f64 - value as f64) * (-(event.time - previous_time) / constant).exp()) as f32;
            }
            if event.kind == "target" { target = Some((event.value, event.constant)); }
            else { previous = event.value; target = None; }
            previous_time = event.time;
        }
        if let Some((value, constant)) = target { (value as f64 + (previous as f64 - value as f64) * (-(time - previous_time) / constant).exp()) as f32 } else { previous }
    }
}
#[derive(Deserialize)]
struct Node {
    kind: String,
    #[serde(default)] inputs: Vec<usize>,
    #[serde(default)] params: BTreeMap<String, Param>,
    #[serde(default)] shape: String,
    #[serde(default)] start: Option<f64>,
    #[serde(default)] stop: Option<f64>,
}
impl Node {
    fn param(&self, key: &str, time: f64, default: f32) -> f32 { self.params.get(key).map_or(default, |p| p.at(time)) }
}
#[derive(Deserialize)]
struct Graph { rate: f32, channels: usize, length: usize, nodes: Vec<Node> }
#[derive(Serialize)]
struct NodeResult { reduction: f32, history: Option<String> }
#[derive(Serialize)]
struct RenderResult { channels: Vec<String>, nodes: Vec<NodeResult>, frames: usize }
fn encoded(values: &[f32]) -> String {
    let bytes: Vec<u8> = values.iter().flat_map(|x| x.to_le_bytes()).collect();
    base64::engine::general_purpose::STANDARD.encode(bytes)
}
// Chromium audio_utilities::TimeToSampleFrame uses a 1024x sample grid to
// avoid spuriously rounding an integer k/rate schedule into the next frame.
// See LICENSE.chromium and the upstream revision cited in oscillator.rs.
fn sample_frame(time: f64, rate: f32) -> usize {
    ((time * rate as f64 * 1024.0).round() / 1024.0).ceil() as usize
}
fn visit(id: usize, graph: &Graph, status: &mut [u8], order: &mut Vec<usize>) -> Result<(), String> {
    if status[id] == 2 { return Ok(()); }
    if status[id] == 1 { return Err("NotSupportedError: audio graph cycles require DelayNode support".into()); }
    status[id] = 1;
    for &input in &graph.nodes[id].inputs { visit(input, graph, status, order)?; }
    status[id] = 2;
    order.push(id);
    Ok(())
}
fn validate(graph: &Graph) -> Result<(), String> {
    if !graph.rate.is_finite() || !(8000.0..=96000.0).contains(&graph.rate) || !(1..=2).contains(&graph.channels)
        || graph.length == 0 || graph.length.checked_mul(graph.channels).is_none_or(|n| n > MAX_SAMPLES)
        || graph.nodes.is_empty() || graph.nodes.len() > MAX_NODES || graph.nodes[0].kind != "destination" {
        return Err("NotSupportedError: offline audio format or allocation budget exceeded".into());
    }
    let mut cost = graph.nodes.len();
    for node in &graph.nodes {
        if !matches!(node.kind.as_str(), "destination" | "oscillator" | "gain" | "compressor" | "analyser" | "biquad") {
            return Err("NotSupportedError: unsupported audio node".into());
        }
        if node.kind == "oscillator" && !matches!(node.shape.as_str(), "sine" | "triangle" | "square" | "sawtooth") {
            return Err("NotSupportedError: unsupported oscillator waveform".into());
        }
        if node.kind == "biquad" {
            if !biquad::TYPES.contains(&node.shape.as_str()) { return Err("NotSupportedError: unsupported biquad type".into()); }
            // Charge coefficient work as well as five filter multiplies. This
            // also bounds dense a-rate automation under the existing deadline.
            cost += 16;
            if node.params.iter().any(|(key, p)| !matches!(key.as_str(), "frequency" | "detune" | "Q" | "gain") || !matches!(p.rate.as_str(), "" | "a-rate" | "k-rate")) {
                return Err("NotSupportedError: unsupported biquad parameter".into());
            }
            if node.params.values().any(|p| p.events.windows(2).any(|events| events[0].kind == "target" && matches!(events[1].kind.as_str(), "linear" | "exponential"))) {
                return Err("NotSupportedError: target-to-ramp biquad automation is not implemented".into());
            }
        }
        if node.inputs.len() > MAX_NODES || node.inputs.iter().any(|&id| id >= graph.nodes.len()) { return Err("InvalidAccessError: invalid audio edge".into()); }
        cost += node.inputs.len();
        if node.params.len() > 8 { return Err("NotSupportedError: parameter budget exceeded".into()); }
        for param in node.params.values() {
            if !param.value.is_finite() || param.events.len() > 128 { return Err("NotSupportedError: parameter budget exceeded".into()); }
            cost += param.events.len();
            let mut last = 0.0;
            for event in &param.events {
                if !event.time.is_finite() || event.time < last || !event.value.is_finite()
                    || !matches!(event.kind.as_str(), "set" | "linear" | "exponential" | "target")
                    || (event.kind == "target" && (!event.constant.is_finite() || event.constant <= 0.0)) {
                    return Err("TypeError: invalid audio automation".into());
                }
                last = event.time;
            }
        }
        if node.start.into_iter().chain(node.stop).any(|t| !t.is_finite() || t < 0.0) { return Err("RangeError: invalid source time".into()); }
    }
    if graph.length.div_ceil(QUANTUM).checked_mul(QUANTUM).and_then(|n| n.checked_mul(graph.channels)).and_then(|n| n.checked_mul(cost)).is_none_or(|n| n > MAX_WORK) {
        return Err("NotSupportedError: offline audio work budget exceeded".into());
    }
    Ok(())
}

#[cfg(test)]
fn render(request: &str) -> Result<String, String> { render_cancellable(request, &AtomicBool::new(false)) }

pub(crate) fn render_cancellable(request: &str, cancelled: &AtomicBool) -> Result<String, String> {
    if request.len() > 1_048_576 { return Err("NotSupportedError: audio graph request budget exceeded".into()); }
    let graph: Graph = serde_json::from_str(request).map_err(|e| format!("TypeError: invalid audio graph: {e}"))?;
    validate(&graph)?;
    let deadline = Instant::now() + Duration::from_secs(3);
    let count = graph.nodes.len();
    let mut order = Vec::new();
    let mut status = vec![0; count];
    visit(0, &graph, &mut status, &mut order)?;
    for (id, node) in graph.nodes.iter().enumerate() {
        if node.kind == "analyser" { visit(id, &graph, &mut status, &mut order)?; }
    }
    let mut waves = BTreeMap::new();
    for &id in &order {
        if cancelled.load(Ordering::Relaxed) || Instant::now() > deadline { return Err("AbortError: offline audio initialization cancelled".into()); }
        let node = &graph.nodes[id];
        if node.kind == "oscillator" && !waves.contains_key(&node.shape) {
            waves.insert(node.shape.clone(), oscillator::Wave::new(&node.shape, graph.rate));
        }
    }
    let mut phases = vec![0.0_f64; count];
    let mut started = vec![false; count];
    let mut compressors: Vec<_> = graph.nodes.iter().map(|node| (node.kind == "compressor").then(|| compressor::Compressor::new(graph.channels, graph.rate))).collect();
    let mut filters: Vec<_> = graph.nodes.iter().map(|node| (node.kind == "biquad").then(biquad::Filter::new)).collect();
    let mut histories: Vec<_> = graph.nodes.iter().map(|node| if node.kind == "analyser" { vec![0.0; 32768] } else { Vec::new() }).collect();
    let mut output = Vec::with_capacity(graph.channels);
    for _ in 0..graph.channels {
        let mut channel = Vec::new();
        channel.try_reserve_exact(graph.length).map_err(|_| "NotSupportedError: audio output allocation failed".to_string())?;
        channel.resize(graph.length, 0.0);
        output.push(channel);
    }
    let mut blocks = vec![vec![vec![0.0; QUANTUM]; graph.channels]; count];
    let rendered_frames = graph.length.div_ceil(QUANTUM) * QUANTUM;
    for frame in (0..rendered_frames).step_by(QUANTUM) {
        if cancelled.load(Ordering::Relaxed) { return Err("AbortError: offline audio context was destroyed".into()); }
        if Instant::now() > deadline { return Err("NotSupportedError: offline audio deadline exceeded".into()); }
        for &id in &order {
            let node = &graph.nodes[id];
            for channel in &mut blocks[id] { channel.fill(0.0); }
            for &input in &node.inputs {
                // Splitting permits mixing without cloning an entire source block.
                let (source, destination) = if input < id {
                    let (left, right) = blocks.split_at_mut(id); (&left[input], &mut right[0])
                } else {
                    let (left, right) = blocks.split_at_mut(input); (&right[0], &mut left[id])
                };
                for (src, dst) in source.iter().zip(destination.iter_mut()) { for (s, d) in src.iter().zip(dst) { *d += *s; } }
            }
            let block = &mut blocks[id];
            match node.kind.as_str() {
                "oscillator" => {
                    let start_position = node.start.map(|start| start * graph.rate as f64);
                    let first_frame = node.start.map(|start| {
                        let scheduled = sample_frame(start, graph.rate);
                        if start * graph.rate as f64 > scheduled as f64 { scheduled.saturating_add(1) } else { scheduled }
                    });
                    let stop_frame = node.stop.map(|stop| sample_frame(stop, graph.rate));
                    for i in 0..QUANTUM {
                        let sample_frame = frame + i;
                        let time = sample_frame as f64 / graph.rate as f64;
                        if first_frame.is_some_and(|start| sample_frame >= start) && stop_frame.is_none_or(|stop| sample_frame < stop) {
                            let frequency = node.param("frequency", time, 440.0).clamp(-graph.rate / 2.0, graph.rate / 2.0);
                            let detune = node.param("detune", time, 0.0).clamp(-153600.0, 153600.0);
                            let frequency = (frequency as f64 * 2.0_f64.powf(detune as f64 / 1200.0)).clamp(-(graph.rate as f64) / 2.0, graph.rate as f64 / 2.0) as f32;
                            if !started[id] {
                                let elapsed_frames = sample_frame as f64 - start_position.unwrap();
                                phases[id] = (elapsed_frames * frequency as f64 / graph.rate as f64).rem_euclid(1.0);
                                started[id] = true;
                            }
                            let sample = waves[&node.shape].sample(phases[id], frequency, graph.rate);
                            for channel in block.iter_mut() { channel[i] = sample; }
                            phases[id] = (phases[id] + frequency as f64 / graph.rate as f64).rem_euclid(1.0);
                        }
                    }
                }
                "gain" => for i in 0..QUANTUM {
                    let gain = node.param("gain", (frame + i) as f64 / graph.rate as f64, 1.0).clamp(f32::MIN, f32::MAX);
                    for channel in block.iter_mut() { channel[i] *= gain; }
                },
                "biquad" => {
                    let filter = filters[id].as_mut().unwrap();
                    for i in 0..QUANTUM {
                        let value = |key: &str, default: f32| node.params.get(key).map_or(default, |p| {
                            p.at((frame + if p.rate == "k-rate" { 0 } else { i }) as f64 / graph.rate as f64)
                        });
                        filter.configure(&node.shape, graph.rate, [value("frequency", 350.0), value("detune", 0.0), value("Q", 1.0), value("gain", 0.0)])?;
                        for (channel, data) in block.iter_mut().enumerate() { data[i] = filter.sample(channel, data[i])?; }
                    }
                    filter.end_quantum();
                }
                "compressor" => {
                    let time = frame as f64 / graph.rate as f64;
                    let values = [node.param("threshold", time, -24.0).clamp(-100.0, 0.0), node.param("knee", time, 30.0).clamp(0.0, 40.0), node.param("ratio", time, 12.0).clamp(1.0, 20.0), node.param("attack", time, 0.003).clamp(0.0, 1.0), node.param("release", time, 0.25).clamp(0.0, 1.0)];
                    compressors[id].as_mut().unwrap().process(block, graph.rate, values);
                }
                "analyser" => for i in 0..QUANTUM {
                    histories[id][(frame + i) % 32768] = block.iter().map(|ch| ch[i]).sum::<f32>() / graph.channels as f32;
                },
                _ => {}
            }
        }
        let frames = QUANTUM.min(graph.length - frame);
        for (channel, block) in output.iter_mut().zip(&blocks[0]) { channel[frame..frame + frames].copy_from_slice(&block[..frames]); }
    }
    let nodes = histories.iter().enumerate().map(|(id, history)| {
        let history = if history.is_empty() { None } else {
            let values: Vec<f32> = (0..32768).map(|i| history[(rendered_frames + i) % 32768]).collect();
            Some(encoded(&values))
        };
        NodeResult { reduction: compressors[id].as_ref().map_or(0.0, |c| if status[id] == 2 { c.meter } else { 0.0 }), history }
    }).collect();
    serde_json::to_string(&RenderResult { channels: output.iter().map(|ch| encoded(ch)).collect(), nodes, frames: rendered_frames }).map_err(|e| e.to_string())
}

#[derive(Deserialize)]
struct BiquadResponse { shape: String, rate: f32, values: [f32; 4] }
// A separate immutable coefficient snapshot: querying a response never advances
// graph history. Native Float32 bytes preserve NaN and avoid alignment casts.
pub(crate) fn biquad_response(request: &str, input: &[u8]) -> Result<String, String> {
    if request.len() > 1024 || input.len() > 131072 || input.len() % 4 != 0 {
        return Err("NotSupportedError: biquad response budget exceeded".into());
    }
    let request: BiquadResponse = serde_json::from_str(request).map_err(|_| "TypeError: invalid biquad response request".to_string())?;
    let coefficients = biquad::Coefficients::new(&request.shape, request.rate, request.values)?;
    let mut magnitude = Vec::with_capacity(input.len() / 4);
    let mut phase = Vec::with_capacity(input.len() / 4);
    for bytes in input.chunks_exact(4) {
        let (m, p) = coefficients.response(f32::from_ne_bytes([bytes[0], bytes[1], bytes[2], bytes[3]]), request.rate);
        magnitude.push(m); phase.push(p);
    }
    serde_json::to_string(&[encoded(&magnitude), encoded(&phase)]).map_err(|e| e.to_string())
}

// deno_core 0.350 exposes Uint8Array slices at the op boundary. The JS view
// covers exactly fftSize Float32 elements, including its original byte offset.
// Decode without alignment casts; V8 typed-array storage uses native endianness.
pub(crate) fn analyse_bytes(input: &[u8]) -> Result<String, String> {
    if !(128..=131072).contains(&input.len()) || !input.len().is_power_of_two() {
        return Err("IndexSizeError: invalid FFT byte length".into());
    }
    let values: Vec<f32> = input.chunks_exact(4).map(|bytes| {
        f32::from_ne_bytes([bytes[0], bytes[1], bytes[2], bytes[3]])
    }).collect();
    analyse(&values)
}

pub(crate) fn analyse(input: &[f32]) -> Result<String, String> {
    if !(32..=32768).contains(&input.len()) || !input.len().is_power_of_two() { return Err("IndexSizeError: invalid FFT size".into()); }
    let magnitudes = fft::magnitudes(input);
    serde_json::to_string(&magnitudes).map_err(|e| e.to_string())
}

#[cfg(test)]
mod tests;

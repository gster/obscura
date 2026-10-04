//! Real second-order filters. Coefficients/endpoint conventions adapted from
//! Chromium 153.0.8010.53 (792bf6722e73a45aa9e47c163b9901bdc17f3230),
//! platform/audio/biquad.cc and modules/webaudio/biquad_filter_handler.cc.
//! See LICENSE.chromium. Portable f64 coefficients/f32 feedback are not a claim
//! of bit identity with macOS Accelerate or Chromium's fdlibm implementation.
use std::f64::consts::PI;

pub(super) const TYPES: [&str; 8] = ["lowpass", "highpass", "bandpass", "lowshelf", "highshelf", "peaking", "notch", "allpass"];
pub(super) fn max_gain() -> f32 { 40.0 * f32::MAX.log10() }

#[derive(Clone, Copy, Debug)]
pub(super) struct Coefficients { b0: f64, b1: f64, b2: f64, a1: f64, a2: f64 }
impl Coefficients {
    fn constant(gain: f64) -> Self { Self { b0: gain, b1: 0.0, b2: 0.0, a1: 0.0, a2: 0.0 } }
    fn normalized(b0: f64, b1: f64, b2: f64, a0: f64, a1: f64, a2: f64) -> Self {
        let inverse = 1.0 / a0;
        Self { b0: b0 * inverse, b1: b1 * inverse, b2: b2 * inverse, a1: a1 * inverse, a2: a2 * inverse }
    }
    pub(super) fn new(shape: &str, rate: f32, values: [f32; 4]) -> Result<Self, String> {
        let [frequency, detune, q, gain] = values;
        if !rate.is_finite() || !(8000.0..=96000.0).contains(&rate) || values.iter().any(|v| !v.is_finite()) {
            return Err("TypeError: invalid biquad parameters".into());
        }
        let frequency = ((frequency.clamp(0.0, rate / 2.0) as f64 / (rate as f64 / 2.0))
            * 2.0_f64.powf((detune.clamp(-153600.0, 153600.0) / 1200.0) as f64)).clamp(0.0, 1.0);
        let q = q as f64;
        let gain = gain.clamp(f32::MIN, max_gain()) as f64;
        let theta = PI * frequency;
        let (sin, cos) = theta.sin_cos();
        let normalized = Self::normalized;
        let constant = Self::constant;
        let result = match shape {
            "lowpass" | "highpass" => {
                let low = shape == "lowpass";
                if frequency == 0.0 { constant(if low { 0.0 } else { 1.0 }) }
                else if frequency == 1.0 { constant(if low { 1.0 } else { 0.0 }) }
                else {
                    // Web Audio low/high-pass Q is resonance in dB, unlike the
                    // linear quality factor used by bandpass/peaking/notch.
                    let alpha = sin / (2.0 * 10.0_f64.powf(q / 20.0));
                    let beta = if low { (1.0 - cos) / 2.0 } else { (1.0 + cos) / 2.0 };
                    normalized(beta, if low { 2.0 * beta } else { -2.0 * beta }, beta, 1.0 + alpha, -2.0 * cos, 1.0 - alpha)
                }
            }
            "lowshelf" | "highshelf" => {
                let low = shape == "lowshelf";
                let a = 10.0_f64.powf(gain / 40.0);
                if frequency == 0.0 { constant(if low { 1.0 } else { a * a }) }
                else if frequency == 1.0 { constant(if low { a * a } else { 1.0 }) }
                else {
                    let alpha = sin * std::f64::consts::SQRT_2 / 2.0;
                    let k2 = 2.0 * a.sqrt() * alpha;
                    let plus = a + 1.0; let minus = a - 1.0;
                    if low { normalized(a * (plus - minus * cos + k2), 2.0 * a * (minus - plus * cos), a * (plus - minus * cos - k2), plus + minus * cos + k2, -2.0 * (minus + plus * cos), plus + minus * cos - k2) }
                    else { normalized(a * (plus + minus * cos + k2), -2.0 * a * (minus + plus * cos), a * (plus + minus * cos - k2), plus - minus * cos + k2, 2.0 * (minus - plus * cos), plus - minus * cos - k2) }
                }
            }
            "peaking" | "allpass" | "notch" | "bandpass" => {
                if frequency == 0.0 || frequency == 1.0 { constant(if shape == "bandpass" { 0.0 } else { 1.0 }) }
                else if q <= 0.0 {
                    constant(match shape { "peaking" => 10.0_f64.powf(gain / 20.0), "allpass" => -1.0, "notch" => 0.0, _ => 1.0 })
                } else {
                    let alpha = sin / (2.0 * q);
                    match shape {
                        "peaking" => { let a = 10.0_f64.powf(gain / 40.0); normalized(1.0 + alpha * a, -2.0 * cos, 1.0 - alpha * a, 1.0 + alpha / a, -2.0 * cos, 1.0 - alpha / a) }
                        "allpass" => normalized(1.0 - alpha, -2.0 * cos, 1.0 + alpha, 1.0 + alpha, -2.0 * cos, 1.0 - alpha),
                        "notch" => normalized(1.0, -2.0 * cos, 1.0, 1.0 + alpha, -2.0 * cos, 1.0 - alpha),
                        _ => normalized(alpha, 0.0, -alpha, 1.0 + alpha, -2.0 * cos, 1.0 - alpha),
                    }
                }
            }
            _ => return Err("NotSupportedError: unsupported biquad type".into()),
        };
        if [result.b0, result.b1, result.b2, result.a1, result.a2].iter().any(|x| !x.is_finite()) {
            return Err("NotSupportedError: biquad coefficients exceed finite numerical range".into());
        }
        Ok(result)
    }
    pub(super) fn response(&self, hz: f32, rate: f32) -> (f32, f32) {
        // Normalize in f32, as Blink's temporary frequency array does. Input
        // outside [0, Nyquist] (including NaN) produces NaN in both outputs.
        let frequency = hz / (rate * 0.5);
        if !(0.0..=1.0).contains(&frequency) { return (f32::NAN, f32::NAN); }
        let (zi, zr) = (-PI * frequency as f64).sin_cos();
        let z2r = zr * zr - zi * zi; let z2i = 2.0 * zr * zi;
        let nr = self.b0 + self.b1 * zr + self.b2 * z2r;
        let ni = self.b1 * zi + self.b2 * z2i;
        let dr = 1.0 + self.a1 * zr + self.a2 * z2r;
        let di = self.a1 * zi + self.a2 * z2i;
        let denominator = dr * dr + di * di;
        let real = (nr * dr + ni * di) / denominator;
        let imaginary = (ni * dr - nr * di) / denominator;
        (real.hypot(imaginary) as f32, imaginary.atan2(real) as f32)
    }
}

#[derive(Clone, Copy, Default)]
struct History { x1: f64, x2: f64, y1: f64, y2: f64 }
pub(super) struct Filter { channels: [History; 2], last: Option<[f32; 4]>, coefficients: Coefficients }
impl Filter {
    pub(super) fn new() -> Self { Self { channels: [History::default(); 2], last: None, coefficients: Coefficients::constant(1.0) } }
    pub(super) fn configure(&mut self, shape: &str, rate: f32, values: [f32; 4]) -> Result<(), String> {
        // A graph snapshot cannot change type/rate. Reuse coefficients for
        // static runs; automation recomputes only when the actual values differ.
        if self.last != Some(values) { self.coefficients = Coefficients::new(shape, rate, values)?; self.last = Some(values); }
        Ok(())
    }
    pub(super) fn sample(&mut self, channel: usize, input: f32) -> Result<f32, String> {
        let h = &mut self.channels[channel]; let c = self.coefficients;
        let output = (c.b0 * input as f64 + c.b1 * h.x1 + c.b2 * h.x2 - c.a1 * h.y1 - c.a2 * h.y2) as f32;
        if !output.is_finite() { return Err("NotSupportedError: biquad signal exceeds finite numerical range".into()); }
        h.x2 = h.x1; h.x1 = input as f64; h.y2 = h.y1; h.y1 = output as f64;
        Ok(output)
    }
    pub(super) fn end_quantum(&mut self) {
        for h in &mut self.channels {
            for value in [&mut h.x1, &mut h.x2, &mut h.y1, &mut h.y2] { if value.abs() < f32::MIN_POSITIVE as f64 { *value = 0.0; } }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    fn near(a: f64, b: f64, tolerance: f64) { assert!((a - b).abs() < tolerance, "{a} != {b}"); }
    #[test]
    fn biquad_response_obeys_analytic_transfer_identities() {
        let rate = 48000.0;
        for shape in TYPES {
            let c = Coefficients::new(shape, rate, [6000.0, 0.0, 1.0, 12.0]).unwrap();
            let dc = c.response(0.0, rate).0 as f64;
            let nyquist = c.response(rate / 2.0, rate).0 as f64;
            let center = c.response(6000.0, rate).0 as f64;
            match shape {
                "lowpass" => { near(dc, 1.0, 1e-6); near(nyquist, 0.0, 1e-6); near(center, 10.0_f64.powf(1.0/20.0), 1e-6); }
                "highpass" => { near(dc, 0.0, 1e-6); near(nyquist, 1.0, 1e-6); }
                "bandpass" => { near(dc, 0.0, 1e-6); near(nyquist, 0.0, 1e-6); near(center, 1.0, 1e-6); }
                "notch" => { near(dc, 1.0, 1e-6); near(nyquist, 1.0, 1e-6); near(center, 0.0, 1e-6); }
                "peaking" => { near(dc, 1.0, 1e-6); near(nyquist, 1.0, 1e-6); near(center, 10.0_f64.powf(12.0/20.0), 1e-6); }
                "lowshelf" => { near(dc, 10.0_f64.powf(12.0/20.0), 1e-6); near(nyquist, 1.0, 1e-6); }
                "highshelf" => { near(dc, 1.0, 1e-6); near(nyquist, 10.0_f64.powf(12.0/20.0), 1e-6); }
                "allpass" => for hz in [0.0, 500.0, 6000.0, 10000.0, 24000.0] { near(c.response(hz, rate).0 as f64, 1.0, 1e-6); },
                _ => unreachable!(),
            }
        }
    }
    #[test]
    fn biquad_impulse_dft_matches_response_for_every_type_and_rate() {
        // Independent transfer check: numerically sum the actual causal impulse
        // output against complex exponentials, rather than expected sample data.
        for rate in [44100.0, 48000.0, 96000.0] {
            for shape in TYPES {
                let values = [4000.0, 0.0, 1.0, 6.0];
                let c = Coefficients::new(shape, rate, values).unwrap();
                let mut f = Filter::new(); f.configure(shape, rate, values).unwrap();
                let mut output = Vec::new();
                for i in 0..4096 {
                    output.push(f.sample(0, if i == 0 { 1.0 } else { 0.0 }).unwrap());
                    assert_eq!(f.sample(1, 0.0).unwrap(), 0.0, "channel history leaked");
                    if i % 128 == 127 { f.end_quantum(); }
                }
                for hz in [1000.0, 4000.0, 12000.0] {
                    let (mut real, mut imaginary) = (0.0, 0.0);
                    for (i, &y) in output.iter().enumerate() { let omega = -2.0 * PI * hz as f64 * i as f64 / rate as f64; real += y as f64 * omega.cos(); imaginary += y as f64 * omega.sin(); }
                    let (m, p) = c.response(hz, rate);
                    near(real.hypot(imaginary), m as f64, 3e-5);
                    if m > 1e-5 { near((imaginary.atan2(real) - p as f64 + PI).rem_euclid(2.0 * PI) - PI, 0.0, 3e-5); }
                }
            }
        }
    }
    #[test]
    fn biquad_silence_step_unity_and_state_cross_quantum() {
        for shape in TYPES {
            let mut f = Filter::new(); f.configure(shape, 48000.0, [2000.0, 0.0, 1.0, 0.0]).unwrap();
            for i in 0..512 { assert_eq!(f.sample(0, 0.0).unwrap(), 0.0); if i % 128 == 127 { f.end_quantum(); } }
        }
        for (shape, target) in [("lowpass", 1.0), ("highpass", 0.0)] {
            let mut f = Filter::new(); f.configure(shape, 48000.0, [2000.0, 0.0, 1.0, 0.0]).unwrap();
            let mut last = 0.0;
            for i in 0..4096 { last = f.sample(0, 1.0).unwrap(); if i % 128 == 127 { f.end_quantum(); } }
            near(last as f64, target, 2e-6);
        }
        for (shape, frequency) in [("lowpass",24000.0),("highpass",0.0),("peaking",0.0)] {
            let mut f = Filter::new(); f.configure(shape, 48000.0, [frequency, 0.0, 1.0, 0.0]).unwrap();
            for x in [1.0, -0.5, 0.25, 0.0] { assert_eq!(f.sample(0,x).unwrap(),x); }
        }
        let mut a = Filter::new(); let mut b = Filter::new();
        for f in [&mut a,&mut b] { f.configure("lowpass",48000.0,[2000.0,0.0,1.0,0.0]).unwrap(); }
        for i in 0..1024 { let x = if i == 127 {1.0} else {0.0}; near(a.sample(0,x).unwrap() as f64,b.sample(0,x).unwrap() as f64,1e-35); if i % 128 == 127 { b.end_quantum(); } }
    }
    #[test]
    fn biquad_frequency_edges_detune_and_extremes_are_explicit() {
        for shape in TYPES {
            for frequency in [0.0, 24000.0] {
                for q in [-1.0, 0.0, 1.0] {
                    let c=Coefficients::new(shape,48000.0,[frequency,0.0,q,0.0]).unwrap();
                    assert!(c.response(1000.0,48000.0).0.is_finite());
                }
            }
            let a=Coefficients::new(shape,48000.0,[2000.0,1200.0,1.0,6.0]).unwrap();
            let b=Coefficients::new(shape,48000.0,[4000.0,0.0,1.0,6.0]).unwrap();
            assert_eq!(a.response(3000.0,48000.0),b.response(3000.0,48000.0));
            for hz in [-1.0,24001.0,f32::NAN,f32::INFINITY] { let (m,p)=a.response(hz,48000.0); assert!(m.is_nan() && p.is_nan()); }
        }
        for (shape, expected) in [("bandpass",1.0),("notch",0.0),("allpass",1.0),("peaking",1.0)] {
            let c=Coefficients::new(shape,48000.0,[2000.0,0.0,0.0,0.0]).unwrap(); near(c.response(1000.0,48000.0).0 as f64,expected,1e-6);
        }
        assert!(Coefficients::new("lowpass",48000.0,[1000.0,0.0,-f32::MAX,0.0]).is_err());
        assert!(Coefficients::new("unsupported",48000.0,[1000.0,0.0,1.0,0.0]).is_err());
        let mut f=Filter::new(); f.configure("lowshelf",48000.0,[24000.0,0.0,1.0,1000.0]).unwrap(); assert!(f.sample(0,1.0).is_err());
    }
}

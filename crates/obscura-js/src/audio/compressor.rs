// Port of Chromium 153 dynamics_compressor.cc, revision
// 583c5b4655acea7450a3b5224ea5121ae8caca9b. Copyright (C) 2011 Google Inc.
// Redistribution terms and disclaimer are in LICENSE.chromium.
// Rust libm is used instead of Chromium fdlibm; bitwise identity is not promised.
use super::QUANTUM;
fn db(x: f32) -> f32 { (20.0 * (x as f64).log10()) as f32 }
fn linear(x: f32) -> f32 { 10.0_f64.powf(x as f64 * 0.05) as f32 }
fn finite(x: f32, fallback: f32) -> f32 { if x.is_finite() { x } else { fallback } }
fn flush(x: f32) -> f32 { if x.is_subnormal() { 0.0 } else { x } }
struct Curve { threshold: f32, knee_threshold: f32, db_knee: f32, y_knee: f32, slope: f32, k: f32 }
impl Curve {
    fn knee(&self, x: f32, k: f32) -> f32 {
        if x < self.threshold { x } else { self.threshold + (1.0 - ((-k * (x - self.threshold)) as f64).exp() as f32) / k }
    }
    fn new(threshold: f32, knee: f32, ratio: f32) -> Self {
        let mut c = Self { threshold: linear(threshold), knee_threshold: linear(threshold + knee), db_knee: threshold + knee, y_knee: 0.0, slope: 1.0 / ratio, k: 5.0 };
        let x = c.knee_threshold;
        let x2 = (x as f64 * 1.001) as f32;
        let mut low = 0.1;
        let mut high = 10000.0;
        for _ in 0..15 {
            let slope = (db(c.knee(x2, c.k)) - db(c.knee(x, c.k))) / (db(x2) - db(x));
            if slope < c.slope { high = c.k; } else { low = c.k; }
            c.k = (low * high).sqrt();
        }
        c.y_knee = db(c.knee(x, c.k));
        c
    }
    fn saturate(&self, x: f32) -> f32 {
        if x < self.knee_threshold { self.knee(x, self.k) } else { linear(self.y_knee + self.slope * (db(x) - self.db_knee)) }
    }
}

pub(super) struct Compressor {
    delay: Vec<Vec<f32>>, read: usize, write: usize,
    detector: f32, gain: f32, pub meter: f32, max_attack: f32,
}
impl Compressor {
    pub fn new(channels: usize, rate: f32) -> Self {
        Self { delay: vec![vec![0.0; 1024]; channels], read: 0, write: ((0.006_f32 * rate) as usize).min(1023), detector: 0.0, gain: 1.0, meter: 1.0, max_attack: -1.0 }
    }
    pub fn process(&mut self, data: &mut [Vec<f32>], rate: f32, params: [f32; 5]) {
        let [threshold, knee, ratio, attack, release] = params;
        let curve = Curve::new(threshold, knee, ratio);
        let post = (1.0 / curve.saturate(1.0)).powf(0.6);
        let attack_frames = attack.max(0.001) * rate;
        let release_frames = release * rate;
        let sat_frames = 0.0025 * rate;
        let meter_release = (1.0 - (-1.0 / (0.325 * rate as f64)).exp()) as f32;
        let zones = [0.09_f32, 0.16, 0.42, 0.98];
        let [z1, z2, z3, z4] = zones;
        let a = release_frames * (0.9999999999999998_f32 * z1 + 1.8432219684323923e-16_f32 * z2 - 1.9373394351676423e-16_f32 * z3 + 8.824516011816245e-18_f32 * z4);
        let b = release_frames * (-1.5788320352845888_f32 * z1 + 2.3305837032074286_f32 * z2 - 0.9141194204840429_f32 * z3 + 0.1623677525612032_f32 * z4);
        let c = release_frames * (0.5334142869106424_f32 * z1 - 1.272736789213631_f32 * z2 + 0.9258856042207512_f32 * z3 - 0.18656310191776226_f32 * z4);
        let d = release_frames * (0.08783463138207234_f32 * z1 - 0.1694162967925622_f32 * z2 + 0.08588057951595272_f32 * z3 - 0.00429891410546283_f32 * z4);
        let e = release_frames * (-0.042416883008123074_f32 * z1 + 0.1115693827987602_f32 * z2 - 0.09764676325265872_f32 * z3 + 0.028494263462021576_f32 * z4);
        for division in (0..QUANTUM).step_by(32) {
            self.detector = finite(self.detector, 1.0);
            let desired = self.detector.asin() / std::f32::consts::FRAC_PI_2;
            let releasing = desired > self.gain;
            let difference = if desired == 0.0 { if releasing { -1.0 } else { 1.0 } } else { db(self.gain / desired) };
            let envelope = if releasing {
                self.max_attack = -1.0;
                let x = 0.25 * (finite(difference, -1.0).clamp(-12.0, 0.0) + 12.0);
                let x2 = x * x;
                let frames = a + b * x + c * x2 + d * x2 * x + e * x2 * x2;
                linear(5.0 / frames)
            } else {
                let difference = finite(difference, 1.0);
                self.max_attack = self.max_attack.max(difference);
                1.0 - (0.25 / self.max_attack.max(0.5)).powf(1.0 / attack_frames)
            };
            for frame in division..division + 32 {
                let mut peak = 0.0_f32;
                for (channel, delay) in data.iter().zip(&mut self.delay) {
                    delay[self.write] = channel[frame];
                    peak = peak.max(channel[frame].abs());
                }
                let attenuation = if peak <= 0.0001 { 1.0 } else { curve.saturate(peak) / peak };
                let detector_rate = if attenuation > self.detector { linear((-db(attenuation)).max(2.0) / sat_frames) - 1.0 } else { 1.0 };
                self.detector = finite((self.detector + (attenuation - self.detector) * detector_rate).min(1.0), 1.0);
                if envelope < 1.0 { self.gain += (desired - self.gain) * envelope; }
                else { self.gain = (self.gain * envelope).min(1.0); }
                let warped = ((std::f32::consts::FRAC_PI_2 * self.gain) as f64).sin() as f32;
                let real_db = db(warped);
                if real_db < self.meter { self.meter = real_db; }
                else { self.meter += (real_db - self.meter) * meter_release; }
                for (channel, delay) in data.iter_mut().zip(&self.delay) { channel[frame] = delay[self.read] * (post * warped); }
                self.read = (self.read + 1) & 1023;
                self.write = (self.write + 1) & 1023;
            }
            self.detector = flush(self.detector);
            self.gain = flush(self.gain);
        }
    }
}

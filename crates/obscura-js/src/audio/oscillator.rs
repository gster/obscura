// Band-limited tables follow Chromium 153 PeriodicWave's public algorithm.
// Derived from third_party/blink/renderer/modules/webaudio/periodic_wave.cc,
// revision 583c5b4655acea7450a3b5224ea5121ae8caca9b. See LICENSE.chromium.
// Portable FFT arithmetic intentionally does not claim Accelerate bit parity.
use super::fft;

pub(super) struct Wave { tables: Vec<Vec<f32>>, lowest: f32 }
impl Wave {
    pub fn new(shape: &str, rate: f32) -> Self {
        let size: usize = if rate <= 24000.0 { 2048 } else if rate <= 88200.0 { 4096 } else { 16384 };
        let ranges = 3 * size.ilog2() as usize;
        let mut tables = Vec::with_capacity(ranges);
        let mut normalization = 1.0;
        for range in 0..ranges {
            let partials = ((size / 2) as f64 * 2.0_f64.powf(-(range as f64) / 3.0)) as usize;
            let mut real = vec![0.0; size];
            let mut imag = vec![0.0; size];
            for n in 1..=(partials.min(size / 2 - 1)) {
                let p = 2.0 / (n as f32 * std::f32::consts::PI);
                let coefficient = match shape {
                    "sine" => if n == 1 { 1.0 } else { 0.0 },
                    "square" => if n & 1 == 1 { 2.0 * p } else { 0.0 },
                    "sawtooth" => p * if n & 1 == 1 { 1.0 } else { -1.0 },
                    "triangle" => if n & 1 == 1 { 2.0 * p * p * if ((n - 1) / 2) & 1 == 1 { -1.0 } else { 1.0 } } else { 0.0 },
                    _ => 0.0,
                };
                imag[n] = -coefficient * size as f32 * 0.5;
                imag[size - n] = -imag[n];
            }
            fft::transform(&mut real, &mut imag, true);
            if range == 0 {
                let peak = real.iter().fold(0.0_f32, |a, x| a.max(x.abs()));
                if peak > 0.0 { normalization = 1.0 / peak; }
            }
            for value in &mut real { *value *= normalization; }
            tables.push(real);
        }
        Self { tables, lowest: rate / size as f32 }
    }

    pub fn sample(&self, phase: f64, frequency: f32, rate: f32) -> f32 {
        let ratio = if frequency != 0.0 { frequency.abs() / self.lowest } else { 0.5 };
        let pitch = (1.0 + 3.0 * ratio.log2()).clamp(0.0, (self.tables.len() - 1) as f32);
        let high = pitch as usize;
        let low = (high + 1).min(self.tables.len() - 1);
        let factor = pitch - high as f32;
        let size = self.tables[0].len();
        let position = phase * size as f64;
        let index = position.floor() as usize;
        let fraction = (position - position.floor()) as f32;
        let increment = frequency.abs() * size as f32 / rate;
        let interpolate = |table: &[f32]| {
            if increment >= 0.3 {
                let a = table[index & (size - 1)];
                a + fraction * (table[(index + 1) & (size - 1)] - a)
            } else {
                // Lagrange interpolation for slowly moving table positions.
                let points: &[isize] = if increment >= 0.16 { &[-1, 0, 1] } else { &[-2, -1, 0, 1, 2] };
                let mut sum = 0.0;
                for &k in points {
                    let mut weight = 1.0;
                    for &j in points { if j != k { weight *= (fraction - j as f32) / (k - j) as f32; } }
                    sum += weight * table[(index.wrapping_add_signed(k)) & (size - 1)];
                }
                sum
            }
        };
        let a = interpolate(&self.tables[high]);
        a + factor * (interpolate(&self.tables[low]) - a)
    }
}

// Portable radix-2 transform. No platform FFT or render feature is required.
pub(super) fn transform(real: &mut [f32], imag: &mut [f32], inverse: bool) {
    let n = real.len();
    let mut j = 0;
    for i in 1..n {
        let mut bit = n >> 1;
        while j & bit != 0 { j ^= bit; bit >>= 1; }
        j ^= bit;
        if i < j { real.swap(i, j); imag.swap(i, j); }
    }
    let mut size = 2;
    while size <= n {
        let angle = if inverse { 2.0 } else { -2.0 } * std::f64::consts::PI / size as f64;
        for base in (0..n).step_by(size) {
            for k in 0..size / 2 {
                let (sin, cos) = (angle * k as f64).sin_cos();
                let (sin, cos) = (sin as f32, cos as f32);
                let a = base + k;
                let b = a + size / 2;
                let re = real[b] * cos - imag[b] * sin;
                let im = real[b] * sin + imag[b] * cos;
                real[b] = real[a] - re;
                imag[b] = imag[a] - im;
                real[a] += re;
                imag[a] += im;
            }
        }
        size *= 2;
    }
    if inverse {
        for x in real.iter_mut().chain(imag.iter_mut()) { *x /= n as f32; }
    }
}

pub(super) fn magnitudes(input: &[f32]) -> Vec<f32> {
    let n = input.len();
    let mut real: Vec<f32> = input.iter().enumerate().map(|(i, x)| {
        let phase = 2.0 * std::f64::consts::PI * i as f64 / n as f64;
        (*x as f64 * (0.42 - 0.5 * phase.cos() + 0.08 * (2.0 * phase).cos())) as f32
    }).collect();
    let mut imag = vec![0.0; n];
    transform(&mut real, &mut imag, false);
    (0..n / 2).map(|i| ((real[i] as f64).hypot(imag[i] as f64) / n as f64) as f32).collect()
}

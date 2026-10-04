// Exact saved Chrome153 CDP B observations, not production-model expectations.
// 48 rows x empty/Mg/j; see geometry evidence manifest for immutable original SHA.
use super::*;
#[cfg(target_os="macos")]
use crate::canvas_font_geometry::ParsedCanvasFont;

#[cfg(target_os = "macos")]
#[test]
fn canvas_geometry_matches_saved_144_same_byte_samples() {
    let mut engine = CanvasTextEngine::new();
    let cases: &[(&str, f32, &str, &str, [f32;7])] = &[
        ("Liberation Sans", 10.0, "alphabetic", "", [0.0, 0.0, 0.0, -0.0, 0.0, 9.0, 2.0]),
        ("Liberation Sans", 10.0, "alphabetic", "Mg", [13.8916015625, -0.8203125, 13.2470703125, 6.8798828125, 2.0751953125, 9.0, 2.0]),
        ("Liberation Sans", 10.0, "alphabetic", "j", [2.2216796875, 0.244140625, 1.5478515625, 7.24609375, 2.0751953125, 9.0, 2.0]),
        ("Liberation Sans", 10.0, "top", "", [0.0, 0.0, 0.0, -7.75, 7.75, 1.25, 9.75]),
        ("Liberation Sans", 10.0, "top", "Mg", [13.8916015625, -0.8203125, 13.2470703125, -0.8701171875, 9.8251953125, 1.25, 9.75]),
        ("Liberation Sans", 10.0, "top", "j", [2.2216796875, 0.244140625, 1.5478515625, -0.50390625, 9.8251953125, 1.25, 9.75]),
        ("Liberation Sans", 10.0, "middle", "", [0.0, 0.0, 0.0, -2.75, 2.75, 6.25, 4.75]),
        ("Liberation Sans", 10.0, "middle", "Mg", [13.8916015625, -0.8203125, 13.2470703125, 4.1298828125, 4.8251953125, 6.25, 4.75]),
        ("Liberation Sans", 10.0, "middle", "j", [2.2216796875, 0.244140625, 1.5478515625, 4.49609375, 4.8251953125, 6.25, 4.75]),
        ("Liberation Sans", 10.0, "bottom", "", [0.0, 0.0, 0.0, 2.25, -2.25, 11.25, -0.25]),
        ("Liberation Sans", 10.0, "bottom", "Mg", [13.8916015625, -0.8203125, 13.2470703125, 9.1298828125, -0.1748046875, 11.25, -0.25]),
        ("Liberation Sans", 10.0, "bottom", "j", [2.2216796875, 0.244140625, 1.5478515625, 9.49609375, -0.1748046875, 11.25, -0.25]),
        ("Liberation Sans", 10.5, "alphabetic", "", [0.0, 0.0, 0.0, -0.0, 0.0, 10.0, 2.0]),
        ("Liberation Sans", 10.5, "alphabetic", "Mg", [14.586181640625, -0.861328125, 13.909423828125, 7.223876953125, 2.178955078125, 10.0, 2.0]),
        ("Liberation Sans", 10.5, "alphabetic", "j", [2.332763671875, 0.25634765625, 1.625244140625, 7.6083984375, 2.178955078125, 10.0, 2.0]),
        ("Liberation Sans", 10.5, "top", "", [0.0, 0.0, 0.0, -8.140625, 8.140625, 1.859375, 10.140625]),
        ("Liberation Sans", 10.5, "top", "Mg", [14.586181640625, -0.861328125, 13.909423828125, -0.916748046875, 10.319580078125, 1.859375, 10.140625]),
        ("Liberation Sans", 10.5, "top", "j", [2.332763671875, 0.25634765625, 1.625244140625, -0.5322265625, 10.319580078125, 1.859375, 10.140625]),
        ("Liberation Sans", 10.5, "middle", "", [0.0, 0.0, 0.0, -2.890625, 2.890625, 7.109375, 4.890625]),
        ("Liberation Sans", 10.5, "middle", "Mg", [14.586181640625, -0.861328125, 13.909423828125, 4.333251953125, 5.069580078125, 7.109375, 4.890625]),
        ("Liberation Sans", 10.5, "middle", "j", [2.332763671875, 0.25634765625, 1.625244140625, 4.7177734375, 5.069580078125, 7.109375, 4.890625]),
        ("Liberation Sans", 10.5, "bottom", "", [0.0, 0.0, 0.0, 2.359375, -2.359375, 12.359375, -0.359375]),
        ("Liberation Sans", 10.5, "bottom", "Mg", [14.586181640625, -0.861328125, 13.909423828125, 9.583251953125, -0.180419921875, 12.359375, -0.359375]),
        ("Liberation Sans", 10.5, "bottom", "j", [2.332763671875, 0.25634765625, 1.625244140625, 9.9677734375, -0.180419921875, 12.359375, -0.359375]),
        ("Liberation Sans", 16.0, "alphabetic", "", [0.0, 0.0, 0.0, -0.0, 0.0, 14.0, 3.0]),
        ("Liberation Sans", 16.0, "alphabetic", "Mg", [22.2265625, -1.3125, 21.1953125, 11.0078125, 3.3203125, 14.0, 3.0]),
        ("Liberation Sans", 16.0, "alphabetic", "j", [3.5546875, 0.390625, 2.4765625, 11.59375, 3.3203125, 14.0, 3.0]),
        ("Liberation Sans", 16.0, "top", "", [0.0, 0.0, 0.0, -12.40625, 12.40625, 1.59375, 15.40625]),
        ("Liberation Sans", 16.0, "top", "Mg", [22.2265625, -1.3125, 21.1953125, -1.3984375, 15.7265625, 1.59375, 15.40625]),
        ("Liberation Sans", 16.0, "top", "j", [3.5546875, 0.390625, 2.4765625, -0.8125, 15.7265625, 1.59375, 15.40625]),
        ("Liberation Sans", 16.0, "middle", "", [0.0, 0.0, 0.0, -4.40625, 4.40625, 9.59375, 7.40625]),
        ("Liberation Sans", 16.0, "middle", "Mg", [22.2265625, -1.3125, 21.1953125, 6.6015625, 7.7265625, 9.59375, 7.40625]),
        ("Liberation Sans", 16.0, "middle", "j", [3.5546875, 0.390625, 2.4765625, 7.1875, 7.7265625, 9.59375, 7.40625]),
        ("Liberation Sans", 16.0, "bottom", "", [0.0, 0.0, 0.0, 3.59375, -3.59375, 17.59375, -0.59375]),
        ("Liberation Sans", 16.0, "bottom", "Mg", [22.2265625, -1.3125, 21.1953125, 14.6015625, -0.2734375, 17.59375, -0.59375]),
        ("Liberation Sans", 16.0, "bottom", "j", [3.5546875, 0.390625, 2.4765625, 15.1875, -0.2734375, 17.59375, -0.59375]),
        ("Liberation Sans", 32.0, "alphabetic", "", [0.0, 0.0, 0.0, -0.0, 0.0, 29.0, 7.0]),
        ("Liberation Sans", 32.0, "alphabetic", "Mg", [44.453125, -2.625, 42.390625, 22.015625, 6.640625, 29.0, 7.0]),
        ("Liberation Sans", 32.0, "alphabetic", "j", [7.109375, 0.78125, 4.953125, 23.1875, 6.640625, 29.0, 7.0]),
        ("Liberation Sans", 32.0, "top", "", [0.0, 0.0, 0.0, -24.828125, 24.828125, 4.171875, 31.828125]),
        ("Liberation Sans", 32.0, "top", "Mg", [44.453125, -2.625, 42.390625, -2.8125, 31.46875, 4.171875, 31.828125]),
        ("Liberation Sans", 32.0, "top", "j", [7.109375, 0.78125, 4.953125, -1.640625, 31.46875, 4.171875, 31.828125]),
        ("Liberation Sans", 32.0, "middle", "", [0.0, 0.0, 0.0, -8.828125, 8.828125, 20.171875, 15.828125]),
        ("Liberation Sans", 32.0, "middle", "Mg", [44.453125, -2.625, 42.390625, 13.1875, 15.46875, 20.171875, 15.828125]),
        ("Liberation Sans", 32.0, "middle", "j", [7.109375, 0.78125, 4.953125, 14.359375, 15.46875, 20.171875, 15.828125]),
        ("Liberation Sans", 32.0, "bottom", "", [0.0, 0.0, 0.0, 7.171875, -7.171875, 36.171875, -0.171875]),
        ("Liberation Sans", 32.0, "bottom", "Mg", [44.453125, -2.625, 42.390625, 29.1875, -0.53125, 36.171875, -0.171875]),
        ("Liberation Sans", 32.0, "bottom", "j", [7.109375, 0.78125, 4.953125, 30.359375, -0.53125, 36.171875, -0.171875]),
        ("Liberation Serif", 10.0, "alphabetic", "", [0.0, 0.0, 0.0, -0.0, 0.0, 9.0, 2.0]),
        ("Liberation Serif", 10.0, "alphabetic", "Mg", [13.8916015625, -0.2880859375, 13.701171875, 6.5478515625, 2.158203125, 9.0, 2.0]),
        ("Liberation Serif", 10.0, "alphabetic", "j", [2.7783203125, 0.2880859375, 1.9189453125, 6.62109375, 2.12890625, 9.0, 2.0]),
        ("Liberation Serif", 10.0, "top", "", [0.0, 0.0, 0.0, -7.625, 7.625, 1.375, 9.625]),
        ("Liberation Serif", 10.0, "top", "Mg", [13.8916015625, -0.2880859375, 13.701171875, -1.0771484375, 9.783203125, 1.375, 9.625]),
        ("Liberation Serif", 10.0, "top", "j", [2.7783203125, 0.2880859375, 1.9189453125, -1.00390625, 9.75390625, 1.375, 9.625]),
        ("Liberation Serif", 10.0, "middle", "", [0.0, 0.0, 0.0, -2.625, 2.625, 6.375, 4.625]),
        ("Liberation Serif", 10.0, "middle", "Mg", [13.8916015625, -0.2880859375, 13.701171875, 3.9228515625, 4.783203125, 6.375, 4.625]),
        ("Liberation Serif", 10.0, "middle", "j", [2.7783203125, 0.2880859375, 1.9189453125, 3.99609375, 4.75390625, 6.375, 4.625]),
        ("Liberation Serif", 10.0, "bottom", "", [0.0, 0.0, 0.0, 2.375, -2.375, 11.375, -0.375]),
        ("Liberation Serif", 10.0, "bottom", "Mg", [13.8916015625, -0.2880859375, 13.701171875, 8.9228515625, -0.216796875, 11.375, -0.375]),
        ("Liberation Serif", 10.0, "bottom", "j", [2.7783203125, 0.2880859375, 1.9189453125, 8.99609375, -0.24609375, 11.375, -0.375]),
        ("Liberation Serif", 10.5, "alphabetic", "", [0.0, 0.0, 0.0, -0.0, 0.0, 9.0, 2.0]),
        ("Liberation Serif", 10.5, "alphabetic", "Mg", [14.586181640625, -0.302490234375, 14.38623046875, 6.875244140625, 2.26611328125, 9.0, 2.0]),
        ("Liberation Serif", 10.5, "alphabetic", "j", [2.917236328125, 0.302490234375, 2.014892578125, 6.9521484375, 2.2353515625, 9.0, 2.0]),
        ("Liberation Serif", 10.5, "top", "", [0.0, 0.0, 0.0, -8.0, 8.0, 1.0, 10.0]),
        ("Liberation Serif", 10.5, "top", "Mg", [14.586181640625, -0.302490234375, 14.38623046875, -1.124755859375, 10.26611328125, 1.0, 10.0]),
        ("Liberation Serif", 10.5, "top", "j", [2.917236328125, 0.302490234375, 2.014892578125, -1.0478515625, 10.2353515625, 1.0, 10.0]),
        ("Liberation Serif", 10.5, "middle", "", [0.0, 0.0, 0.0, -2.75, 2.75, 6.25, 4.75]),
        ("Liberation Serif", 10.5, "middle", "Mg", [14.586181640625, -0.302490234375, 14.38623046875, 4.125244140625, 5.01611328125, 6.25, 4.75]),
        ("Liberation Serif", 10.5, "middle", "j", [2.917236328125, 0.302490234375, 2.014892578125, 4.2021484375, 4.9853515625, 6.25, 4.75]),
        ("Liberation Serif", 10.5, "bottom", "", [0.0, 0.0, 0.0, 2.5, -2.5, 11.5, -0.5]),
        ("Liberation Serif", 10.5, "bottom", "Mg", [14.586181640625, -0.302490234375, 14.38623046875, 9.375244140625, -0.23388671875, 11.5, -0.5]),
        ("Liberation Serif", 10.5, "bottom", "j", [2.917236328125, 0.302490234375, 2.014892578125, 9.4521484375, -0.2646484375, 11.5, -0.5]),
        ("Liberation Serif", 16.0, "alphabetic", "", [0.0, 0.0, 0.0, -0.0, 0.0, 14.0, 3.0]),
        ("Liberation Serif", 16.0, "alphabetic", "Mg", [22.2265625, -0.4609375, 21.921875, 10.4765625, 3.453125, 14.0, 3.0]),
        ("Liberation Serif", 16.0, "alphabetic", "j", [4.4453125, 0.4609375, 3.0703125, 10.59375, 3.40625, 14.0, 3.0]),
        ("Liberation Serif", 16.0, "top", "", [0.0, 0.0, 0.0, -12.203125, 12.203125, 1.796875, 15.203125]),
        ("Liberation Serif", 16.0, "top", "Mg", [22.2265625, -0.4609375, 21.921875, -1.7265625, 15.65625, 1.796875, 15.203125]),
        ("Liberation Serif", 16.0, "top", "j", [4.4453125, 0.4609375, 3.0703125, -1.609375, 15.609375, 1.796875, 15.203125]),
        ("Liberation Serif", 16.0, "middle", "", [0.0, 0.0, 0.0, -4.203125, 4.203125, 9.796875, 7.203125]),
        ("Liberation Serif", 16.0, "middle", "Mg", [22.2265625, -0.4609375, 21.921875, 6.2734375, 7.65625, 9.796875, 7.203125]),
        ("Liberation Serif", 16.0, "middle", "j", [4.4453125, 0.4609375, 3.0703125, 6.390625, 7.609375, 9.796875, 7.203125]),
        ("Liberation Serif", 16.0, "bottom", "", [0.0, 0.0, 0.0, 3.796875, -3.796875, 17.796875, -0.796875]),
        ("Liberation Serif", 16.0, "bottom", "Mg", [22.2265625, -0.4609375, 21.921875, 14.2734375, -0.34375, 17.796875, -0.796875]),
        ("Liberation Serif", 16.0, "bottom", "j", [4.4453125, 0.4609375, 3.0703125, 14.390625, -0.390625, 17.796875, -0.796875]),
        ("Liberation Serif", 32.0, "alphabetic", "", [0.0, 0.0, 0.0, -0.0, 0.0, 29.0, 7.0]),
        ("Liberation Serif", 32.0, "alphabetic", "Mg", [44.453125, -0.921875, 43.84375, 20.953125, 6.90625, 29.0, 7.0]),
        ("Liberation Serif", 32.0, "alphabetic", "j", [8.890625, 0.921875, 6.140625, 21.1875, 6.8125, 29.0, 7.0]),
        ("Liberation Serif", 32.0, "top", "", [0.0, 0.0, 0.0, -24.40625, 24.40625, 4.59375, 31.40625]),
        ("Liberation Serif", 32.0, "top", "Mg", [44.453125, -0.921875, 43.84375, -3.453125, 31.3125, 4.59375, 31.40625]),
        ("Liberation Serif", 32.0, "top", "j", [8.890625, 0.921875, 6.140625, -3.21875, 31.21875, 4.59375, 31.40625]),
        ("Liberation Serif", 32.0, "middle", "", [0.0, 0.0, 0.0, -8.40625, 8.40625, 20.59375, 15.40625]),
        ("Liberation Serif", 32.0, "middle", "Mg", [44.453125, -0.921875, 43.84375, 12.546875, 15.3125, 20.59375, 15.40625]),
        ("Liberation Serif", 32.0, "middle", "j", [8.890625, 0.921875, 6.140625, 12.78125, 15.21875, 20.59375, 15.40625]),
        ("Liberation Serif", 32.0, "bottom", "", [0.0, 0.0, 0.0, 7.59375, -7.59375, 36.59375, -0.59375]),
        ("Liberation Serif", 32.0, "bottom", "Mg", [44.453125, -0.921875, 43.84375, 28.546875, -0.6875, 36.59375, -0.59375]),
        ("Liberation Serif", 32.0, "bottom", "j", [8.890625, 0.921875, 6.140625, 28.78125, -0.78125, 36.59375, -0.59375]),
        ("Liberation Mono", 10.0, "alphabetic", "", [0.0, 0.0, 0.0, -0.0, 0.0, 8.0, 3.0]),
        ("Liberation Mono", 10.0, "alphabetic", "Mg", [12.001953125, -0.6298828125, 11.1474609375, 6.5869140625, 2.0703125, 8.0, 3.0]),
        ("Liberation Mono", 10.0, "alphabetic", "j", [6.0009765625, -0.5712890625, 4.08203125, 7.24609375, 2.0751953125, 8.0, 3.0]),
        ("Liberation Mono", 10.0, "top", "", [0.0, 0.0, 0.0, -7.640625, 7.640625, 0.359375, 10.640625]),
        ("Liberation Mono", 10.0, "top", "Mg", [12.001953125, -0.6298828125, 11.1474609375, -1.0537109375, 9.7109375, 0.359375, 10.640625]),
        ("Liberation Mono", 10.0, "top", "j", [6.0009765625, -0.5712890625, 4.08203125, -0.39453125, 9.7158203125, 0.359375, 10.640625]),
        ("Liberation Mono", 10.0, "middle", "", [0.0, 0.0, 0.0, -2.640625, 2.640625, 5.359375, 5.640625]),
        ("Liberation Mono", 10.0, "middle", "Mg", [12.001953125, -0.6298828125, 11.1474609375, 3.9462890625, 4.7109375, 5.359375, 5.640625]),
        ("Liberation Mono", 10.0, "middle", "j", [6.0009765625, -0.5712890625, 4.08203125, 4.60546875, 4.7158203125, 5.359375, 5.640625]),
        ("Liberation Mono", 10.0, "bottom", "", [0.0, 0.0, 0.0, 2.359375, -2.359375, 10.359375, 0.640625]),
        ("Liberation Mono", 10.0, "bottom", "Mg", [12.001953125, -0.6298828125, 11.1474609375, 8.9462890625, -0.2890625, 10.359375, 0.640625]),
        ("Liberation Mono", 10.0, "bottom", "j", [6.0009765625, -0.5712890625, 4.08203125, 9.60546875, -0.2841796875, 10.359375, 0.640625]),
        ("Liberation Mono", 10.5, "alphabetic", "", [0.0, 0.0, 0.0, -0.0, 0.0, 9.0, 3.0]),
        ("Liberation Mono", 10.5, "alphabetic", "Mg", [12.60205078125, -0.661376953125, 11.704833984375, 6.916259765625, 2.173828125, 9.0, 3.0]),
        ("Liberation Mono", 10.5, "alphabetic", "j", [6.301025390625, -0.599853515625, 4.2861328125, 7.6083984375, 2.178955078125, 9.0, 3.0]),
        ("Liberation Mono", 10.5, "top", "", [0.0, 0.0, 0.0, -8.03125, 8.03125, 0.96875, 11.03125]),
        ("Liberation Mono", 10.5, "top", "Mg", [12.60205078125, -0.661376953125, 11.704833984375, -1.114990234375, 10.205078125, 0.96875, 11.03125]),
        ("Liberation Mono", 10.5, "top", "j", [6.301025390625, -0.599853515625, 4.2861328125, -0.4228515625, 10.210205078125, 0.96875, 11.03125]),
        ("Liberation Mono", 10.5, "middle", "", [0.0, 0.0, 0.0, -2.78125, 2.78125, 6.21875, 5.78125]),
        ("Liberation Mono", 10.5, "middle", "Mg", [12.60205078125, -0.661376953125, 11.704833984375, 4.135009765625, 4.955078125, 6.21875, 5.78125]),
        ("Liberation Mono", 10.5, "middle", "j", [6.301025390625, -0.599853515625, 4.2861328125, 4.8271484375, 4.960205078125, 6.21875, 5.78125]),
        ("Liberation Mono", 10.5, "bottom", "", [0.0, 0.0, 0.0, 2.46875, -2.46875, 11.46875, 0.53125]),
        ("Liberation Mono", 10.5, "bottom", "Mg", [12.60205078125, -0.661376953125, 11.704833984375, 9.385009765625, -0.294921875, 11.46875, 0.53125]),
        ("Liberation Mono", 10.5, "bottom", "j", [6.301025390625, -0.599853515625, 4.2861328125, 10.0771484375, -0.289794921875, 11.46875, 0.53125]),
        ("Liberation Mono", 16.0, "alphabetic", "", [0.0, 0.0, 0.0, -0.0, 0.0, 13.0, 5.0]),
        ("Liberation Mono", 16.0, "alphabetic", "Mg", [19.203125, -1.0078125, 17.8359375, 10.5390625, 3.3125, 13.0, 5.0]),
        ("Liberation Mono", 16.0, "alphabetic", "j", [9.6015625, -0.9140625, 6.53125, 11.59375, 3.3203125, 13.0, 5.0]),
        ("Liberation Mono", 16.0, "top", "", [0.0, 0.0, 0.0, -12.234375, 12.234375, 0.765625, 17.234375]),
        ("Liberation Mono", 16.0, "top", "Mg", [19.203125, -1.0078125, 17.8359375, -1.6953125, 15.546875, 0.765625, 17.234375]),
        ("Liberation Mono", 16.0, "top", "j", [9.6015625, -0.9140625, 6.53125, -0.640625, 15.5546875, 0.765625, 17.234375]),
        ("Liberation Mono", 16.0, "middle", "", [0.0, 0.0, 0.0, -4.234375, 4.234375, 8.765625, 9.234375]),
        ("Liberation Mono", 16.0, "middle", "Mg", [19.203125, -1.0078125, 17.8359375, 6.3046875, 7.546875, 8.765625, 9.234375]),
        ("Liberation Mono", 16.0, "middle", "j", [9.6015625, -0.9140625, 6.53125, 7.359375, 7.5546875, 8.765625, 9.234375]),
        ("Liberation Mono", 16.0, "bottom", "", [0.0, 0.0, 0.0, 3.765625, -3.765625, 16.765625, 1.234375]),
        ("Liberation Mono", 16.0, "bottom", "Mg", [19.203125, -1.0078125, 17.8359375, 14.3046875, -0.453125, 16.765625, 1.234375]),
        ("Liberation Mono", 16.0, "bottom", "j", [9.6015625, -0.9140625, 6.53125, 15.359375, -0.4453125, 16.765625, 1.234375]),
        ("Liberation Mono", 32.0, "alphabetic", "", [0.0, 0.0, 0.0, -0.0, 0.0, 27.0, 10.0]),
        ("Liberation Mono", 32.0, "alphabetic", "Mg", [38.40625, -2.015625, 35.671875, 21.078125, 6.625, 27.0, 10.0]),
        ("Liberation Mono", 32.0, "alphabetic", "j", [19.203125, -1.828125, 13.0625, 23.1875, 6.640625, 27.0, 10.0]),
        ("Liberation Mono", 32.0, "top", "", [0.0, 0.0, 0.0, -24.46875, 24.46875, 2.53125, 34.46875]),
        ("Liberation Mono", 32.0, "top", "Mg", [38.40625, -2.015625, 35.671875, -3.390625, 31.09375, 2.53125, 34.46875]),
        ("Liberation Mono", 32.0, "top", "j", [19.203125, -1.828125, 13.0625, -1.28125, 31.109375, 2.53125, 34.46875]),
        ("Liberation Mono", 32.0, "middle", "", [0.0, 0.0, 0.0, -8.46875, 8.46875, 18.53125, 18.46875]),
        ("Liberation Mono", 32.0, "middle", "Mg", [38.40625, -2.015625, 35.671875, 12.609375, 15.09375, 18.53125, 18.46875]),
        ("Liberation Mono", 32.0, "middle", "j", [19.203125, -1.828125, 13.0625, 14.71875, 15.109375, 18.53125, 18.46875]),
        ("Liberation Mono", 32.0, "bottom", "", [0.0, 0.0, 0.0, 7.53125, -7.53125, 34.53125, 2.46875]),
        ("Liberation Mono", 32.0, "bottom", "Mg", [38.40625, -2.015625, 35.671875, 28.609375, -0.90625, 34.53125, 2.46875]),
        ("Liberation Mono", 32.0, "bottom", "j", [19.203125, -1.828125, 13.0625, 30.71875, -0.890625, 34.53125, 2.46875]),
    ];
    assert_eq!(cases.len(),144);
    for &(family,size,baseline,text,expected) in cases {
        let font = CanvasFont { family: format!("\"{family}\""), size, weight:400, italic:false };
        let run = engine.shape(&font,text).unwrap();
        assert_eq!(run.geometry.source,crate::canvas_font_geometry::MetricSource::CoreText);
        assert!(!run.geometry.has_unresolved_baselines);
        if text=="Mg" {
            let expected_bytes=match family { "Liberation Sans"=>crate::font::SANS_R,
                "Liberation Serif"=>crate::font::SERIF_R, "Liberation Mono"=>crate::font::MONO_R, _=>unreachable!() };
            for glyph in &run.glyphs {
                assert!(engine.font_system.db().with_face_data(glyph.layout.font_id,|data,index|
                    index==0 && data==expected_bytes).unwrap());
            }
        }
        let m=run.metrics(TextReference { align:"left",baseline,rtl:false }).unwrap();
        assert_eq!([m.width,m.left,m.right,m.ascent,m.descent,m.font_ascent,m.font_descent],expected,"{family} {size} {baseline} {text:?}");
    }
}

#[test]
fn canvas_baseline_equivalent_fill_stroke_and_maxwidth() {
    let mut engine=CanvasTextEngine::new();
    let font=CanvasFont::parse("10.5px 'Liberation Sans'").unwrap();
    // Saved independent 10.5px OS/2/Chrome row, not a value read from Run.
    for (baseline,offset) in [("top",8.140625),("middle",2.890625),("bottom",-2.359375)] {
        for stroke in [None,Some(1.25)] {
            for max_width in [None,Some(10.0)] {
                let mut a=vec![0;128*80*4];let mut b=a.clone();
                for (pixels,baseline,y) in [(&mut a,baseline,30.0),(&mut b,"alphabetic",30.0+offset)] {
                    engine.draw(&font,"Mg j",TextPaint { reference:TextReference {align:"center",baseline,rtl:false},
                        x:60.25,y,color:[30,80,180,255],alpha:0.75,stroke_width:stroke,max_width },pixels,128,80).unwrap();
                }
                assert!(a.chunks_exact(4).any(|p|p[3]!=0));assert_eq!(a,b,"{baseline} {stroke:?} {max_width:?}");
            }
        }
    }
}
#[test]
fn canvas_empty_and_advancing_space_translate_the_zero_ink_rectangle() {
    let mut engine=CanvasTextEngine::new();let font=CanvasFont::parse("10.5px 'Liberation Sans'").unwrap();
    for text in [""," ","   "] {
        let run=engine.shape(&font,text).unwrap();assert!(run.ink.is_none());
        for align in ["left","center","right"] {
            for (baseline,y) in [("alphabetic",0.0),("top",8.140625),("middle",2.890625),("bottom",-2.359375)] {
                let m=run.metrics(TextReference {align,baseline,rtl:false}).unwrap();
                let expected_x=match align {"right"=>-run.width,"center"=>-run.width/2.0,_=>0.0};
                assert_eq!([m.left,m.right,m.ascent,m.descent],[-expected_x,expected_x,-y,y]);
            }
        }
    }
}
#[test]
fn canvas_numeric_geometry_is_warm_bounded_and_purged_with_generation() {
    let mut engine=CanvasTextEngine::new();let mut font=CanvasFont::parse("16px serif").unwrap();
    engine.shape(&font,"").unwrap();let queries=engine.geometry.queries;
    for text in ["Mg","j"," ","WWW"] {engine.shape(&font,text).unwrap();}
    assert_eq!(engine.geometry.queries,queries);
    for size in 1..=140 {font.size=size as f32+0.25;engine.shape(&font,"j").unwrap();}
    assert!(engine.geometry.len_for_test()<=128);
    let prior=engine.geometry.generation;
    engine.geometry.clear();engine.last=None;
    assert_eq!(engine.geometry.generation,prior+1);assert_eq!(engine.geometry.len_for_test(),0);
    engine.shape(&font,"").unwrap();assert_eq!(engine.geometry.len_for_test(),1);
}
#[test]
fn canvas_real_fallback_ink_does_not_replace_primary_geometry() {
    use crate::{font::with_native_provider_for_test,native_font::fixture_cascade_provider};
    const ARABIC:&[u8]=include_bytes!("../../../vendor/cosmic-text/fonts/NotoSansArabic.ttf");
    let (provider,_)=fixture_cascade_provider(vec![ARABIC.to_vec()],vec![0],32);
    with_native_provider_for_test(Some(provider),|| {
        let mut engine=CanvasTextEngine::new();let font=CanvasFont::parse("32px monospace").unwrap();
        let empty=engine.shape(&font,"").unwrap();
        let face=cosmic_text::ttf_parser::Face::parse(ARABIC,0).unwrap();
        let witness=(0x600..=0x8ff).filter_map(char::from_u32).find(|ch| {
            ch.is_alphabetic() && face.glyph_index(*ch).and_then(|id|face.glyph_bounding_box(id))
                .is_some_and(|b| f32::from(b.y_max)*32.0/f32::from(face.units_per_em())>empty.geometry.font_ascent)
                && engine.font_system.db().faces().all(|candidate|engine.font_system.db().with_face_data(candidate.id,|bytes,index|
                    cosmic_text::ttf_parser::Face::parse(bytes,index).unwrap().glyph_index(*ch).is_none()).unwrap())
        }).expect("real tall fallback witness absent from every base face");
        let run=engine.shape(&font,&witness.to_string()).unwrap();
        assert!(run.glyphs.iter().any(|g|engine.native.is_native_face(g.layout.font_id)));
        assert!(run.glyphs.iter().all(|g|g.layout.glyph_id!=0));
        assert_eq!([run.geometry.font_ascent,run.geometry.font_descent,run.geometry.em_ascent,run.geometry.em_descent],
            [empty.geometry.font_ascent,empty.geometry.font_descent,empty.geometry.em_ascent,empty.geometry.em_descent]);
        assert!(-run.ink.unwrap()[1]>run.geometry.font_ascent);
    });
}

#[cfg(target_os="macos")]
#[test]
fn canvas_coretext_exact_ttc_member_and_ambiguous_member_rejection() {
    use crate::canvas_font_geometry::FontBytes;
    let bytes:FontBytes=Arc::new(crate::native_font::fixture_collection(&[crate::font::SANS_R,crate::font::MONO_R]));
    let sans:FontBytes=Arc::new(crate::font::SANS_R);let mono:FontBytes=Arc::new(crate::font::MONO_R);
    for (index,reference) in [(0,&sans),(1,&mono)] {
        let collection=crate::native_font::canvas_instance_metrics(&ParsedCanvasFont::parse(&bytes,index).unwrap(),10.5,None).unwrap();
        let standalone=crate::native_font::canvas_instance_metrics(&ParsedCanvasFont::parse(reference,0).unwrap(),10.5,None).unwrap();
        assert_eq!([collection.ascent,collection.descent],[standalone.ascent,standalone.descent]);
        assert_eq!(collection.typo,standalone.typo);
    }
    let duplicate:FontBytes=Arc::new(crate::native_font::fixture_collection(&[crate::font::SANS_R,crate::font::SANS_R]));
    assert!(crate::native_font::canvas_instance_metrics(&ParsedCanvasFont::parse(&duplicate,1).unwrap(),10.5,None).is_err());
    assert!(ParsedCanvasFont::parse(&bytes,2).is_err());
}

#[cfg(all(target_os="macos",feature="paint"))]
#[test]
fn canvas_primary_variable_axes_exist_for_empty_and_first_fallback() {
    use base64::Engine as _;
    let encoded:String=include_str!("../tests/fonts/obscura-vf-test.woff2.b64").chars().filter(|c|!c.is_whitespace()).collect();
    let compressed=base64::engine::general_purpose::STANDARD.decode(encoded).unwrap();
    let bytes=wuff::decompress_woff2(&compressed).unwrap();
    let (provider,_)=crate::native_font::fixture_provider(bytes.clone());
    let face=cosmic_text::ttf_parser::Face::parse(&bytes,0).unwrap();
    let family=face.names().into_iter().filter(|n|n.name_id==1).find_map(|n|n.to_string()).unwrap();
    crate::font::with_native_provider_for_test(Some(provider),|| {
        let mut engine=CanvasTextEngine::new();
        for weight in [100,900] {
            let font=CanvasFont { family:format!("'{family}'"),size:16.0,weight,italic:false };
            let empty=engine.shape(&font,"").unwrap();
            assert_eq!(empty.geometry.source,crate::canvas_font_geometry::MetricSource::CoreText);
            let normal=engine.shape(&font,"M").unwrap();
            let own=normal.glyphs.first().expect("fixture M is a real primary glyph");
            let variations=own.variations.as_ref().expect("wght/opsz instance coordinates");
            assert_eq!(variations.iter().find(|v|v.tag.as_bytes()==b"wght").unwrap().value.0,weight as f32);
            let own_source=engine.font_system.db().face(own.layout.font_id).unwrap();
            let cosmic_text::fontdb::Source::Binary(owner)=&own_source.source else {panic!("retained source");};
            let instance=crate::native_font::canvas_instance_metrics(&ParsedCanvasFont::parse(owner,own_source.index).unwrap(),font.size,Some(variations)).unwrap();
            assert!(instance.ascent>0.0 && instance.descent>=0.0);
            let fallback=engine.shape(&font,"🚀M").unwrap();
            assert_ne!(fallback.glyphs[0].layout.font_id,own.layout.font_id);
            for run in [normal,fallback] {
                assert_eq!([run.geometry.font_ascent,run.geometry.font_descent,run.geometry.em_ascent,run.geometry.em_descent],
                    [empty.geometry.font_ascent,empty.geometry.font_descent,empty.geometry.em_ascent,empty.geometry.em_descent]);
            }
        }
        // At least two different primary coordinate tuples were actually queried.
        assert!(engine.geometry.queries>=2);
    });
}

#[cfg(all(target_os="macos",feature="paint"))]
#[test]
fn canvas_metric_failure_applies_real_axes_retries_and_recovers() {
    use base64::Engine as _;
    use crate::canvas_font_geometry::MetricSource;
    let encoded:String=include_str!("../tests/fonts/obscura-vf-test.woff2.b64").chars().filter(|c|!c.is_whitespace()).collect();
    let bytes=wuff::decompress_woff2(&base64::engine::general_purpose::STANDARD.decode(encoded).unwrap()).unwrap();
    let face=cosmic_text::ttf_parser::Face::parse(&bytes,0).unwrap();
    let family=face.names().into_iter().filter(|n|n.name_id==1).find_map(|n|n.to_string()).unwrap();
    let axes:Vec<_>=face.variation_axes().into_iter().collect();
    let weight=axes.iter().find(|a|a.tag.to_bytes()==*b"wght").unwrap();
    let optical=axes.iter().find(|a|a.tag.to_bytes()==*b"opsz").unwrap();
    assert!(weight.min_value<weight.def_value && weight.def_value<weight.max_value);
    assert!(optical.min_value<optical.max_value);
    let coordinates=[(weight.min_value,optical.min_value,-16384),(weight.max_value,optical.max_value,16384)];
    let (provider,_)=crate::native_font::fixture_provider(bytes.clone());
    crate::font::with_native_provider_for_test(Some(provider),|| {
        for (weight,size,endpoint) in coordinates {
            assert!(weight>=1.0 && weight<=1000.0 && weight.fract()==0.0);
            assert!(size>0.0 && size<=4096.0);
            let font=CanvasFont {family:format!("'{family}'"),size,weight:weight as u16,italic:false};
            let mut expected=face.clone();
            expected.set_variation(cosmic_text::ttf_parser::Tag::from_bytes(b"wght"),weight);
            expected.set_variation(cosmic_text::ttf_parser::Tag::from_bytes(b"opsz"),size);
            let scale=size/f32::from(expected.units_per_em());
            let vertical=[f32::from(expected.ascender())*scale,-f32::from(expected.descender())*scale];
            let mut engine=CanvasTextEngine::new();
            crate::native_font::with_canvas_metric_failure_for_test(|| {
                let mut primary_geometry=None;
                for text in [""," ","M",""] {
                    let prior=engine.geometry.queries;
                    let run=engine.shape(&font,text).unwrap();
                    assert_eq!(run.geometry.source,MetricSource::Software);
                    assert_eq!([run.geometry.font_ascent,run.geometry.font_descent],vertical);
                    let full=[run.geometry.font_ascent,run.geometry.font_descent,run.geometry.em_ascent,run.geometry.em_descent];
                    if let Some(prior)=primary_geometry {assert_eq!(full,prior);} else {primary_geometry=Some(full);}
                    assert_eq!(engine.geometry.queries,prior+1,"unavailable metrics must be retried");
                    assert_eq!(engine.geometry.len_for_test(),0,"native failure is not cached on macOS");
                    assert!(engine.last.is_none(),"software fallback must not enter identical-run reuse");
                    let observed=engine.geometry.software_coordinates.as_ref().expect("actual software Face coordinates");
                    assert_eq!(observed.len(),axes.len());
                    for (axis,coordinate) in axes.iter().zip(observed) {
                        let requested=if axis.tag.to_bytes()==*b"wght" {Some(weight)}
                            else if axis.tag.to_bytes()==*b"opsz" {Some(size)} else {None};
                        if let Some(requested)=requested {
                            // An endpoint equal to the fvar default stays at zero.
                            // avar preserves the normalized -1, 0 and +1 anchors.
                            let expected=if requested==axis.def_value {0} else {endpoint};
                            assert_eq!(*coordinate,expected,"real fvar endpoint must be applied before software metrics");
                        }
                    }
                }
            });
            let recovered=engine.shape(&font,"").unwrap();
            assert_eq!(recovered.geometry.source,MetricSource::CoreText);
            assert!(engine.geometry.len_for_test()>0);
            assert!(engine.geometry.software_coordinates.is_none());
            let prior=engine.geometry.queries;
            engine.shape(&font,"MA").unwrap();
            assert_eq!(engine.geometry.queries,prior,"different text reuses successful numeric metrics");
        }
    });
    // The adapter override must also restore on unwinding, not leak to later queries.
    let failure=std::panic::catch_unwind(|| crate::native_font::with_canvas_metric_failure_for_test(|| panic!("metric seam unwind")));
    assert!(failure.is_err());
    let owner:crate::canvas_font_geometry::FontBytes=Arc::new(bytes);
    // Use the same explicit instance coordinates as the successful native test.
    // The raw variable-font query with axes=None is not a qualified native path.
    let mut restored_axes=cosmic_text::FontVariations::new();
    restored_axes.set(cosmic_text::VariationTag::new(b"wght"),100.0);
    restored_axes.set(cosmic_text::VariationTag::new(b"opsz"),16.0);
    let restored=crate::native_font::canvas_instance_metrics(&ParsedCanvasFont::parse(&owner,0).unwrap(),16.0,Some(&restored_axes));
    assert!(restored.is_ok(),"post-unwind variable metrics at size=16 explicit axes failed: {:?}",restored.as_ref().err());
}

#[cfg(not(target_os="macos"))]
#[test]
fn canvas_unsupported_platform_keeps_software_geometry_cache() {
    let mut engine=CanvasTextEngine::new();
    let font=CanvasFont {family:"'Liberation Sans'".into(),size:16.0,weight:400,italic:false};
    let first=engine.shape(&font,"").unwrap();
    assert_eq!(first.geometry.source,crate::canvas_font_geometry::MetricSource::Software);
    assert_eq!(engine.geometry.len_for_test(),1);
    let prior=engine.geometry.queries;
    let second=engine.shape(&font,"M").unwrap();
    assert_eq!([first.geometry.font_ascent,first.geometry.font_descent],
        [second.geometry.font_ascent,second.geometry.font_descent]);
    assert_eq!(engine.geometry.queries,prior,"unsupported platforms cache software metrics");
}

#[test]
fn canvas_actual_native_eviction_purges_geometry_before_new_face_ids() {
    const A:&[u8]=include_bytes!("../../../vendor/cosmic-text/fonts/NotoSans-Regular.ttf");
    const B:&[u8]=include_bytes!("../../../vendor/cosmic-text/fonts/NotoSansHebrew.ttf");
    let (provider,_)=crate::native_font::fixture_cascade_provider(vec![A.to_vec(),B.to_vec()],vec![1,0],32);
    crate::font::with_native_provider_for_test(Some(provider),|| {
        let mut engine=CanvasTextEngine::new();engine.reduce_native_file_limit_for_test(1);
        let a=CanvasFont::parse("16px 'Noto Sans'").unwrap();
        let b=CanvasFont::parse("16px 'Noto Sans Hebrew'").unwrap();
        let first=engine.shape(&a,"A").unwrap();let id=first.glyphs[0].layout.font_id;
        let generation=engine.geometry.generation;
        let expected=[first.geometry.font_ascent,first.geometry.font_descent,first.geometry.em_ascent,first.geometry.em_descent];
        engine.shape(&b,"שלום").unwrap();
        assert!(engine.font_system.db().face(id).is_none());assert!(engine.geometry.generation>generation);
        let returned=engine.shape(&a,"A").unwrap();assert_ne!(returned.glyphs[0].layout.font_id,id);
        assert_eq!([returned.geometry.font_ascent,returned.geometry.font_descent,returned.geometry.em_ascent,returned.geometry.em_descent],expected);
        let (files,bytes,faces)=engine.native.resident_counts();assert_eq!(files,1);assert!(bytes<=32<<20 && faces<=256);
    });
}

#[cfg(target_os="macos")]
#[test]
fn canvas_tiny_size_policy_uses_real_coretext_instances() {
    let bytes:crate::canvas_font_geometry::FontBytes=Arc::new(crate::font::SANS_R);
    let mut engine=CanvasTextEngine::new();
    for size in [1.0,2.0,2.5,3.25,4.0] {
        let font=CanvasFont {family:"'Liberation Sans'".into(),size,weight:400,italic:false};
        let instance=crate::native_font::canvas_instance_metrics(&ParsedCanvasFont::parse(&bytes,0).unwrap(),size,None).unwrap();
        let run=engine.shape(&font,"").unwrap();
        assert_eq!(run.geometry.source,crate::canvas_font_geometry::MetricSource::CoreText);
        if size<=3.25 {
            assert!(run.geometry.uses_subpixel_tiny_metrics);
            assert_eq!([run.geometry.font_ascent,run.geometry.font_descent],[instance.ascent,instance.descent]);
        } else {
            assert!(!run.geometry.uses_subpixel_tiny_metrics);
            assert_eq!([run.geometry.font_ascent,run.geometry.font_descent],[4.0,1.0]);
        }
    }
}

fn unavailable_font_metrics(engine:&mut CanvasTextEngine,font:&CanvasFont,text:&str)->Vec<[f32;7]> {
    ["alphabetic","top","middle","bottom"].into_iter().map(|baseline| {
        let m=engine.measure(font,text,TextReference {align:"center",baseline,rtl:false}).unwrap();
        [m.width,m.left,m.right,m.ascent,m.descent,m.font_ascent,m.font_descent]
    }).collect()
}
fn unavailable_font_paint(engine:&mut CanvasTextEngine,font:&CanvasFont,stroke:Option<f32>)->Vec<u8> {
    let mut pixels=vec![0;160*80*4];
    engine.draw(font,"Mg j",TextPaint {reference:TextReference {align:"left",baseline:"top",rtl:false},
        x:12.25,y:18.0,color:[30,90,170,255],alpha:0.8,stroke_width:stroke,max_width:Some(32.0)},&mut pixels,160,80).unwrap();
    assert!(pixels.chunks_exact(4).any(|p|p[3]!=0));pixels
}
#[test]
fn canvas_sole_and_all_missing_named_fonts_use_real_loaded_fallback_style() {
    crate::font::with_native_provider_for_test(None,|| {
        for prefix in ["","bold ","italic ","italic bold ","550 "] {
            let expected=CanvasFont::parse(&format!("{prefix}16px sans-serif")).unwrap();
            let mut control=CanvasTextEngine::new();
            let expected_bytes=match (expected.italic,expected.weight>=550) {
                (false,false)=>crate::font::SANS_R,(false,true)=>crate::font::SANS_B,
                (true,false)=>crate::font::SANS_O,(true,true)=>crate::font::SANS_BO,
            };
            for stack in ["'Geometry Sole Missing Face'","'Geometry Missing One', 'Geometry Missing Two'"] {
                let font=CanvasFont::parse(&format!("{prefix}16px {stack}")).unwrap();
                let mut engine=CanvasTextEngine::new();
                for text in ["","Mg"," "] {
                    assert_eq!(unavailable_font_metrics(&mut engine,&font,text),unavailable_font_metrics(&mut control,&expected,text));
                    let run=engine.shape(&font,text).unwrap();
                    for glyph in &run.glyphs {
                        assert!(engine.font_system.db().with_face_data(glyph.layout.font_id,|bytes,index|
                            index==0 && bytes==expected_bytes).unwrap(),"wrong physical fallback for {prefix} {stack}");
                    }
                }
                for stroke in [None,Some(1.25)] {
                    assert_eq!(unavailable_font_paint(&mut engine,&font,stroke),unavailable_font_paint(&mut control,&expected,stroke));
                }
                assert!(!engine.native.retry_needed());
                assert_eq!(engine.native.resident_counts(),(0,0,0));
            }
        }
    });
}
#[test]
fn canvas_native_lookup_failure_uses_fallback_without_poisoning_retry() {
    use std::sync::atomic::Ordering;
    const NOTO:&[u8]=include_bytes!("../../../vendor/cosmic-text/fonts/NotoSans-Regular.ttf");
    for text in ["","Mg"," "] {
        let (provider,signals)=crate::native_font::fixture_provider(NOTO.to_vec());
        crate::font::with_native_provider_for_test(Some(provider),|| {
            let mut engine=CanvasTextEngine::new();let mut control=CanvasTextEngine::new();
            let font=CanvasFont::parse("16px 'Noto Sans'").unwrap();let fallback=CanvasFont::parse("16px sans-serif").unwrap();
            signals.fail_next.store(true,Ordering::Relaxed);
            let failed=engine.shape(&font,text).unwrap();
            assert!(engine.native.retry_needed());assert!(engine.last.is_none());
            assert_eq!(engine.native.resident_counts(),(0,0,0));
            let expected=control.shape(&fallback,text).unwrap();
            for baseline in ["alphabetic","top","middle","bottom"] {
                let r=TextReference {align:"center",baseline,rtl:false};let a=failed.metrics(r).unwrap();let b=expected.metrics(r).unwrap();
                assert_eq!([a.width,a.left,a.right,a.ascent,a.descent,a.font_ascent,a.font_descent],
                    [b.width,b.left,b.right,b.ascent,b.descent,b.font_ascent,b.font_descent]);
            }
            // Fail each new paint request deliberately; it must still paint the
            // true fallback, then retry a later identical authored family.
            for stroke in [None,Some(1.25)] {
                signals.fail_next.store(true,Ordering::Relaxed);
                assert_eq!(unavailable_font_paint(&mut engine,&font,stroke),unavailable_font_paint(&mut control,&fallback,stroke));
                assert!(engine.native.retry_needed());assert!(engine.last.is_none());
            }
            let calls=signals.lookup_calls.load(Ordering::Relaxed);
            let recovered=engine.shape(&font,text).unwrap();
            assert!(signals.lookup_calls.load(Ordering::Relaxed)>calls);
            assert!(!engine.native.retry_needed());assert_eq!(engine.native.resident_counts().0,1);
            assert!(!recovered.native_owners.is_empty(),"even empty text owns the selected native primary");
            assert!(recovered.glyphs.iter().all(|g|engine.native.is_native_face(g.layout.font_id)));
        });
    }
}
#[test]
fn canvas_native_no_admission_capacity_keeps_real_fallback_and_later_recovers() {
    const NOTO:&[u8]=include_bytes!("../../../vendor/cosmic-text/fonts/NotoSans-Regular.ttf");
    const HEBREW:&[u8]=include_bytes!("../../../vendor/cosmic-text/fonts/NotoSansHebrew.ttf");
    let (provider,_)=crate::native_font::fixture_style_provider_with_file_limit(vec![NOTO.to_vec(),HEBREW.to_vec()],1);
    let crate::native_font::Lookup::Found(held)=provider.lookup("Noto Sans Hebrew") else {panic!("real occupied provider file");};
    crate::font::with_native_provider_for_test(Some(provider),move || {
        let mut engine=CanvasTextEngine::new();let mut control=CanvasTextEngine::new();
        let font=CanvasFont::parse("16px 'Noto Sans'").unwrap();let fallback=CanvasFont::parse("16px sans-serif").unwrap();
        for text in ["","Mg"," "] {
            assert_eq!(unavailable_font_metrics(&mut engine,&font,text),unavailable_font_metrics(&mut control,&fallback,text));
            assert!(engine.native.retry_needed());assert!(engine.last.is_none());
            assert_eq!(engine.native.resident_counts(),(0,0,0));
        }
        for stroke in [None,Some(1.25)] {
            assert_eq!(unavailable_font_paint(&mut engine,&font,stroke),unavailable_font_paint(&mut control,&fallback,stroke));
            assert!(engine.native.retry_needed());assert!(engine.last.is_none());
        }
        drop(held);
        let recovered=engine.shape(&font,"Mg").unwrap();
        assert!(!engine.native.retry_needed());assert_eq!(engine.native.resident_counts().0,1);
        assert!(recovered.glyphs.iter().all(|g|engine.native.is_native_face(g.layout.font_id)));
    });
}

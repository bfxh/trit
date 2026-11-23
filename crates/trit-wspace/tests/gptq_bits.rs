//! 均匀位宽 GPTQ 的性质测试：码值域、块 scale 布局，以及相对 RTN 的 H 误差下降。

use trit_wspace::gptq::quantize_grouped_bits;

struct Rng(u64);

impl Rng {
    fn new(seed: u64) -> Self {
        Self(seed | 1)
    }
    fn next(&mut self) -> u64 {
        self.0 ^= self.0 >> 12;
        self.0 ^= self.0 << 25;
        self.0 ^= self.0 >> 27;
        self.0.wrapping_mul(0x2545_F491_4F6C_DD1D)
    }
    fn gaussian(&mut self) -> f32 {
        let mut sum = 0.0;
        for _ in 0..12 {
            let value = (self.next() >> 40) as f32 / (1u64 << 24) as f32;
            sum += value * 2.0 - 1.0;
        }
        sum
    }
}

fn hessian(samples: usize, cols: usize) -> Vec<f32> {
    let mut rng = Rng::new(7);
    let scale: Vec<f32> = (0..cols)
        .map(|col| 10f32.powi(col as i32 % 5 - 2))
        .collect();
    let x: Vec<f32> = (0..samples * cols)
        .map(|index| rng.gaussian() * scale[index % cols])
        .collect();
    let mut h = vec![0.0f32; cols * cols];
    for row in 0..samples {
        for a in 0..cols {
            let left = x[row * cols + a];
            for b in 0..cols {
                h[a * cols + b] += left * x[row * cols + b];
            }
        }
    }
    h
}

fn h_error(
    weight: &[f32],
    codes: &[i8],
    scales: &[f32],
    h: &[f32],
    rows: usize,
    cols: usize,
    group: usize,
) -> f64 {
    let groups = cols.div_ceil(group);
    let mut total = 0.0f64;
    for row in 0..rows {
        let error: Vec<f64> = (0..cols)
            .map(|col| {
                let value = f64::from(weight[row * cols + col]);
                let scale = f64::from(scales[row * groups + col / group]);
                value - scale * f64::from(codes[row * cols + col])
            })
            .collect();
        for a in 0..cols {
            for b in 0..cols {
                total += error[a] * f64::from(h[a * cols + b]) * error[b];
            }
        }
    }
    total
}

fn rtn(weight: &[f32], rows: usize, cols: usize, group: usize, bits: u32) -> (Vec<i8>, Vec<f32>) {
    let groups = cols.div_ceil(group);
    let qmax = ((1i32 << (bits - 1)) - 1) as f32;
    let mut codes = vec![0i8; rows * cols];
    let mut scales = vec![0.0f32; rows * groups];
    for row in 0..rows {
        for chunk in 0..groups {
            let start = chunk * group;
            let end = (start + group).min(cols);
            let max = weight[row * cols + start..row * cols + end]
                .iter()
                .fold(0.0f32, |max, value| max.max(value.abs()));
            let scale = if max == 0.0 { 1.0 } else { max / qmax };
            scales[row * groups + chunk] = scale;
            for col in start..end {
                let value = weight[row * cols + col] / scale;
                codes[row * cols + col] = value.round().clamp(-qmax, qmax) as i8;
            }
        }
    }
    (codes, scales)
}

#[test]
fn four_bit_codes_fit_and_gptq_beats_rtn() {
    let (rows, cols, group, bits) = (8, 16, 8, 4);
    let mut rng = Rng::new(11);
    let weight: Vec<f32> = (0..rows * cols).map(|_| rng.gaussian()).collect();
    let h = hessian(32, cols);
    let (codes, scales) =
        quantize_grouped_bits(&weight, rows, cols, &h, group, bits).expect("GPTQ 应成功");
    assert_eq!(codes.len(), rows * cols);
    assert_eq!(scales.len(), rows * (cols / group));
    assert!(codes.iter().all(|&code| (-7..=7).contains(&code)));
    assert!(scales.iter().all(|scale| scale.is_finite() && *scale > 0.0));
    let (rtn_codes, rtn_scales) = rtn(&weight, rows, cols, group, bits);
    let gptq_error = h_error(&weight, &codes, &scales, &h, rows, cols, group);
    let rtn_error = h_error(&weight, &rtn_codes, &rtn_scales, &h, rows, cols, group);
    assert!(
        gptq_error < rtn_error * 0.99,
        "GPTQ 误差 {gptq_error} 应明显低于 RTN {rtn_error}"
    );
}

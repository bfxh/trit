use serde_json::json;
use trit_wspace::safetensors::{self, Weights};
mod basis;
mod report;
use basis::*;
use trit_wspace::agree;
use trit_wspace::{Layer, Net, Trit};
const QUERY: &str = "encoder.layer.0.attention.self.query.weight";
const EMBEDDINGS: &str = "embeddings.word_embeddings.weight";
const DIMS: [usize; 6] = [1, 8, 32, 64, 128, 384];
fn trit_row(values: &[f32]) -> Vec<Trit> {
    let weights = Weights {
        rows: 1,
        cols: values.len(),
        values: values.to_vec(),
    };
    let layer = weights.to_layer().expect("ternarize");
    (0..values.len())
        .map(|c| layer.trit(0, c).expect("trit"))
        .collect()
}

fn trit_row_i8(values: &[f32]) -> Vec<i8> {
    trit_row(values).iter().map(|t| t.to_i8()).collect()
}

fn main() {
    let path = std::env::args()
        .nth(1)
        .expect("用法: agree_real <model.safetensors>");
    let query = safetensors::read_tensor(&path, QUERY).expect("query");
    let embeddings = safetensors::read_tensor(&path, EMBEDDINGS).expect("embeddings");
    let dim = query.cols;
    let row: usize = std::env::args()
        .nth(2)
        .map(|s| s.parse().expect("readout_row"))
        .unwrap_or(0);

    let readout: Vec<f32> = query.values[row * dim..(row + 1) * dim].to_vec();
    let student = Net::new(vec![
        query.to_layer().expect("ternary query"),
        Layer::new(1, dim, &trit_row_i8(&readout), &[0]).expect("readout"),
    ])
    .expect("student");

    let mut w = vec![0.0f64; dim];
    for (row, r) in readout.iter().enumerate() {
        let line = &query.values[row * dim..(row + 1) * dim];
        for (slot, value) in w.iter_mut().zip(line) {
            *slot += f64::from(*r) * f64::from(*value);
        }
    }
    let mut basis: Vec<Vec<f64>> = Vec::new();
    orthonormalize(&mut w, &basis);
    basis.push(w);

    let moment = second_moment(&embeddings, dim, 2048);
    for comp in jacobi_eigen(&moment, dim, 10) {
        if basis.len() >= dim {
            break;
        }
        let mut candidate = comp;
        if orthonormalize(&mut candidate, &basis) > 1e-6 {
            basis.push(candidate);
        }
    }

    let samples = 1000usize.min(embeddings.rows);
    let inputs: Vec<Vec<Trit>> = (0..samples)
        .map(|row| trit_row(&embeddings.values[row * dim..(row + 1) * dim]))
        .collect();

    let raw: Vec<&[f32]> = (0..samples)
        .map(|row| &embeddings.values[row * dim..(row + 1) * dim])
        .collect();
    let f32_base: Vec<Trit> = raw
        .iter()
        .map(|x| agree::reference(&query, &readout, x))
        .collect();

    let mut curves = Vec::new();
    for &k in &DIMS {
        if k > dim {
            continue;
        }
        let sub = &basis[..k];
        let qlayer = query.to_layer().expect("q");
        let bias: Vec<i8> = qlayer.bias().iter().map(|t| t.to_i8()).collect();
        let layer = Layer::new(
            dim,
            k,
            &{
                let mut trits = Vec::with_capacity(dim * k);
                for row in 0..dim {
                    let source = &query.values[row * dim..(row + 1) * dim];
                    let projected: Vec<f32> = sub
                        .iter()
                        .map(|u| {
                            u.iter()
                                .zip(source)
                                .map(|(a, b)| a * f64::from(*b))
                                .sum::<f64>() as f32
                        })
                        .collect();
                    trits.extend(trit_row_i8(&projected));
                }
                trits
            },
            &bias,
        )
        .expect("reduced layer");
        let net = Net::new(vec![
            layer,
            Layer::new(1, dim, &trit_row_i8(&readout), &[0]).expect("readout"),
        ])
        .expect("projected");

        let diag = agree::Diagnosis::new(&query, &readout, sub);
        let (rate, tern_out) = diag.agree(&student, &net, &inputs);
        let q = diag.compare(&f32_base, &raw, &inputs, &tern_out);
        curves.push(report::curve(k, rate, &q, &student, &net));
    }

    println!(
        "{}",
        serde_json::to_string_pretty(&json!({
            "input": "word_embeddings rows (real); f32 base = raw f32 weights+inputs",
            "basis": "supervised (w_eff + orthogonal PCA)", "readout_row": row,
            "curves": curves,
        }))
        .expect("json")
    );
}

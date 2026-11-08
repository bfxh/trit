//! 8³ 立方体自编码读数：两种三值编码器的 loss 轨迹与重建 IoU。
//!
//! 运行：`cargo run -p trit-3d --example voxel_ae`

use trit_3d::{
    AttentionEncoder, DenseEncoder, VoxelGrid, VoxelHead, train_attention_ae, train_dense_ae,
};

fn iou(out: &[f32], g: &VoxelGrid) -> f32 {
    let mut inter = 0usize;
    let mut union = 0usize;
    for (i, &v) in out.iter().enumerate() {
        let p = v > 0.0;
        let t = g.cells[i] > 0;
        if p && t {
            inter += 1;
        }
        if p || t {
            union += 1;
        }
    }
    inter as f32 / union.max(1) as f32
}

fn main() {
    let g = VoxelGrid::cube(8, 2);

    let mut enc = AttentionEncoder::new(8, 4, 128, 7);
    let mut head = VoxelHead::new(enc.flat_dim(), g.cells.len(), 7);
    let a = train_attention_ae(&mut enc, &mut head, std::slice::from_ref(&g), 2000, 0.05);
    let out = head.forward(&enc.encode_flat(&g));

    let mut denc = DenseEncoder::new(8, 64, 32, 11);
    let mut dhead = VoxelHead::new(denc.latent_dim(), g.cells.len(), 11);
    let d = train_dense_ae(&mut denc, &mut dhead, std::slice::from_ref(&g), 2000, 0.05);
    let dout = dhead.forward(&denc.encode(&g));

    println!(
        "{{\"attention\":{{\"first\":{:.6},\"last\":{:.6},\"iou\":{:.4},\"tokens\":{},\"latent\":{}}},\"dense\":{{\"first\":{:.6},\"last\":{:.6},\"iou\":{:.4},\"latent\":{}}}}}",
        a.first,
        a.last,
        iou(&out, &g),
        enc.tokens(),
        enc.latent_dim(),
        d.first,
        d.last,
        iou(&dout, &g),
        denc.latent_dim()
    );
}

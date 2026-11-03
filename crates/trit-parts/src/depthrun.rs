//! 深度头的**持久化资源引擎**：每条管线/每层的 uniform、输出缓冲与 bind group 只建一次。
//!
//! 训练循环因此只剩 `write_buffer` + dispatch；形状在训练期内不变，权重缓冲由
//! `GConv::upload` 原地覆盖（bind group 引用的是同一批 Buffer，仍然有效）。

use wgpu::Buffer;

use crate::depthkern::{ConvPl, ConvU8, FlatAcc, Pipes, acc_add6};
use crate::gpu::{Gpu, ROW, div_up};

#[repr(C)]
#[derive(Clone, Copy, bytemuck::Pod, bytemuck::Zeroable)]
struct DwU10 {
    h: u32,
    w: u32,
    cin: u32,
    cout: u32,
    kk: u32,
    pad: u32,
    relu: u32,
    chunk: u32,
    chunks: u32,
    _pad: u32,
}

#[repr(C)]
#[derive(Clone, Copy, bytemuck::Pod, bytemuck::Zeroable)]
struct RedU4 {
    n: u32,
    chunks: u32,
    _a: u32,
    _b: u32,
}

/// 一层的形状与权重缓冲（由 `GConv` 提供）。
pub(crate) struct Spec<'a> {
    pub cin: u32,
    pub gcout: u32,
    pub kk: u32,
    pub ternary: bool,
    pub relu: bool,
    pub wf: &'a Buffer,
    pub wt: &'a Buffer,
    pub ws: &'a Buffer,
    pub bias: &'a Buffer,
    pub wf_t: &'a Buffer,
}

/// 一层的持久资源（uniform 值在训练期内不变）。
struct LayerBufs {
    conv_u: Buffer,
    dw_u: Buffer,
    db_u: Buffer,
    dx_u: Buffer,
    part_w: Buffer,
    part_b: Buffer,
    dw: Buffer,
    db: Buffer,
    nw: usize,
    cin: u32,
    gcout: u32,
    chunks: u32,
    red4_bg: wgpu::BindGroup,
    red_bg: wgpu::BindGroup,
}

/// 一个样本槽：前向/反向的激活与梯度缓冲，以及引用它们的 bind group。
/// 激活字段只被 bind group 引用（wgpu 内部持有），这里保留所有权便于排查。
#[allow(dead_code)]
struct Slot {
    x: Buffer,
    a1: Buffer,
    a2: Buffer,
    o3: Buffer,
    dy3: Buffer,
    dy2: Buffer,
    dy1: Buffer,
    fwd: [wgpu::BindGroup; 3],
    /// `[0]` = c3 → dy2，`[1]` = c2 → dy1。
    dx: [wgpu::BindGroup; 2],
    dwk: [wgpu::BindGroup; 3],
    dbk: [wgpu::BindGroup; 3],
}

fn bg(device: &wgpu::Device, layout: &wgpu::BindGroupLayout, bufs: &[&Buffer]) -> wgpu::BindGroup {
    let entries: Vec<wgpu::BindGroupEntry> = bufs
        .iter()
        .enumerate()
        .map(|(i, b)| wgpu::BindGroupEntry {
            binding: i as u32,
            resource: b.as_entire_binding(),
        })
        .collect();
    device.create_bind_group(&wgpu::BindGroupDescriptor {
        label: None,
        layout,
        entries: &entries,
    })
}

pub(crate) struct Engine {
    pl: ConvPl,
    depth: Pipes,
    layers: Vec<LayerBufs>,
    slots: Vec<Slot>,
    hw: u32,
}

impl Engine {
    /// `specs` 依次是 c1/c2/c3（cin、补齐后的 cout、核尺寸与权重缓冲）。
    pub(crate) fn new(gpu: &Gpu, size: u32, nslots: usize, specs: [Spec<'_>; 3]) -> Self {
        let device = &gpu.device;
        let pl = ConvPl::new(device);
        let depth = Pipes::new(device);
        let hw = size * size;
        let p = hw as usize;
        let chunk = (p / 48).max(256);
        let chunks = p.div_ceil(chunk) as u32;
        let mut layers = Vec::with_capacity(3);
        for s in &specs {
            let nw = (s.kk * s.kk * s.cin * s.gcout) as usize;
            let (cin, gcout, kk) = (s.cin, s.gcout, s.kk);
            let conv_u = gpu.ubuf(&ConvU8 {
                h: size,
                w: size,
                cin,
                cout: gcout,
                kk,
                pad: kk / 2,
                mode: u32::from(s.ternary),
                relu: u32::from(s.relu),
            });
            let dw_u = gpu.ubuf(&DwU10 {
                h: size,
                w: size,
                cin,
                cout: gcout,
                kk,
                pad: kk / 2,
                relu: 0,
                chunk: chunk as u32,
                chunks,
                _pad: 0,
            });
            let db_u = gpu.ubuf(&DwU10 {
                h: size,
                w: size,
                cin,
                cout: gcout,
                kk,
                pad: kk / 2,
                relu: 0,
                chunk: chunk as u32,
                chunks,
                _pad: 0,
            });
            let dx_u = gpu.ubuf(&ConvU8 {
                h: size,
                w: size,
                cin,
                cout: gcout,
                kk,
                pad: kk / 2,
                mode: 0,
                relu: 0,
            });
            let part_w = gpu.zeros(nw * chunks as usize, "dg-part-w");
            let part_b = gpu.zeros(gcout as usize * chunks as usize, "dg-part-b");
            let dw = gpu.zeros(nw, "dg-dw");
            let db = gpu.zeros(gcout as usize, "dg-db");
            let red4_u = gpu.ubuf(&RedU4 {
                n: (nw / 4) as u32,
                chunks,
                _a: 0,
                _b: 0,
            });
            let red_u = gpu.ubuf(&RedU4 {
                n: gcout,
                chunks,
                _a: 0,
                _b: 0,
            });
            let red4_bg = bg(device, &pl.red4_bgl, &[&red4_u, &part_w, &dw]);
            let red_bg = bg(device, &pl.red_bgl, &[&red_u, &part_b, &db]);
            layers.push(LayerBufs {
                conv_u,
                dw_u,
                db_u,
                dx_u,
                part_w,
                part_b,
                dw,
                db,
                nw,
                cin,
                gcout,
                chunks,
                red4_bg,
                red_bg,
            });
        }

        let n = |i: usize| p * specs[i].gcout as usize;
        let mut slots = Vec::with_capacity(nslots);
        for _ in 0..nslots {
            let x = gpu.zeros(p * specs[0].cin as usize, "dg-s-x");
            let a1 = gpu.zeros(n(0), "dg-s-a1");
            let a2 = gpu.zeros(n(1), "dg-s-a2");
            let o3 = gpu.zeros(n(2), "dg-s-o3");
            let dy3 = gpu.zeros(n(2), "dg-s-dy3");
            let dy2 = gpu.zeros(p * specs[2].cin as usize, "dg-s-dy2");
            let dy1 = gpu.zeros(p * specs[1].cin as usize, "dg-s-dy1");
            let fwd = [
                bg(
                    device,
                    &pl.conv_bgl,
                    &[
                        &layers[0].conv_u,
                        &x,
                        specs[0].wf,
                        specs[0].wt,
                        specs[0].ws,
                        specs[0].bias,
                        &a1,
                    ],
                ),
                bg(
                    device,
                    &pl.conv_bgl,
                    &[
                        &layers[1].conv_u,
                        &a1,
                        specs[1].wf,
                        specs[1].wt,
                        specs[1].ws,
                        specs[1].bias,
                        &a2,
                    ],
                ),
                bg(
                    device,
                    &pl.conv_bgl,
                    &[
                        &layers[2].conv_u,
                        &a2,
                        specs[2].wf,
                        specs[2].wt,
                        specs[2].ws,
                        specs[2].bias,
                        &o3,
                    ],
                ),
            ];
            let dx = [
                bg(
                    device,
                    &depth.dx4m_bgl,
                    &[&layers[2].dx_u, &dy3, &a2, specs[2].wf_t, &dy2],
                ),
                bg(
                    device,
                    &depth.dx4m_bgl,
                    &[&layers[1].dx_u, &dy2, &a1, specs[1].wf_t, &dy1],
                ),
            ];
            let dwk = [
                bg(
                    device,
                    &pl.dw_bgl,
                    &[&layers[0].dw_u, &x, &dy1, &layers[0].part_w],
                ),
                bg(
                    device,
                    &pl.dw_bgl,
                    &[&layers[1].dw_u, &a1, &dy2, &layers[1].part_w],
                ),
                bg(
                    device,
                    &pl.dw_bgl,
                    &[&layers[2].dw_u, &a2, &dy3, &layers[2].part_w],
                ),
            ];
            let dbk = [
                bg(
                    device,
                    &pl.db_bgl,
                    &[&layers[0].db_u, &dy1, &layers[0].part_b],
                ),
                bg(
                    device,
                    &pl.db_bgl,
                    &[&layers[1].db_u, &dy2, &layers[1].part_b],
                ),
                bg(
                    device,
                    &pl.db_bgl,
                    &[&layers[2].db_u, &dy3, &layers[2].part_b],
                ),
            ];
            slots.push(Slot {
                x,
                a1,
                a2,
                o3,
                dy3,
                dy2,
                dy1,
                fwd,
                dx,
                dwk,
                dbk,
            });
        }
        Self {
            pl,
            depth,
            layers,
            slots,
            hw,
        }
    }

    pub(crate) fn slot_x(&self, i: usize) -> &Buffer {
        &self.slots[i].x
    }
    pub(crate) fn slot_o3(&self, i: usize) -> &Buffer {
        &self.slots[i].o3
    }
    pub(crate) fn slot_dy3(&self, i: usize) -> &Buffer {
        &self.slots[i].dy3
    }
    pub(crate) fn layer_dw(&self, i: usize) -> &Buffer {
        &self.layers[i].dw
    }
    pub(crate) fn layer_db(&self, i: usize) -> &Buffer {
        &self.layers[i].db
    }
    pub(crate) fn layer_nw(&self, i: usize) -> usize {
        self.layers[i].nw
    }
    pub(crate) fn layer_cout(&self, i: usize) -> usize {
        self.layers[i].gcout as usize
    }

    fn dispatch(&self, gpu: &Gpu, pl: &wgpu::ComputePipeline, b: &wgpu::BindGroup, n: u32) {
        gpu.record(|pass| {
            pass.set_pipeline(pl);
            pass.set_bind_group(0, b, &[]);
            pass.dispatch_workgroups(256, div_up(n, ROW), 1);
        });
    }

    /// 3 层前向（同一槽）。
    pub(crate) fn forward(&self, gpu: &Gpu, slot: usize) {
        for l in 0..3 {
            let total = self.hw * self.layers[l].gcout;
            self.dispatch(gpu, &self.pl.conv_pl, &self.slots[slot].fwd[l], total);
        }
    }

    /// `dw_k` + `reduce4`（两层 dispatch）。
    pub(crate) fn dw(&self, gpu: &Gpu, slot: usize, l: usize) {
        let layer = &self.layers[l];
        let nw4 = (layer.nw / 4) as u32;
        let total = nw4 * layer.chunks;
        self.dispatch(gpu, &self.pl.dw_pl, &self.slots[slot].dwk[l], total);
        self.dispatch(gpu, &self.pl.red4_pl, &layer.red4_bg, nw4);
    }

    /// `db_k` + `reduce`（两层 dispatch）。
    pub(crate) fn db(&self, gpu: &Gpu, slot: usize, l: usize) {
        let layer = &self.layers[l];
        let total = layer.gcout * layer.chunks;
        self.dispatch(gpu, &self.pl.db_pl, &self.slots[slot].dbk[l], total);
        self.dispatch(gpu, &self.pl.red_pl, &layer.red_bg, layer.gcout);
    }

    /// `conv_dx4m`：`which=0` 写 c3 的输入梯度（→ dy2），`which=1` 写 c2 的（→ dy1）。
    pub(crate) fn dx(&self, gpu: &Gpu, slot: usize, which: usize) {
        let cin = self.layers[which + 1].cin;
        let n4 = self.hw * cin / 4;
        self.dispatch(gpu, &self.depth.dx4m_pl, &self.slots[slot].dx[which], n4);
    }

    /// 一个槽的完整反向（用槽里已经写好的 `dy3`）。
    pub(crate) fn backward(&self, gpu: &Gpu, slot: usize) {
        self.dw(gpu, slot, 2);
        self.db(gpu, slot, 2);
        self.dx(gpu, slot, 0);
        self.dw(gpu, slot, 1);
        self.db(gpu, slot, 1);
        self.dx(gpu, slot, 1);
        self.dw(gpu, slot, 0);
        self.db(gpu, slot, 0);
    }

    /// 6 段梯度并成一次 dispatch 加进扁平累加器。
    pub(crate) fn acc_add(&self, gpu: &Gpu, dst: &Buffer, acc: &FlatAcc, srcs: [&Buffer; 6]) {
        acc_add6(gpu, &self.depth, dst, acc, srcs);
    }
}

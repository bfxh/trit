//! 深度头的**批次训练引擎**：K 个样本拼进一次 dispatch。
//!
//! - 所有缓冲按 NHWC 批次分配（`b·h·w·c`），`b=1` 与单图逐点一致；
//! - 每条管线的 uniform、每层输出/分片缓冲、整批激活与 bind group **只建一次**；
//! - 每个训练块只需 `set_batch(n)`（重写 12 字节级的 uniform）+ 写输入/dy + dispatch。
//!
//! 权重缓冲由 `GConv::upload` 原地覆盖，bind group 引用的是同一批 Buffer，仍然有效。

use std::cell::Cell;

use wgpu::Buffer;

use crate::depthkern::{FlatAcc, Pipes};
use crate::gpu::{Gpu, ROW, div_up};

#[repr(C)]
#[derive(Clone, Copy, bytemuck::Pod, bytemuck::Zeroable)]
struct ConvB {
    b: u32,
    h: u32,
    w: u32,
    cin: u32,
    cout: u32,
    kk: u32,
    pad: u32,
    mode: u32,
    relu: u32,
    _a: u32,
    _b: u32,
    _c: u32,
}

#[repr(C)]
#[derive(Clone, Copy, bytemuck::Pod, bytemuck::Zeroable)]
struct DwB {
    b: u32,
    h: u32,
    w: u32,
    cin: u32,
    cout: u32,
    kk: u32,
    pad: u32,
    relu: u32,
    chunk: u32,
    chunks: u32,
    _a: u32,
    _b: u32,
}

#[repr(C)]
#[derive(Clone, Copy, bytemuck::Pod, bytemuck::Zeroable)]
struct RedU {
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

struct PipeSet {
    conv_pl: wgpu::ComputePipeline,
    conv_bgl: wgpu::BindGroupLayout,
    dw_pl: wgpu::ComputePipeline,
    dw_bgl: wgpu::BindGroupLayout,
    red4_pl: wgpu::ComputePipeline,
    red4_bgl: wgpu::BindGroupLayout,
    db_pl: wgpu::ComputePipeline,
    db_bgl: wgpu::BindGroupLayout,
    red_pl: wgpu::ComputePipeline,
    red_bgl: wgpu::BindGroupLayout,
    dx_pl: wgpu::ComputePipeline,
    dx_bgl: wgpu::BindGroupLayout,
}

fn layout_entry(binding: u32, read_only: bool) -> wgpu::BindGroupLayoutEntry {
    wgpu::BindGroupLayoutEntry {
        binding,
        visibility: wgpu::ShaderStages::COMPUTE,
        ty: wgpu::BindingType::Buffer {
            ty: wgpu::BufferBindingType::Storage { read_only },
            has_dynamic_offset: false,
            min_binding_size: None,
        },
        count: None,
    }
}

fn layout_unif(binding: u32) -> wgpu::BindGroupLayoutEntry {
    wgpu::BindGroupLayoutEntry {
        binding,
        visibility: wgpu::ShaderStages::COMPUTE,
        ty: wgpu::BindingType::Buffer {
            ty: wgpu::BufferBindingType::Uniform,
            has_dynamic_offset: false,
            min_binding_size: None,
        },
        count: None,
    }
}

impl PipeSet {
    fn new(device: &wgpu::Device) -> Self {
        let depth = device.create_shader_module(wgpu::ShaderModuleDescriptor {
            label: Some("depth-batch"),
            source: wgpu::ShaderSource::Wgsl(include_str!("shaders_depth.wgsl").into()),
        });
        // 两个 reduce 内核复用共享的反向 shader（与 batch 无关）。
        let bwd = device.create_shader_module(wgpu::ShaderModuleDescriptor {
            label: Some("depth-reduce"),
            source: wgpu::ShaderSource::Wgsl(include_str!("shaders_bwd.wgsl").into()),
        });
        let mk = |entries: &[wgpu::BindGroupLayoutEntry]| {
            device.create_bind_group_layout(&wgpu::BindGroupLayoutDescriptor {
                label: None,
                entries,
            })
        };
        let conv_bgl = mk(&[
            layout_unif(0),
            layout_entry(1, true),
            layout_entry(2, true),
            layout_entry(3, true),
            layout_entry(4, true),
            layout_entry(5, true),
            layout_entry(6, false),
        ]);
        let dw_bgl = mk(&[
            layout_unif(0),
            layout_entry(1, true),
            layout_entry(2, true),
            layout_entry(3, false),
        ]);
        let red4_bgl = mk(&[
            layout_unif(0),
            layout_entry(1, true),
            layout_entry(2, false),
        ]);
        let db_bgl = mk(&[
            layout_unif(0),
            layout_entry(1, true),
            layout_entry(2, false),
        ]);
        let red_bgl = mk(&[
            layout_unif(0),
            layout_entry(1, true),
            layout_entry(2, false),
        ]);
        let dx_bgl = mk(&[
            layout_unif(0),
            layout_entry(1, true),
            layout_entry(2, true),
            layout_entry(3, true),
            layout_entry(4, false),
        ]);
        let pipe = |module: &wgpu::ShaderModule, label: &str, bgl: &wgpu::BindGroupLayout| {
            let pl = device.create_pipeline_layout(&wgpu::PipelineLayoutDescriptor {
                label: Some(label),
                bind_group_layouts: &[Some(bgl)],
                immediate_size: 0,
            });
            device.create_compute_pipeline(&wgpu::ComputePipelineDescriptor {
                label: Some(label),
                layout: Some(&pl),
                module,
                entry_point: Some(label),
                compilation_options: Default::default(),
                cache: None,
            })
        };
        Self {
            conv_pl: pipe(&depth, "conv_b", &conv_bgl),
            conv_bgl,
            dw_pl: pipe(&depth, "conv_dw_k_b", &dw_bgl),
            dw_bgl,
            red4_pl: pipe(&bwd, "reduce_chunks4", &red4_bgl),
            red4_bgl,
            db_pl: pipe(&depth, "conv_db_k_b", &db_bgl),
            db_bgl,
            red_pl: pipe(&bwd, "reduce_chunks", &red_bgl),
            red_bgl,
            dx_pl: pipe(&depth, "conv_dx4m_b", &dx_bgl),
            dx_bgl,
        }
    }
}

struct LayerBufs {
    conv_u: Buffer,
    dw_u: Buffer,
    db_u: Buffer,
    dx_u: Buffer,
    red4_u: Buffer,
    red_u: Buffer,
    part_w: Buffer,
    part_b: Buffer,
    dw: Buffer,
    db: Buffer,
    nw: usize,
    cin: u32,
    gcout: u32,
    kk: u32,
    mode: u32,
    relu: u32,
    red4_bg: wgpu::BindGroup,
    red_bg: wgpu::BindGroup,
}

/// 整批激活与梯度（容量 `cap`；尾块用前 `n` 个）。
#[allow(dead_code)]
struct BatchBufs {
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

fn zero_uniform(gpu: &Gpu, words: usize) -> Buffer {
    gpu.device.create_buffer(&wgpu::BufferDescriptor {
        label: Some("dg-u"),
        size: (words * 4) as u64,
        usage: wgpu::BufferUsages::UNIFORM | wgpu::BufferUsages::COPY_DST,
        mapped_at_creation: false,
    })
}

fn write_words(gpu: &Gpu, buf: &Buffer, words: &[u32]) {
    gpu.queue.write_buffer(buf, 0, bytemuck::cast_slice(words));
}

/// split-K 分片：`chunk` 至少 256 像素，块内像素总数按 48 片切。
fn chunking(hw: u32, n: u32) -> (u32, u32) {
    let total = n.max(1) * hw;
    let chunk = (total / 48).max(256);
    (chunk, total.div_ceil(chunk))
}

pub(crate) struct Engine {
    ps: PipeSet,
    accp: Pipes,
    layers: Vec<LayerBufs>,
    bb: BatchBufs,
    hw: u32,
    size: u32,
    cap: usize,
    cur_chunks: Cell<u32>,
    cur_n: Cell<u32>,
}

impl Engine {
    /// `specs` 依次是 c1/c2/c3（cin、补齐后的 cout、核尺寸与权重缓冲）。
    pub(crate) fn new(gpu: &Gpu, size: u32, cap: usize, specs: [Spec<'_>; 3]) -> Self {
        let device = &gpu.device;
        let ps = PipeSet::new(device);
        let accp = Pipes::new(device);
        let hw = size * size;
        let cap = cap.max(1);
        let (_, chunks_max) = chunking(hw, cap as u32);
        let mut layers = Vec::with_capacity(3);
        for s in &specs {
            let nw = (s.kk * s.kk * s.cin * s.gcout) as usize;
            let (cin, gcout, kk) = (s.cin, s.gcout, s.kk);
            let conv_u = zero_uniform(gpu, 12);
            let dx_u = zero_uniform(gpu, 12);
            let dw_u = zero_uniform(gpu, 12);
            let db_u = zero_uniform(gpu, 12);
            let red4_u = zero_uniform(gpu, 4);
            let red_u = zero_uniform(gpu, 4);
            let part_w = gpu.zeros(nw * chunks_max as usize, "dg-part-w");
            let part_b = gpu.zeros(gcout as usize * chunks_max as usize, "dg-part-b");
            let dw = gpu.zeros(nw, "dg-dw");
            let db = gpu.zeros(gcout as usize, "dg-db");
            let red4_bg = bg(device, &ps.red4_bgl, &[&red4_u, &part_w, &dw]);
            let red_bg = bg(device, &ps.red_bgl, &[&red_u, &part_b, &db]);
            layers.push(LayerBufs {
                conv_u,
                dw_u,
                db_u,
                dx_u,
                red4_u,
                red_u,
                part_w,
                part_b,
                dw,
                db,
                nw,
                cin,
                gcout,
                kk,
                mode: u32::from(s.ternary),
                relu: u32::from(s.relu),
                red4_bg,
                red_bg,
            });
        }

        let p = hw as usize;
        let n = |i: usize| cap * p * specs[i].gcout as usize;
        let x = gpu.zeros(cap * p * specs[0].cin as usize, "dg-b-x");
        let a1 = gpu.zeros(n(0), "dg-b-a1");
        let a2 = gpu.zeros(n(1), "dg-b-a2");
        let o3 = gpu.zeros(n(2), "dg-b-o3");
        let dy3 = gpu.zeros(n(2), "dg-b-dy3");
        let dy2 = gpu.zeros(cap * p * specs[2].cin as usize, "dg-b-dy2");
        let dy1 = gpu.zeros(cap * p * specs[1].cin as usize, "dg-b-dy1");
        let fwd = [
            bg(
                device,
                &ps.conv_bgl,
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
                &ps.conv_bgl,
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
                &ps.conv_bgl,
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
                &ps.dx_bgl,
                &[&layers[2].dx_u, &dy3, &a2, specs[2].wf_t, &dy2],
            ),
            bg(
                device,
                &ps.dx_bgl,
                &[&layers[1].dx_u, &dy2, &a1, specs[1].wf_t, &dy1],
            ),
        ];
        let dwk = [
            bg(
                device,
                &ps.dw_bgl,
                &[&layers[0].dw_u, &x, &dy1, &layers[0].part_w],
            ),
            bg(
                device,
                &ps.dw_bgl,
                &[&layers[1].dw_u, &a1, &dy2, &layers[1].part_w],
            ),
            bg(
                device,
                &ps.dw_bgl,
                &[&layers[2].dw_u, &a2, &dy3, &layers[2].part_w],
            ),
        ];
        let dbk = [
            bg(
                device,
                &ps.db_bgl,
                &[&layers[0].db_u, &dy1, &layers[0].part_b],
            ),
            bg(
                device,
                &ps.db_bgl,
                &[&layers[1].db_u, &dy2, &layers[1].part_b],
            ),
            bg(
                device,
                &ps.db_bgl,
                &[&layers[2].db_u, &dy3, &layers[2].part_b],
            ),
        ];
        let eng = Self {
            ps,
            accp,
            layers,
            bb: BatchBufs {
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
            },
            hw,
            size,
            cap,
            cur_chunks: Cell::new(0),
            cur_n: Cell::new(0),
        };
        eng.set_batch(gpu, 1);
        eng
    }

    pub(crate) fn cap(&self) -> usize {
        self.cap
    }

    pub(crate) fn x(&self) -> &Buffer {
        &self.bb.x
    }
    pub(crate) fn o3(&self) -> &Buffer {
        &self.bb.o3
    }
    pub(crate) fn dy3(&self) -> &Buffer {
        &self.bb.dy3
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

    /// 切到批大小 `n`：重写每层 uniform（`b/chunk/chunks`）。
    pub(crate) fn set_batch(&self, gpu: &Gpu, n: usize) {
        let n = n.clamp(1, self.cap) as u32;
        if n == self.cur_n.get() {
            return;
        }
        let (chunk, chunks) = chunking(self.hw, n);
        self.cur_chunks.set(chunks);
        self.cur_n.set(n);
        for l in &self.layers {
            let conv = ConvB {
                b: n,
                h: self.size,
                w: self.size,
                cin: l.cin,
                cout: l.gcout,
                kk: l.kk,
                pad: l.kk / 2,
                mode: l.mode,
                relu: l.relu,
                _a: 0,
                _b: 0,
                _c: 0,
            };
            write_words(gpu, &l.conv_u, bytemuck::cast_slice(&[conv]));
            let dx = ConvB {
                b: n,
                h: self.size,
                w: self.size,
                cin: l.cin,
                cout: l.gcout,
                kk: l.kk,
                pad: l.kk / 2,
                mode: 0,
                relu: 0,
                _a: 0,
                _b: 0,
                _c: 0,
            };
            write_words(gpu, &l.dx_u, bytemuck::cast_slice(&[dx]));
            let dw = DwB {
                b: n,
                h: self.size,
                w: self.size,
                cin: l.cin,
                cout: l.gcout,
                kk: l.kk,
                pad: l.kk / 2,
                relu: 0,
                chunk,
                chunks,
                _a: 0,
                _b: 0,
            };
            write_words(gpu, &l.dw_u, bytemuck::cast_slice(&[dw]));
            write_words(gpu, &l.db_u, bytemuck::cast_slice(&[dw]));
            let r4 = RedU {
                n: (l.nw / 4) as u32,
                chunks,
                _a: 0,
                _b: 0,
            };
            write_words(gpu, &l.red4_u, bytemuck::cast_slice(&[r4]));
            let rb = RedU {
                n: l.gcout,
                chunks,
                _a: 0,
                _b: 0,
            };
            write_words(gpu, &l.red_u, bytemuck::cast_slice(&[rb]));
        }
    }

    fn dispatch(&self, gpu: &Gpu, pl: &wgpu::ComputePipeline, b: &wgpu::BindGroup, n: u32) {
        gpu.record(|pass| {
            pass.set_pipeline(pl);
            pass.set_bind_group(0, b, &[]);
            pass.dispatch_workgroups(256, div_up(n, ROW), 1);
        });
    }

    /// 整批前向（3 次 dispatch）。
    pub(crate) fn forward(&self, gpu: &Gpu) {
        let n = self.cur_n.get();
        for l in 0..3 {
            let total = n * self.hw * self.layers[l].gcout;
            self.dispatch(gpu, &self.ps.conv_pl, &self.bb.fwd[l], total);
        }
    }

    /// `dw_k` + `reduce4`。
    pub(crate) fn dw(&self, gpu: &Gpu, l: usize) {
        let layer = &self.layers[l];
        let nw4 = (layer.nw / 4) as u32;
        let total = nw4 * self.cur_chunks.get();
        self.dispatch(gpu, &self.ps.dw_pl, &self.bb.dwk[l], total);
        self.dispatch(gpu, &self.ps.red4_pl, &layer.red4_bg, nw4);
    }

    /// `db_k` + `reduce`。
    pub(crate) fn db(&self, gpu: &Gpu, l: usize) {
        let layer = &self.layers[l];
        let total = layer.gcout * self.cur_chunks.get();
        self.dispatch(gpu, &self.ps.db_pl, &self.bb.dbk[l], total);
        self.dispatch(gpu, &self.ps.red_pl, &layer.red_bg, layer.gcout);
    }

    /// `conv_dx4m_b`：`which=0` 写 c3 的输入梯度（→ dy2），`which=1` 写 c2 的（→ dy1）。
    pub(crate) fn dx(&self, gpu: &Gpu, which: usize) {
        let cin = self.layers[which + 1].cin;
        let n4 = self.cur_n.get() * self.hw * cin / 4;
        self.dispatch(gpu, &self.ps.dx_pl, &self.bb.dx[which], n4);
    }

    /// 整批反向（8 次 dispatch）。
    pub(crate) fn backward(&self, gpu: &Gpu) {
        self.dw(gpu, 2);
        self.db(gpu, 2);
        self.dx(gpu, 0);
        self.dw(gpu, 1);
        self.db(gpu, 1);
        self.dx(gpu, 1);
        self.dw(gpu, 0);
        self.db(gpu, 0);
    }

    /// 6 段梯度并成一次 dispatch 加进扁平累加器。
    pub(crate) fn acc_add(&self, gpu: &Gpu, dst: &Buffer, acc: &FlatAcc, srcs: [&Buffer; 6]) {
        crate::depthkern::acc_add6(gpu, &self.accp, dst, acc, srcs);
    }
}

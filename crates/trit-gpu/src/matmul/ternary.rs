//! 三值内核族：一条 bind group、一个 shader，按 entry point + grid 跑各变体。
//!
//! 与 f32 基线共享的只是形状；数据布局（双导轨 + 参数都是 packed words）在这里。

use wgpu::util::DeviceExt;

use super::gpu::Gpu;
use super::inputs::Inputs;
use super::{storage, storage_with, uniform};
use crate::bench::{Check, bench_variant};

const WORKGROUP: u32 = 64;

pub(crate) struct Session {
    shader: wgpu::ShaderModule,
    pipeline_layout: wgpu::PipelineLayout,
    bind: wgpu::BindGroup,
    out: wgpu::Buffer,
    /// naive 内核：`ceil(m*n / WORKGROUP)`。
    grid1d: u32,
    /// tiled / 2×2 / 3-popc / 2-popc 共用（每组覆盖 16×16）。
    tiled: (u32, u32),
    /// 4×4 内核（每组覆盖 32×32）。
    reg4: (u32, u32),
    macs: usize,
    elems: usize,
}

impl Session {
    pub fn new(gpu: &Gpu, inp: &Inputs) -> Self {
        let usage = gpu.usage;
        let make = |data: &[u32]| {
            gpu.device
                .create_buffer_init(&wgpu::util::BufferInitDescriptor {
                    label: None,
                    contents: bytemuck::cast_slice(data),
                    usage: usage | wgpu::BufferUsages::COPY_SRC,
                })
        };
        let a_pos_buf = make(&inp.a_pos);
        let a_neg_buf = make(&inp.a_neg);
        let b_pos_buf = make(&inp.b_pos);
        let b_neg_buf = make(&inp.b_neg);
        let out = gpu.device.create_buffer(&wgpu::BufferDescriptor {
            label: None,
            size: (inp.m * inp.n * 4) as u64,
            usage: usage | wgpu::BufferUsages::COPY_SRC | wgpu::BufferUsages::COPY_DST,
            mapped_at_creation: false,
        });

        let shader = gpu
            .device
            .create_shader_module(wgpu::ShaderModuleDescriptor {
                label: Some("matmul"),
                source: wgpu::ShaderSource::Wgsl(include_str!("../matmul.wgsl").into()),
            });
        let layout = gpu
            .device
            .create_bind_group_layout(&wgpu::BindGroupLayoutDescriptor {
                label: None,
                entries: &[
                    storage(0),
                    storage(1),
                    storage(2),
                    storage(3),
                    storage_with(4, false),
                    uniform(5),
                    storage_with(6, false),
                ],
            });
        let pipeline_layout = gpu
            .device
            .create_pipeline_layout(&wgpu::PipelineLayoutDescriptor {
                label: None,
                bind_group_layouts: &[Some(&layout)],
                immediate_size: 0,
            });
        let params: [u32; 3] = [inp.m as u32, inp.n as u32, inp.words as u32];
        let params_buf = gpu
            .device
            .create_buffer_init(&wgpu::util::BufferInitDescriptor {
                label: None,
                contents: bytemuck::cast_slice(&params),
                usage: wgpu::BufferUsages::UNIFORM,
            });
        let bind = gpu.device.create_bind_group(&wgpu::BindGroupDescriptor {
            label: None,
            layout: &layout,
            entries: &[
                wgpu::BindGroupEntry {
                    binding: 0,
                    resource: a_pos_buf.as_entire_binding(),
                },
                wgpu::BindGroupEntry {
                    binding: 1,
                    resource: a_neg_buf.as_entire_binding(),
                },
                wgpu::BindGroupEntry {
                    binding: 2,
                    resource: b_pos_buf.as_entire_binding(),
                },
                wgpu::BindGroupEntry {
                    binding: 3,
                    resource: b_neg_buf.as_entire_binding(),
                },
                wgpu::BindGroupEntry {
                    binding: 4,
                    resource: out.as_entire_binding(),
                },
                wgpu::BindGroupEntry {
                    binding: 5,
                    resource: params_buf.as_entire_binding(),
                },
                wgpu::BindGroupEntry {
                    binding: 6,
                    resource: out.as_entire_binding(),
                },
            ],
        });

        let (m, n) = (inp.m as u32, inp.n as u32);
        Self {
            shader,
            pipeline_layout,
            bind,
            out,
            grid1d: (m * n).div_ceil(WORKGROUP),
            tiled: (n.div_ceil(16), m.div_ceil(16)),
            reg4: (n.div_ceil(32), m.div_ceil(32)),
            macs: inp.m * inp.n * inp.k,
            elems: inp.m * inp.n,
        }
    }

    /// 供 `popc::ceiling` 用同一个 shader 的 countOneBits 入口。
    pub fn shader(&self) -> &wgpu::ShaderModule {
        &self.shader
    }

    fn bench(
        &self,
        gpu: &Gpu,
        label: &str,
        entry: &str,
        grid: (u32, u32, u32),
        clear: bool,
        expected: &[i64],
    ) -> Option<(f64, bool)> {
        bench_variant(
            &gpu.device,
            &gpu.queue,
            &self.pipeline_layout,
            &self.shader,
            &self.bind,
            &self.out,
            label,
            entry,
            grid,
            self.macs,
            self.elems,
            Check::Exact(expected),
            clear,
        )
    }

    /// 一线程一输出（1D grid）。
    pub fn naive(&self, gpu: &Gpu, expected: &[i64]) -> Option<(f64, bool)> {
        self.bench(
            gpu,
            "matmul-ternary",
            "main",
            (self.grid1d, 1, 1),
            false,
            expected,
        )
    }

    /// 16×16 tile + 8-word 段。
    pub fn tiled(&self, gpu: &Gpu, expected: &[i64]) -> Option<(f64, bool)> {
        self.bench(
            gpu,
            "matmul-ternary-tiled",
            "main_tiled",
            (self.tiled.0, self.tiled.1, 1),
            false,
            expected,
        )
    }

    /// 2×2 微块。
    pub fn reg2(&self, gpu: &Gpu, expected: &[i64]) -> Option<(f64, bool)> {
        self.bench(
            gpu,
            "matmul-ternary-reg",
            "main_tiled2",
            (self.tiled.0, self.tiled.1, 1),
            false,
            expected,
        )
    }

    /// 4×4 微块（32×32 tile）。
    pub fn reg4(&self, gpu: &Gpu, expected: &[i64]) -> Option<(f64, bool)> {
        self.bench(
            gpu,
            "matmul-ternary-reg4",
            "main_tiled4",
            (self.reg4.0, self.reg4.1, 1),
            false,
            expected,
        )
    }

    /// 3-popc（2×2 微块，省 1/4 popcount）。
    pub fn o3(&self, gpu: &Gpu, expected: &[i64]) -> Option<(f64, bool)> {
        self.bench(
            gpu,
            "matmul-ternary-o3",
            "main_tiled2o3",
            (self.tiled.0, self.tiled.1, 1),
            false,
            expected,
        )
    }

    /// 2-popc（保留为 3-popc 的 A/B 基线）。
    pub fn o2(&self, gpu: &Gpu, expected: &[i64]) -> Option<(f64, bool)> {
        self.bench(
            gpu,
            "matmul-ternary-o2",
            "main_tiled2o2",
            (self.tiled.0, self.tiled.1, 1),
            false,
            expected,
        )
    }

    /// split-K：沿 K 切 4 片、atomicAdd 汇总；输出必须每轮先清零。
    pub fn splitk(&self, gpu: &Gpu, expected: &[i64]) -> Option<(f64, bool)> {
        self.bench(
            gpu,
            "matmul-ternary-splitk",
            "main_splitk",
            (self.tiled.0, self.tiled.1, 4),
            true,
            expected,
        )
    }
}

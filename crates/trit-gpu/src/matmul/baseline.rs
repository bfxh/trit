//! f32 基线族：与三值内核同形状、同 tiling 结构，唯一变量是 32 bit 表示。
//!
//! 绑定布局用独立的 `matmul_f32.wgsl`（binding 2 是输出、read_write）。

use wgpu::util::DeviceExt;

use super::gpu::Gpu;
use super::inputs::Inputs;
use super::{storage, storage_with, uniform};
use crate::bench::{Check, bench_variant, read_u32s};

pub(crate) struct Session {
    shader: wgpu::ShaderModule,
    pipeline_layout: wgpu::PipelineLayout,
    bind: wgpu::BindGroup,
    out: wgpu::Buffer,
    tiled: (u32, u32),
    reg4: (u32, u32),
    macs: usize,
    elems: usize,
}

impl Session {
    pub fn new(gpu: &Gpu, inp: &Inputs) -> Self {
        let usage = gpu.usage;
        let fa_buf = gpu
            .device
            .create_buffer_init(&wgpu::util::BufferInitDescriptor {
                label: None,
                contents: bytemuck::cast_slice(&inp.fa),
                usage,
            });
        let fb_buf = gpu
            .device
            .create_buffer_init(&wgpu::util::BufferInitDescriptor {
                label: None,
                contents: bytemuck::cast_slice(&inp.fb),
                usage,
            });
        let out = gpu.device.create_buffer(&wgpu::BufferDescriptor {
            label: None,
            size: (inp.m * inp.n * 4) as u64,
            usage: usage | wgpu::BufferUsages::COPY_SRC,
            mapped_at_creation: false,
        });

        let shader = gpu
            .device
            .create_shader_module(wgpu::ShaderModuleDescriptor {
                label: Some("matmul-f32"),
                source: wgpu::ShaderSource::Wgsl(include_str!("../matmul_f32.wgsl").into()),
            });
        let layout = gpu
            .device
            .create_bind_group_layout(&wgpu::BindGroupLayoutDescriptor {
                label: None,
                // f32 布局：binding 2 是输出，必须 read_write。
                entries: &[
                    storage(0),
                    storage(1),
                    storage_with(2, false),
                    uniform(3),
                    storage(4),
                    storage(5),
                ],
            });
        let pipeline_layout = gpu
            .device
            .create_pipeline_layout(&wgpu::PipelineLayoutDescriptor {
                label: None,
                bind_group_layouts: &[Some(&layout)],
                immediate_size: 0,
            });
        let params: [u32; 3] = [inp.m as u32, inp.n as u32, inp.k as u32];
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
                    resource: fa_buf.as_entire_binding(),
                },
                wgpu::BindGroupEntry {
                    binding: 1,
                    resource: fb_buf.as_entire_binding(),
                },
                wgpu::BindGroupEntry {
                    binding: 2,
                    resource: out.as_entire_binding(),
                },
                wgpu::BindGroupEntry {
                    binding: 3,
                    resource: params_buf.as_entire_binding(),
                },
                wgpu::BindGroupEntry {
                    binding: 4,
                    resource: fa_buf.as_entire_binding(),
                },
                wgpu::BindGroupEntry {
                    binding: 5,
                    resource: fb_buf.as_entire_binding(),
                },
            ],
        });

        let (m, n) = (inp.m as u32, inp.n as u32);
        Self {
            shader,
            pipeline_layout,
            bind,
            out,
            tiled: (n.div_ceil(16), m.div_ceil(16)),
            reg4: (n.div_ceil(32), m.div_ceil(32)),
            macs: inp.m * inp.n * inp.k,
            elems: inp.m * inp.n,
        }
    }

    fn bench(
        &self,
        gpu: &Gpu,
        label: &str,
        entry: &str,
        grid: (u32, u32, u32),
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
            Check::F32(expected),
            false,
        )
    }

    /// 经典共享内存 tiling（16×16）。
    pub fn tiled(&self, gpu: &Gpu, expected: &[i64]) -> Option<(f64, bool)> {
        self.bench(
            gpu,
            "matmul-f32-tiled",
            "main_tiled",
            (self.tiled.0, self.tiled.1, 1),
            expected,
        )
    }

    /// 4×4 + K 段 32（屏障再减半，共享 8 KB）。
    pub fn reg4k32(&self, gpu: &Gpu, expected: &[i64]) -> Option<(f64, bool)> {
        self.bench(
            gpu,
            "matmul-f32-reg4k32",
            "main_tiled4k32",
            (self.reg4.0, self.reg4.1, 1),
            expected,
        )
    }

    /// 4×4 + K 段 8。
    pub fn reg4(&self, gpu: &Gpu, expected: &[i64]) -> Option<(f64, bool)> {
        self.bench(
            gpu,
            "matmul-f32-reg4",
            "main_tiled4",
            (self.reg4.0, self.reg4.1, 1),
            expected,
        )
    }

    /// 2D 线程块 naive（对齐 cuda-oxide 的 16×16 block 映射）。
    pub fn two_d(&self, gpu: &Gpu, expected: &[i64]) -> Option<(f64, bool)> {
        self.bench(
            gpu,
            "matmul-f32-2d",
            "main2d",
            (self.tiled.0, self.tiled.1, 1),
            expected,
        )
    }

    /// 2×2 + K 段 8（对齐 cuda-oxide 的 reg2 结构）。
    pub fn reg2k8(&self, gpu: &Gpu, expected: &[i64]) -> Option<(f64, bool)> {
        self.bench(
            gpu,
            "matmul-f32-reg2k8",
            "main_tiled2k8",
            (self.tiled.0, self.tiled.1, 1),
            expected,
        )
    }

    /// 读回输出缓冲（最后跑的那个变体的结果），算相对误差的最大值。
    pub fn relative_error(&self, gpu: &Gpu, expected: &[i64]) -> Option<f64> {
        let bits = read_u32s(&gpu.device, &gpu.queue, &self.out, self.elems)?;
        Some(
            bits.iter()
                .map(|word| f32::from_bits(*word))
                .zip(expected.iter())
                .map(|(f, e)| {
                    let scale = e.abs().max(1) as f64;
                    f64::from((f - *e as f32).abs()) / scale
                })
                .fold(0.0f64, f64::max),
        )
    }
}

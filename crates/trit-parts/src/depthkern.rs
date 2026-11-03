//! 深度头专用的融合内核：管线、扁平梯度累加器与两条 dispatch。
//!
//! 与 `depthgpu`（模型/训练循环）分文件，避免单文件超模块预算；
//! shader 走自包含的 `shaders_depth.wgsl`，不改共享管线。

use wgpu::Buffer;

use crate::gpu::Gpu;

#[repr(C)]
#[derive(Clone, Copy, bytemuck::Pod, bytemuck::Zeroable)]
pub(crate) struct ConvU8 {
    pub(crate) h: u32,
    pub(crate) w: u32,
    pub(crate) cin: u32,
    pub(crate) cout: u32,
    pub(crate) kk: u32,
    pub(crate) pad: u32,
    pub(crate) mode: u32,
    pub(crate) relu: u32,
}

#[repr(C)]
#[derive(Clone, Copy, bytemuck::Pod, bytemuck::Zeroable)]
struct AccU {
    total: u32,
    nseg: u32,
    _a: u32,
    _b: u32,
    seg: [[u32; 4]; 8],
}

fn entry(binding: u32, read_only: bool) -> wgpu::BindGroupLayoutEntry {
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

fn unif(binding: u32) -> wgpu::BindGroupLayoutEntry {
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

/// 深度头自己的两条管线。
pub(crate) struct Pipes {
    pub(crate) dx4m_pl: wgpu::ComputePipeline,
    pub(crate) dx4m_bgl: wgpu::BindGroupLayout,
    pub(crate) acc_pl: wgpu::ComputePipeline,
    pub(crate) acc_bgl: wgpu::BindGroupLayout,
}

impl Pipes {
    pub(crate) fn new(device: &wgpu::Device) -> Self {
        let module = device.create_shader_module(wgpu::ShaderModuleDescriptor {
            label: Some("depth-fused"),
            source: wgpu::ShaderSource::Wgsl(include_str!("shaders_depth.wgsl").into()),
        });
        let mk = |entries: &[wgpu::BindGroupLayoutEntry]| {
            device.create_bind_group_layout(&wgpu::BindGroupLayoutDescriptor {
                label: None,
                entries,
            })
        };
        let dx4m_bgl = mk(&[
            unif(0),
            entry(1, true),
            entry(2, true),
            entry(3, true),
            entry(4, false),
        ]);
        let acc_bgl = mk(&[
            unif(0),
            entry(1, true),
            entry(2, true),
            entry(3, true),
            entry(4, true),
            entry(5, true),
            entry(6, true),
            entry(7, false),
        ]);
        let pipe = |label: &str, bgl: &wgpu::BindGroupLayout| {
            let pl = device.create_pipeline_layout(&wgpu::PipelineLayoutDescriptor {
                label: Some(label),
                bind_group_layouts: &[Some(bgl)],
                immediate_size: 0,
            });
            device.create_compute_pipeline(&wgpu::ComputePipelineDescriptor {
                label: Some(label),
                layout: Some(&pl),
                module: &module,
                entry_point: Some(label),
                compilation_options: Default::default(),
                cache: None,
            })
        };
        Self {
            dx4m_pl: pipe("conv_dx4m", &dx4m_bgl),
            dx4m_bgl,
            acc_pl: pipe("acc_add6", &acc_bgl),
            acc_bgl,
        }
    }
}

/// 6 段梯度共用的扁平累加器（`--accum K`）：加一次、读一次、清一次。
pub(crate) struct FlatAcc {
    buf: Option<Buffer>,
    total: usize,
    lens: [usize; 6],
    offs: [usize; 6],
}

impl FlatAcc {
    pub(crate) fn new() -> Self {
        Self {
            buf: None,
            total: 0,
            lens: [0; 6],
            offs: [0; 6],
        }
    }

    pub(crate) fn init(&mut self, gpu: &Gpu, lens: [usize; 6]) {
        self.lens = lens;
        let mut off = 0usize;
        for (o, len) in self.offs.iter_mut().zip(&lens) {
            *o = off;
            off += len;
        }
        self.total = off;
        self.buf = Some(gpu.zeros(off, "dg-acc"));
    }

    pub(crate) fn ready(&self) -> bool {
        self.buf.is_some()
    }

    pub(crate) fn buf(&self) -> &Buffer {
        self.buf.as_ref().expect("累加器未初始化")
    }

    pub(crate) fn total(&self) -> usize {
        self.total
    }

    pub(crate) fn offs(&self) -> [usize; 6] {
        self.offs
    }

    pub(crate) fn lens(&self) -> [usize; 6] {
        self.lens
    }
}

/// 6 段梯度并成 **1 次 dispatch** 加进扁平累加器。
pub(crate) fn acc_add6(gpu: &Gpu, pipes: &Pipes, dst: &Buffer, acc: &FlatAcc, srcs: [&Buffer; 6]) {
    let mut seg = [[0u32; 4]; 8];
    for (s, (o, l)) in seg.iter_mut().zip(acc.offs.iter().zip(&acc.lens)) {
        *s = [*o as u32, *l as u32, 0, 0];
    }
    let ub = gpu.ubuf(&AccU {
        total: acc.total as u32,
        nseg: 6,
        _a: 0,
        _b: 0,
        seg,
    });
    let bg = gpu.device.create_bind_group(&wgpu::BindGroupDescriptor {
        label: None,
        layout: &pipes.acc_bgl,
        entries: &[
            wgpu::BindGroupEntry {
                binding: 0,
                resource: ub.as_entire_binding(),
            },
            wgpu::BindGroupEntry {
                binding: 1,
                resource: srcs[0].as_entire_binding(),
            },
            wgpu::BindGroupEntry {
                binding: 2,
                resource: srcs[1].as_entire_binding(),
            },
            wgpu::BindGroupEntry {
                binding: 3,
                resource: srcs[2].as_entire_binding(),
            },
            wgpu::BindGroupEntry {
                binding: 4,
                resource: srcs[3].as_entire_binding(),
            },
            wgpu::BindGroupEntry {
                binding: 5,
                resource: srcs[4].as_entire_binding(),
            },
            wgpu::BindGroupEntry {
                binding: 6,
                resource: srcs[5].as_entire_binding(),
            },
            wgpu::BindGroupEntry {
                binding: 7,
                resource: dst.as_entire_binding(),
            },
        ],
    });
    gpu.record(|pass| {
        pass.set_pipeline(&pipes.acc_pl);
        pass.set_bind_group(0, &bg, &[]);
        pass.dispatch_workgroups(
            256,
            crate::gpu::div_up(acc.total as u32, crate::gpu::ROW),
            1,
        );
    });
}

// ---------------------------------------------------------------- 持久化资源引擎
//
// 每个 conv/bwd 调用的 uniform、输入/输出缓冲与 bind group **只建一次**：
// 形状在训练期内不变，权重缓冲由 `GConv::upload` 原地覆盖（bind group 仍有效）。

pub(crate) struct ConvPl {
    pub(crate) conv_pl: wgpu::ComputePipeline,
    pub(crate) conv_bgl: wgpu::BindGroupLayout,
    pub(crate) dw_pl: wgpu::ComputePipeline,
    pub(crate) dw_bgl: wgpu::BindGroupLayout,
    pub(crate) red4_pl: wgpu::ComputePipeline,
    pub(crate) red4_bgl: wgpu::BindGroupLayout,
    pub(crate) db_pl: wgpu::ComputePipeline,
    pub(crate) db_bgl: wgpu::BindGroupLayout,
    pub(crate) red_pl: wgpu::ComputePipeline,
    pub(crate) red_bgl: wgpu::BindGroupLayout,
}

impl ConvPl {
    pub(crate) fn new(device: &wgpu::Device) -> Self {
        let fwd = device.create_shader_module(wgpu::ShaderModuleDescriptor {
            label: Some("depth-fwd"),
            source: wgpu::ShaderSource::Wgsl(include_str!("shaders.wgsl").into()),
        });
        let bwd = device.create_shader_module(wgpu::ShaderModuleDescriptor {
            label: Some("depth-bwd"),
            source: wgpu::ShaderSource::Wgsl(include_str!("shaders_bwd.wgsl").into()),
        });
        let mk = |entries: &[wgpu::BindGroupLayoutEntry]| {
            device.create_bind_group_layout(&wgpu::BindGroupLayoutDescriptor {
                label: None,
                entries,
            })
        };
        let conv_bgl = mk(&[
            unif(0),
            entry(1, true),
            entry(2, true),
            entry(3, true),
            entry(4, true),
            entry(5, true),
            entry(6, false),
        ]);
        let dw_bgl = mk(&[unif(0), entry(1, true), entry(2, true), entry(3, false)]);
        let red4_bgl = mk(&[unif(0), entry(1, true), entry(2, false)]);
        let db_bgl = mk(&[unif(0), entry(1, true), entry(2, false)]);
        let red_bgl = mk(&[unif(0), entry(1, true), entry(2, false)]);
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
            conv_pl: pipe(&fwd, "conv", &conv_bgl),
            conv_bgl,
            dw_pl: pipe(&bwd, "conv_dw_k", &dw_bgl),
            dw_bgl,
            red4_pl: pipe(&bwd, "reduce_chunks4", &red4_bgl),
            red4_bgl,
            db_pl: pipe(&bwd, "conv_db_k", &db_bgl),
            db_bgl,
            red_pl: pipe(&bwd, "reduce_chunks", &red_bgl),
            red_bgl,
        }
    }
}

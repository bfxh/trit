//! 深度头专用的融合内核：管线、扁平梯度累加器与两条 dispatch。
//!
//! 与 `depthgpu`（模型/训练循环）分文件，避免单文件超模块预算；
//! shader 走自包含的 `shaders_depth.wgsl`，不改共享管线。

use wgpu::Buffer;

use crate::gpu::Gpu;

#[repr(C)]
#[derive(Clone, Copy, bytemuck::Pod, bytemuck::Zeroable)]
struct ConvU8 {
    h: u32,
    w: u32,
    cin: u32,
    cout: u32,
    kk: u32,
    pad: u32,
    mode: u32,
    relu: u32,
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
    dx4m_pl: wgpu::ComputePipeline,
    dx4m_bgl: wgpu::BindGroupLayout,
    acc_pl: wgpu::ComputePipeline,
    acc_bgl: wgpu::BindGroupLayout,
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

/// `conv_dx4m`：与 `conv_dx4` 同式，额外按 ReLU 输出 `y` 把输入梯度逐通道置零。
#[allow(clippy::too_many_arguments)]
pub(crate) fn dx_masked(
    gpu: &Gpu,
    pipes: &Pipes,
    cin: u32,
    gcout: u32,
    kk: u32,
    size: u32,
    dy: &Buffer,
    y: &Buffer,
    wf_t: &Buffer,
) -> Buffer {
    let ub = gpu.ubuf(&ConvU8 {
        h: size,
        w: size,
        cin,
        cout: gcout,
        kk,
        pad: kk / 2,
        mode: 0,
        relu: 0,
    });
    let out = gpu.empty_out((size * size * cin) as usize, "dg-dx");
    let bg = gpu.device.create_bind_group(&wgpu::BindGroupDescriptor {
        label: None,
        layout: &pipes.dx4m_bgl,
        entries: &[
            wgpu::BindGroupEntry {
                binding: 0,
                resource: ub.as_entire_binding(),
            },
            wgpu::BindGroupEntry {
                binding: 1,
                resource: dy.as_entire_binding(),
            },
            wgpu::BindGroupEntry {
                binding: 2,
                resource: y.as_entire_binding(),
            },
            wgpu::BindGroupEntry {
                binding: 3,
                resource: wf_t.as_entire_binding(),
            },
            wgpu::BindGroupEntry {
                binding: 4,
                resource: out.as_entire_binding(),
            },
        ],
    });
    let n4 = size * size * cin / 4;
    gpu.record(|pass| {
        pass.set_pipeline(&pipes.dx4m_pl);
        pass.set_bind_group(0, &bg, &[]);
        pass.dispatch_workgroups(256, crate::gpu::div_up(n4, crate::gpu::ROW), 1);
    });
    out
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

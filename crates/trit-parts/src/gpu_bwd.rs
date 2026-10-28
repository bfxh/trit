//! 反向内核的 wgpu 封装（与 [`crate::gpu`] 的前向管线并列）。
//!
//! 只产出「对有效权重 E」的梯度；`E → latent` 的 STE 映射在主机侧（见 `backward::ste`）。

use wgpu::util::DeviceExt;

use crate::gpu::{Gpu, ROW, div_up};
use crate::weights::ConvW;

struct Pipe {
    pl: wgpu::ComputePipeline,
    bgl: wgpu::BindGroupLayout,
}

#[derive(Clone, Copy)]
enum Bind {
    Uniform,
    Read,
    Write,
}

fn layout(device: &wgpu::Device, binds: &[Bind]) -> wgpu::BindGroupLayout {
    let entries: Vec<wgpu::BindGroupLayoutEntry> = binds
        .iter()
        .enumerate()
        .map(|(i, kind)| wgpu::BindGroupLayoutEntry {
            binding: i as u32,
            visibility: wgpu::ShaderStages::COMPUTE,
            ty: wgpu::BindingType::Buffer {
                ty: match kind {
                    Bind::Uniform => wgpu::BufferBindingType::Uniform,
                    Bind::Read => wgpu::BufferBindingType::Storage { read_only: true },
                    Bind::Write => wgpu::BufferBindingType::Storage { read_only: false },
                },
                has_dynamic_offset: false,
                min_binding_size: None,
            },
            count: None,
        })
        .collect();
    device.create_bind_group_layout(&wgpu::BindGroupLayoutDescriptor {
        label: None,
        entries: &entries,
    })
}

/// 八条反向管线。
pub struct Bwd {
    conv_dx: Pipe,
    conv_dw: Pipe,
    conv_db: Pipe,
    pool_dx: Pipe,
    cat_dx: Pipe,
    convt_dx: Pipe,
    convt_dw: Pipe,
    convt_db: Pipe,
}

impl Bwd {
    pub fn new(device: &wgpu::Device) -> Self {
        let module = device.create_shader_module(wgpu::ShaderModuleDescriptor {
            label: Some("parts-bwd"),
            source: wgpu::ShaderSource::Wgsl(include_str!("shaders_bwd.wgsl").into()),
        });
        let mk = |entry: &str, binds: &[Bind]| {
            let bgl = layout(device, binds);
            let pl = device.create_pipeline_layout(&wgpu::PipelineLayoutDescriptor {
                label: Some(entry),
                bind_group_layouts: &[Some(&bgl)],
                immediate_size: 0,
            });
            let pl = device.create_compute_pipeline(&wgpu::ComputePipelineDescriptor {
                label: Some(entry),
                layout: Some(&pl),
                module: &module,
                entry_point: Some(entry),
                compilation_options: Default::default(),
                cache: None,
            });
            Pipe { pl, bgl }
        };
        Self {
            conv_dx: mk(
                "conv_dx",
                &[
                    Bind::Uniform,
                    Bind::Read,
                    Bind::Read,
                    Bind::Read,
                    Bind::Read,
                    Bind::Read,
                    Bind::Write,
                ],
            ),
            conv_dw: mk(
                "conv_dw",
                &[
                    Bind::Uniform,
                    Bind::Read,
                    Bind::Read,
                    Bind::Read,
                    Bind::Write,
                ],
            ),
            conv_db: mk(
                "conv_db",
                &[Bind::Uniform, Bind::Read, Bind::Read, Bind::Write],
            ),
            pool_dx: mk(
                "pool_dx",
                &[Bind::Uniform, Bind::Read, Bind::Read, Bind::Write],
            ),
            cat_dx: mk(
                "cat_dx",
                &[Bind::Uniform, Bind::Read, Bind::Write, Bind::Write],
            ),
            convt_dx: mk(
                "convt_dx",
                &[
                    Bind::Uniform,
                    Bind::Read,
                    Bind::Read,
                    Bind::Read,
                    Bind::Read,
                    Bind::Write,
                ],
            ),
            convt_dw: mk(
                "convt_dw",
                &[Bind::Uniform, Bind::Read, Bind::Read, Bind::Write],
            ),
            convt_db: mk("convt_db", &[Bind::Uniform, Bind::Read, Bind::Write]),
        }
    }
}

#[repr(C)]
#[derive(Clone, Copy, bytemuck::Pod, bytemuck::Zeroable)]
struct ConvU {
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
struct PoolU {
    h: u32,
    w: u32,
    c: u32,
    _p: u32,
}

#[repr(C)]
#[derive(Clone, Copy, bytemuck::Pod, bytemuck::Zeroable)]
struct CatU {
    hw: u32,
    ca: u32,
    cb: u32,
    _p: u32,
}

#[repr(C)]
#[derive(Clone, Copy, bytemuck::Pod, bytemuck::Zeroable)]
struct UpU {
    h: u32,
    w: u32,
    cin: u32,
    cout: u32,
    mode: u32,
    _a: u32,
    _b: u32,
    _c: u32,
}

impl Gpu {
    fn dispatch(&self, pipe: &Pipe, bufs: &[&wgpu::Buffer], total: usize) {
        let entries: Vec<wgpu::BindGroupEntry> = bufs
            .iter()
            .enumerate()
            .map(|(i, b)| wgpu::BindGroupEntry {
                binding: i as u32,
                resource: b.as_entire_binding(),
            })
            .collect();
        let bg = self.device.create_bind_group(&wgpu::BindGroupDescriptor {
            label: None,
            layout: &pipe.bgl,
            entries: &entries,
        });
        let mut enc = self.device.create_command_encoder(&Default::default());
        {
            let mut pass = enc.begin_compute_pass(&Default::default());
            pass.set_pipeline(&pipe.pl);
            pass.set_bind_group(0, &bg, &[]);
            pass.dispatch_workgroups(256, div_up(total as u32, ROW), 1);
        }
        self.queue.submit([enc.finish()]);
    }

    fn zeros(&self, n: usize, label: &str) -> wgpu::Buffer {
        self.device
            .create_buffer_init(&wgpu::util::BufferInitDescriptor {
                label: Some(label),
                contents: &vec![0u8; n * 4],
                usage: wgpu::BufferUsages::STORAGE
                    | wgpu::BufferUsages::COPY_SRC
                    | wgpu::BufferUsages::COPY_DST,
            })
    }

    fn conv_u(&self, h: u32, w: u32, cw: &ConvW, mode: u32, relu: u32) -> wgpu::Buffer {
        self.ubuf(&ConvU {
            h,
            w,
            cin: cw.cin,
            cout: cw.cout,
            kk: cw.kk,
            pad: cw.kk / 2,
            mode,
            relu,
        })
    }

    /// 卷积输入梯度 `∂L/∂x`（`y` 是前向输出，用于 ReLU 掩码）。
    #[allow(clippy::too_many_arguments)]
    pub fn bwd_conv_dx(
        &self,
        h: u32,
        w: u32,
        cw: &ConvW,
        mode: u32,
        relu: u32,
        y: &wgpu::Buffer,
        dy: &wgpu::Buffer,
        wf: &wgpu::Buffer,
        wt: &wgpu::Buffer,
        ws: &wgpu::Buffer,
    ) -> wgpu::Buffer {
        let u = self.conv_u(h, w, cw, mode, relu);
        let dx = self.zeros((h * w * cw.cin) as usize, "conv-dx");
        let bufs = [&u, y, dy, wf, wt, ws, &dx];
        self.dispatch(&self.bwd.conv_dx, &bufs, dx.size() as usize / 4);
        dx
    }

    /// 卷积权重梯度 `∂L/∂E`（`x` 是前向输入）。
    #[allow(clippy::too_many_arguments)]
    pub fn bwd_conv_dw(
        &self,
        h: u32,
        w: u32,
        cw: &ConvW,
        relu: u32,
        x: &wgpu::Buffer,
        y: &wgpu::Buffer,
        dy: &wgpu::Buffer,
    ) -> wgpu::Buffer {
        let u = self.conv_u(h, w, cw, 0, relu);
        let n = (cw.kk * cw.kk * cw.cin * cw.cout) as usize;
        let dw = self.zeros(n, "conv-dw");
        let bufs = [&u, x, y, dy, &dw];
        self.dispatch(&self.bwd.conv_dw, &bufs, n);
        dw
    }

    /// 卷积偏置梯度。
    pub fn bwd_conv_db(
        &self,
        h: u32,
        w: u32,
        cw: &ConvW,
        relu: u32,
        y: &wgpu::Buffer,
        dy: &wgpu::Buffer,
    ) -> wgpu::Buffer {
        let u = self.conv_u(h, w, cw, 0, relu);
        let db = self.zeros(cw.cout as usize, "conv-db");
        let bufs = [&u, y, dy, &db];
        self.dispatch(&self.bwd.conv_db, &bufs, cw.cout as usize);
        db
    }

    /// 池化反向（输出已清零，只有 argmax 位置被写入）。
    pub fn bwd_pool_dx(
        &self,
        h: u32,
        w: u32,
        c: u32,
        x: &wgpu::Buffer,
        dy: &wgpu::Buffer,
    ) -> wgpu::Buffer {
        let u = self.ubuf(&PoolU { h, w, c, _p: 0 });
        let dx = self.zeros((h * w * c) as usize, "pool-dx");
        let total = ((h / 2) * (w / 2) * c) as usize;
        let bufs = [&u, x, dy, &dx];
        self.dispatch(&self.bwd.pool_dx, &bufs, total);
        dx
    }

    /// 拼接反向：返回 `(∂L/∂a, ∂L/∂b)`。
    pub fn bwd_cat_dx(
        &self,
        hw: u32,
        ca: u32,
        cb: u32,
        dy: &wgpu::Buffer,
    ) -> (wgpu::Buffer, wgpu::Buffer) {
        let u = self.ubuf(&CatU { hw, ca, cb, _p: 0 });
        let da = self.zeros((hw * ca) as usize, "cat-da");
        let db = self.zeros((hw * cb) as usize, "cat-db");
        let total = (hw * (ca + cb)) as usize;
        let bufs = [&u, dy, &da, &db];
        self.dispatch(&self.bwd.cat_dx, &bufs, total);
        (da, db)
    }

    /// 转置卷积输入梯度。
    #[allow(clippy::too_many_arguments)]
    pub fn bwd_convt_dx(
        &self,
        h: u32,
        w: u32,
        cw: &ConvW,
        mode: u32,
        dy: &wgpu::Buffer,
        wf: &wgpu::Buffer,
        wt: &wgpu::Buffer,
        ws: &wgpu::Buffer,
    ) -> wgpu::Buffer {
        let u = self.ubuf(&UpU {
            h,
            w,
            cin: cw.cin,
            cout: cw.cout,
            mode,
            _a: 0,
            _b: 0,
            _c: 0,
        });
        let dx = self.zeros((h * w * cw.cin) as usize, "convt-dx");
        let bufs = [&u, dy, wf, wt, ws, &dx];
        self.dispatch(&self.bwd.convt_dx, &bufs, dx.size() as usize / 4);
        dx
    }

    /// 转置卷积权重梯度。
    pub fn bwd_convt_dw(
        &self,
        h: u32,
        w: u32,
        cw: &ConvW,
        x: &wgpu::Buffer,
        dy: &wgpu::Buffer,
    ) -> wgpu::Buffer {
        let u = self.ubuf(&UpU {
            h,
            w,
            cin: cw.cin,
            cout: cw.cout,
            mode: 0,
            _a: 0,
            _b: 0,
            _c: 0,
        });
        let n = (4 * cw.cin * cw.cout) as usize;
        let dw = self.zeros(n, "convt-dw");
        let bufs = [&u, x, dy, &dw];
        self.dispatch(&self.bwd.convt_dw, &bufs, n);
        dw
    }

    /// 转置卷积偏置梯度。
    pub fn bwd_convt_db(&self, h: u32, w: u32, cw: &ConvW, dy: &wgpu::Buffer) -> wgpu::Buffer {
        let u = self.ubuf(&UpU {
            h,
            w,
            cin: cw.cin,
            cout: cw.cout,
            mode: 0,
            _a: 0,
            _b: 0,
            _c: 0,
        });
        let db = self.zeros(cw.cout as usize, "convt-db");
        let bufs = [&u, dy, &db];
        self.dispatch(&self.bwd.convt_db, &bufs, cw.cout as usize);
        db
    }
}

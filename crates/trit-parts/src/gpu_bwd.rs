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
    add: Pipe,
    conv_dw_k: Pipe,
    conv_db_k: Pipe,
    reduce: Pipe,
    add_into: Pipe,
    clear: Pipe,
    mask_dy: Pipe,
    reduce4: Pipe,
    conv_dx4: Pipe,
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
                &[Bind::Uniform, Bind::Read, Bind::Read, Bind::Write],
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
                &[Bind::Uniform, Bind::Read, Bind::Read, Bind::Write],
            ),
            convt_dw: mk(
                "convt_dw",
                &[Bind::Uniform, Bind::Read, Bind::Read, Bind::Write],
            ),
            convt_db: mk("convt_db", &[Bind::Uniform, Bind::Read, Bind::Write]),
            add: mk("add", &[Bind::Uniform, Bind::Read, Bind::Read, Bind::Write]),
            conv_dw_k: mk(
                "conv_dw_k",
                &[Bind::Uniform, Bind::Read, Bind::Read, Bind::Write],
            ),
            conv_db_k: mk("conv_db_k", &[Bind::Uniform, Bind::Read, Bind::Write]),
            reduce: mk("reduce_chunks", &[Bind::Uniform, Bind::Read, Bind::Write]),
            add_into: mk("add_into", &[Bind::Uniform, Bind::Read, Bind::Write]),
            clear: mk("clear_buf", &[Bind::Uniform, Bind::Write]),
            mask_dy: mk(
                "mask_dy",
                &[Bind::Uniform, Bind::Read, Bind::Read, Bind::Write],
            ),
            reduce4: mk("reduce_chunks4", &[Bind::Uniform, Bind::Read, Bind::Write]),
            conv_dx4: mk(
                "conv_dx4",
                &[Bind::Uniform, Bind::Read, Bind::Read, Bind::Write],
            ),
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

#[repr(C)]
#[derive(Clone, Copy, bytemuck::Pod, bytemuck::Zeroable)]
struct DwU {
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
struct MaskU {
    n: u32,
    relu: u32,
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

#[repr(C)]
#[derive(Clone, Copy, bytemuck::Pod, bytemuck::Zeroable)]
struct AddU {
    n: u32,
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

    /// 分配但不初始化（只用于「每个元素都会被写满」的输出，省掉一次主机→显存拷贝）。
    pub fn alloc(&self, n: usize, label: &str) -> wgpu::Buffer {
        self.device.create_buffer(&wgpu::BufferDescriptor {
            label: Some(label),
            size: (n * 4) as u64,
            usage: wgpu::BufferUsages::STORAGE
                | wgpu::BufferUsages::COPY_SRC
                | wgpu::BufferUsages::COPY_DST,
            mapped_at_creation: false,
        })
    }

    pub fn zeros(&self, n: usize, label: &str) -> wgpu::Buffer {
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

    /// 把 ReLU 掩码预先乘进 dy（dx/dw/db 三个内核就不用再读 y）。
    pub fn mask_dy(
        &self,
        y: &wgpu::Buffer,
        dy: &wgpu::Buffer,
        n: usize,
        relu: bool,
    ) -> wgpu::Buffer {
        let u = self.ubuf(&MaskU {
            n: n as u32,
            relu: relu as u32,
            _a: 0,
            _b: 0,
        });
        let out = self.alloc(n, "dy-masked");
        let bufs = [&u, y, dy, &out];
        self.dispatch(&self.bwd.mask_dy, &bufs, n);
        out
    }

    /// 卷积输入梯度 `∂L/∂x`（`dy` 已含 ReLU 掩码）。
    #[allow(clippy::too_many_arguments)]
    pub fn bwd_conv_dx(
        &self,
        h: u32,
        w: u32,
        cw: &ConvW,
        relu: u32,
        dy: &wgpu::Buffer,
        wf: &wgpu::Buffer,
        we_t: &wgpu::Buffer,
    ) -> wgpu::Buffer {
        assert!(cw.cout.is_multiple_of(4), "vec4 内核要求 cout 是 4 的倍数");
        let u = self.conv_u(h, w, cw, 0, relu);
        let dx = self.alloc((h * w * cw.cin) as usize, "conv-dx");
        if cw.cin.is_multiple_of(4) {
            // 转置权重 [k·cout+oc]·cin+ic，按 ic4 向量化
            let bufs = [&u, dy, we_t, &dx];
            self.dispatch(&self.bwd.conv_dx4, &bufs, (h * w * cw.cin / 4) as usize);
        } else {
            // 标量路径用原始布局 [k·cin+ic]·cout+oc，按 oc4 向量化
            let bufs = [&u, dy, wf, &dx];
            self.dispatch(&self.bwd.conv_dx, &bufs, dx.size() as usize / 4);
        }
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
    pub fn bwd_convt_dx(
        &self,
        h: u32,
        w: u32,
        cw: &ConvW,
        mode: u32,
        dy: &wgpu::Buffer,
        we_t: &wgpu::Buffer,
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
        let dx = self.alloc((h * w * cw.cin) as usize, "convt-dx");
        let bufs = [&u, dy, we_t, &dx];
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
        let dw = self.alloc(n, "convt-dw");
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
        let db = self.alloc(cw.cout as usize, "convt-db");
        let bufs = [&u, dy, &db];
        self.dispatch(&self.bwd.convt_db, &bufs, cw.cout as usize);
        db
    }

    /// split-K 版权重梯度：并行度 = 权重数 × 分片数，先写分片再归约。
    #[allow(clippy::too_many_arguments)]
    pub fn bwd_conv_dw_split(
        &self,
        h: u32,
        w: u32,
        cw: &ConvW,
        relu: u32,
        x: &wgpu::Buffer,
        dy: &wgpu::Buffer,
    ) -> wgpu::Buffer {
        assert!(cw.cout.is_multiple_of(4), "vec4 内核要求 cout 是 4 的倍数");
        let nw = (cw.kk * cw.kk * cw.cin * cw.cout) as usize;
        let p = (h * w) as usize;
        let chunk = (p / 48).max(256);
        let chunks = p.div_ceil(chunk) as u32;
        let u = self.ubuf(&DwU {
            h,
            w,
            cin: cw.cin,
            cout: cw.cout,
            kk: cw.kk,
            pad: cw.kk / 2,
            relu,
            chunk: chunk as u32,
            chunks,
            _pad: 0,
        });
        let part = self.alloc(nw * chunks as usize, "dw-part");
        let bufs = [&u, x, dy, &part];
        // vec4 分片：线程数 = (权重数/4) × 分片数；分片在浮点下标上与标量布局一致
        let nw4 = nw / 4;
        self.dispatch(&self.bwd.conv_dw_k, &bufs, nw4 * chunks as usize);
        let out = self.alloc(nw, "dw");
        let ru = self.ubuf(&RedU {
            n: nw4 as u32,
            chunks,
            _a: 0,
            _b: 0,
        });
        let rbufs = [&ru, &part, &out];
        self.dispatch(&self.bwd.reduce4, &rbufs, nw4);
        out
    }

    /// split-K 版偏置梯度。
    pub fn bwd_conv_db_split(
        &self,
        h: u32,
        w: u32,
        cw: &ConvW,
        relu: u32,
        dy: &wgpu::Buffer,
    ) -> wgpu::Buffer {
        let n = cw.cout as usize;
        let p = (h * w) as usize;
        let chunk = (p / 48).max(256);
        let chunks = p.div_ceil(chunk) as u32;
        let u = self.ubuf(&DwU {
            h,
            w,
            cin: cw.cin,
            cout: cw.cout,
            kk: cw.kk,
            pad: cw.kk / 2,
            relu,
            chunk: chunk as u32,
            chunks,
            _pad: 0,
        });
        let part = self.alloc(n * chunks as usize, "db-part");
        let bufs = [&u, dy, &part];
        self.dispatch(&self.bwd.conv_db_k, &bufs, n * chunks as usize);
        let out = self.alloc(n, "db");
        let ru = self.ubuf(&RedU {
            n: n as u32,
            chunks,
            _a: 0,
            _b: 0,
        });
        let rbufs = [&ru, &part, &out];
        self.dispatch(&self.bwd.reduce, &rbufs, n);
        out
    }

    /// 就地累加 `dst += src`（训练时把每样本梯度攒在显存里，每 accum 次才读回）。
    pub fn add_into(&self, dst: &wgpu::Buffer, src: &wgpu::Buffer, n: usize) {
        let u = self.ubuf(&RedU {
            n: n as u32,
            chunks: 1,
            _a: 0,
            _b: 0,
        });
        let bufs = [&u, src, dst];
        self.dispatch(&self.bwd.add_into, &bufs, n);
    }

    /// 清零一段缓冲。
    pub fn clear(&self, dst: &wgpu::Buffer, n: usize) {
        let u = self.ubuf(&RedU {
            n: n as u32,
            chunks: 1,
            _a: 0,
            _b: 0,
        });
        let bufs = [&u, dst];
        self.dispatch(&self.bwd.clear, &bufs, n);
    }

    /// 逐元素相加（两路梯度汇合）。
    pub fn add(&self, a: &wgpu::Buffer, b: &wgpu::Buffer, n: usize) -> wgpu::Buffer {
        let u = self.ubuf(&AddU {
            n: n as u32,
            _a: 0,
            _b: 0,
            _c: 0,
        });
        let out = self.zeros(n, "add-out");
        let bufs = [&u, a, b, &out];
        self.dispatch(&self.bwd.add, &bufs, n);
        out
    }
}

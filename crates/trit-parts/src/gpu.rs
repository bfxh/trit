//! wgpu 封装：四条计算管线（conv / pool / cat / convt）与缓冲读写。
//!
//! 分派一律 `gx = 256, gy = ceil(total / 16384)`，避开单维 65535 工作组上限。

use wgpu::util::DeviceExt;

use crate::weights::ConvW;

/// 每个工作组处理 64 个元素，256 组 ⇒ 每次分派 16384 个元素。
pub const ROW: u32 = 16384;

pub struct Gpu {
    pub device: wgpu::Device,
    pub queue: wgpu::Queue,
    conv_pl: wgpu::ComputePipeline,
    conv_bgl: wgpu::BindGroupLayout,
    pool_pl: wgpu::ComputePipeline,
    pool_bgl: wgpu::BindGroupLayout,
    cat_pl: wgpu::ComputePipeline,
    cat_bgl: wgpu::BindGroupLayout,
    conv_t4_pl: wgpu::ComputePipeline,
    conv_t4_bgl: wgpu::BindGroupLayout,
    up_pl: wgpu::ComputePipeline,
    up_bgl: wgpu::BindGroupLayout,
    /// 反向管线（与上面四条前向管线共用同一个 device）。
    pub bwd: crate::gpu_bwd::Bwd,
    /// 实际选中的适配器（排查「跑在核显/软渲染上」用）。
    pub adapter: String,
}

fn entry(read_only: bool, binding: u32) -> wgpu::BindGroupLayoutEntry {
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

impl Gpu {
    pub fn new() -> Option<Self> {
        let instance = wgpu::Instance::new(wgpu::InstanceDescriptor::new_without_display_handle());
        let adapter = pollster::block_on(instance.request_adapter(&wgpu::RequestAdapterOptions {
            power_preference: wgpu::PowerPreference::HighPerformance,
            ..Default::default()
        }))
        .ok()?;
        let adapter_name = adapter.get_info().name;
        let alim = adapter.limits();
        let limits = wgpu::Limits {
            max_storage_buffer_binding_size: alim.max_storage_buffer_binding_size,
            max_buffer_size: alim.max_buffer_size,
            ..Default::default()
        };
        let (device, queue) = pollster::block_on(adapter.request_device(&wgpu::DeviceDescriptor {
            required_features: wgpu::Features::empty(),
            required_limits: limits,
            ..Default::default()
        }))
        .ok()?;

        let src = include_str!("shaders.wgsl");
        let module = device.create_shader_module(wgpu::ShaderModuleDescriptor {
            label: Some("parts-head"),
            source: wgpu::ShaderSource::Wgsl(src.into()),
        });
        let mk = |entries: &[wgpu::BindGroupLayoutEntry]| {
            device.create_bind_group_layout(&wgpu::BindGroupLayoutDescriptor {
                label: None,
                entries,
            })
        };
        let conv_bgl = mk(&[
            unif(0),
            entry(true, 1),
            entry(true, 2),
            entry(true, 3),
            entry(true, 4),
            entry(true, 5),
            entry(false, 6),
        ]);
        let pool_bgl = mk(&[unif(0), entry(true, 1), entry(false, 2)]);
        let cat_bgl = mk(&[unif(0), entry(true, 1), entry(true, 2), entry(false, 3)]);
        let conv_t4_bgl = mk(&[
            unif(0),
            entry(true, 1),
            entry(true, 2),
            entry(true, 3),
            entry(false, 4),
        ]);
        let up_bgl = mk(&[
            unif(0),
            entry(true, 1),
            entry(true, 2),
            entry(true, 3),
            entry(true, 4),
            entry(true, 5),
            entry(false, 6),
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
        let bwd = crate::gpu_bwd::Bwd::new(&device);
        Some(Self {
            conv_pl: pipe("conv", &conv_bgl),
            conv_bgl,
            pool_pl: pipe("pool", &pool_bgl),
            pool_bgl,
            cat_pl: pipe("cat", &cat_bgl),
            cat_bgl,
            conv_t4_pl: pipe("conv_f4", &conv_t4_bgl),
            conv_t4_bgl,
            up_pl: pipe("convt", &up_bgl),
            up_bgl,
            bwd,
            adapter: adapter_name,
            device,
            queue,
        })
    }

    pub fn buf(&self, data: &[f32], label: &str) -> wgpu::Buffer {
        let bytes: &[u8] = bytemuck::cast_slice(data);
        let bytes = if bytes.is_empty() {
            &[0u8; 4][..]
        } else {
            bytes
        };
        self.device
            .create_buffer_init(&wgpu::util::BufferInitDescriptor {
                label: Some(label),
                contents: bytes,
                usage: wgpu::BufferUsages::STORAGE
                    | wgpu::BufferUsages::COPY_SRC
                    | wgpu::BufferUsages::COPY_DST,
            })
    }

    pub fn ibuf(&self, data: &[i32], label: &str) -> wgpu::Buffer {
        let bytes: &[u8] = bytemuck::cast_slice(data);
        let bytes = if bytes.is_empty() {
            &[0u8; 4][..]
        } else {
            bytes
        };
        self.device
            .create_buffer_init(&wgpu::util::BufferInitDescriptor {
                label: Some(label),
                contents: bytes,
                usage: wgpu::BufferUsages::STORAGE
                    | wgpu::BufferUsages::COPY_SRC
                    | wgpu::BufferUsages::COPY_DST,
            })
    }

    pub fn empty_out(&self, n: usize, label: &str) -> wgpu::Buffer {
        self.device.create_buffer(&wgpu::BufferDescriptor {
            label: Some(label),
            size: (n * 4) as u64,
            usage: wgpu::BufferUsages::STORAGE | wgpu::BufferUsages::COPY_SRC,
            mapped_at_creation: false,
        })
    }

    pub fn ubuf<T: bytemuck::Pod>(&self, v: &T) -> wgpu::Buffer {
        self.device
            .create_buffer_init(&wgpu::util::BufferInitDescriptor {
                label: Some("uniform"),
                contents: bytemuck::bytes_of(v),
                usage: wgpu::BufferUsages::UNIFORM | wgpu::BufferUsages::COPY_DST,
            })
    }

    /// 等 GPU 做完（计时/调试用）。
    pub fn sync(&self) {
        let _ = self.device.poll(wgpu::PollType::wait_indefinitely());
    }

    /// 覆盖写入一段 f32（训练时参数更新用）。
    pub fn write(&self, buf: &wgpu::Buffer, data: &[f32]) {
        self.queue.write_buffer(buf, 0, bytemuck::cast_slice(data));
    }

    /// 覆盖写入一段 i32。
    pub fn write_i32(&self, buf: &wgpu::Buffer, data: &[i32]) {
        self.queue.write_buffer(buf, 0, bytemuck::cast_slice(data));
    }

    #[allow(clippy::too_many_arguments)]
    pub fn conv(
        &self,
        x: &wgpu::Buffer,
        h: u32,
        w: u32,
        cw: &ConvW,
        mode: u32,
        relu: u32,
        wf: &wgpu::Buffer,
        wt: &wgpu::Buffer,
        ws: &wgpu::Buffer,
        bs: &wgpu::Buffer,
    ) -> wgpu::Buffer {
        let u = ConvU {
            h,
            w,
            cin: cw.cin,
            cout: cw.cout,
            kk: cw.kk,
            pad: cw.kk / 2,
            mode,
            relu,
        };
        let ub = self.ubuf(&u);
        let total = (h * w * cw.cout) as usize;
        let out = self.empty_out(total, "conv-out");
        let bg = self.device.create_bind_group(&wgpu::BindGroupDescriptor {
            label: None,
            layout: &self.conv_bgl,
            entries: &[
                wgpu::BindGroupEntry {
                    binding: 0,
                    resource: ub.as_entire_binding(),
                },
                wgpu::BindGroupEntry {
                    binding: 1,
                    resource: x.as_entire_binding(),
                },
                wgpu::BindGroupEntry {
                    binding: 2,
                    resource: wf.as_entire_binding(),
                },
                wgpu::BindGroupEntry {
                    binding: 3,
                    resource: wt.as_entire_binding(),
                },
                wgpu::BindGroupEntry {
                    binding: 4,
                    resource: ws.as_entire_binding(),
                },
                wgpu::BindGroupEntry {
                    binding: 5,
                    resource: bs.as_entire_binding(),
                },
                wgpu::BindGroupEntry {
                    binding: 6,
                    resource: out.as_entire_binding(),
                },
            ],
        });
        let mut enc = self.device.create_command_encoder(&Default::default());
        {
            let mut pass = enc.begin_compute_pass(&Default::default());
            pass.set_pipeline(&self.conv_pl);
            pass.set_bind_group(0, &bg, &[]);
            pass.dispatch_workgroups(256, div_up((total / 4) as u32, ROW), 1);
        }
        self.queue.submit([enc.finish()]);
        out
    }

    /// 前向卷积的 vec4 版：线程按 oc4，权重用原布局（`[k·cin+ic]·cout+oc`），`cout` 须是 4 的倍数。
    #[allow(clippy::too_many_arguments)]
    pub fn conv_t(
        &self,
        x: &wgpu::Buffer,
        h: u32,
        w: u32,
        cw: &ConvW,
        relu: bool,
        wf: &wgpu::Buffer,
        bias: &wgpu::Buffer,
    ) -> wgpu::Buffer {
        let u = ConvU {
            h,
            w,
            cin: cw.cin,
            cout: cw.cout,
            kk: cw.kk,
            pad: cw.kk / 2,
            mode: 0,
            relu: relu as u32,
        };
        let ub = self.ubuf(&u);
        let total = (h * w * cw.cout) as usize;
        let out = self.empty_out(total, "conv-out");
        let bg = self.device.create_bind_group(&wgpu::BindGroupDescriptor {
            label: None,
            layout: &self.conv_t4_bgl,
            entries: &[
                wgpu::BindGroupEntry {
                    binding: 0,
                    resource: ub.as_entire_binding(),
                },
                wgpu::BindGroupEntry {
                    binding: 1,
                    resource: x.as_entire_binding(),
                },
                wgpu::BindGroupEntry {
                    binding: 2,
                    resource: wf.as_entire_binding(),
                },
                wgpu::BindGroupEntry {
                    binding: 3,
                    resource: bias.as_entire_binding(),
                },
                wgpu::BindGroupEntry {
                    binding: 4,
                    resource: out.as_entire_binding(),
                },
            ],
        });
        let mut enc = self.device.create_command_encoder(&Default::default());
        {
            let mut pass = enc.begin_compute_pass(&Default::default());
            pass.set_pipeline(&self.conv_t4_pl);
            pass.set_bind_group(0, &bg, &[]);
            pass.dispatch_workgroups(256, div_up((total / 4) as u32, ROW), 1);
        }
        self.queue.submit([enc.finish()]);
        out
    }

    pub fn pool(&self, x: &wgpu::Buffer, h: u32, w: u32, c: u32) -> wgpu::Buffer {
        let u = PoolU { h, w, c, _p: 0 };
        let ub = self.ubuf(&u);
        let total = ((h / 2) * (w / 2) * c) as usize;
        let out = self.empty_out(total, "pool-out");
        let bg = self.device.create_bind_group(&wgpu::BindGroupDescriptor {
            label: None,
            layout: &self.pool_bgl,
            entries: &[
                wgpu::BindGroupEntry {
                    binding: 0,
                    resource: ub.as_entire_binding(),
                },
                wgpu::BindGroupEntry {
                    binding: 1,
                    resource: x.as_entire_binding(),
                },
                wgpu::BindGroupEntry {
                    binding: 2,
                    resource: out.as_entire_binding(),
                },
            ],
        });
        let mut enc = self.device.create_command_encoder(&Default::default());
        {
            let mut pass = enc.begin_compute_pass(&Default::default());
            pass.set_pipeline(&self.pool_pl);
            pass.set_bind_group(0, &bg, &[]);
            pass.dispatch_workgroups(256, div_up((total / 4) as u32, ROW), 1);
        }
        self.queue.submit([enc.finish()]);
        out
    }

    pub fn cat(
        &self,
        a: &wgpu::Buffer,
        b: &wgpu::Buffer,
        hw: u32,
        ca: u32,
        cb: u32,
    ) -> wgpu::Buffer {
        let u = CatU { hw, ca, cb, _p: 0 };
        let ub = self.ubuf(&u);
        let total = (hw * (ca + cb)) as usize;
        let out = self.empty_out(total, "cat-out");
        let bg = self.device.create_bind_group(&wgpu::BindGroupDescriptor {
            label: None,
            layout: &self.cat_bgl,
            entries: &[
                wgpu::BindGroupEntry {
                    binding: 0,
                    resource: ub.as_entire_binding(),
                },
                wgpu::BindGroupEntry {
                    binding: 1,
                    resource: a.as_entire_binding(),
                },
                wgpu::BindGroupEntry {
                    binding: 2,
                    resource: b.as_entire_binding(),
                },
                wgpu::BindGroupEntry {
                    binding: 3,
                    resource: out.as_entire_binding(),
                },
            ],
        });
        let mut enc = self.device.create_command_encoder(&Default::default());
        {
            let mut pass = enc.begin_compute_pass(&Default::default());
            pass.set_pipeline(&self.cat_pl);
            pass.set_bind_group(0, &bg, &[]);
            pass.dispatch_workgroups(256, div_up((total / 4) as u32, ROW), 1);
        }
        self.queue.submit([enc.finish()]);
        out
    }

    #[allow(clippy::too_many_arguments)]
    pub fn convt(
        &self,
        x: &wgpu::Buffer,
        h: u32,
        w: u32,
        cw: &ConvW,
        mode: u32,
        wf: &wgpu::Buffer,
        wt: &wgpu::Buffer,
        ws: &wgpu::Buffer,
        bs: &wgpu::Buffer,
    ) -> wgpu::Buffer {
        let u = UpU {
            h,
            w,
            cin: cw.cin,
            cout: cw.cout,
            mode,
            _a: 0,
            _b: 0,
            _c: 0,
        };
        let ub = self.ubuf(&u);
        let total = ((h * 2) * (w * 2) * cw.cout) as usize;
        let out = self.empty_out(total, "convt-out");
        let bg = self.device.create_bind_group(&wgpu::BindGroupDescriptor {
            label: None,
            layout: &self.up_bgl,
            entries: &[
                wgpu::BindGroupEntry {
                    binding: 0,
                    resource: ub.as_entire_binding(),
                },
                wgpu::BindGroupEntry {
                    binding: 1,
                    resource: x.as_entire_binding(),
                },
                wgpu::BindGroupEntry {
                    binding: 2,
                    resource: wf.as_entire_binding(),
                },
                wgpu::BindGroupEntry {
                    binding: 3,
                    resource: wt.as_entire_binding(),
                },
                wgpu::BindGroupEntry {
                    binding: 4,
                    resource: ws.as_entire_binding(),
                },
                wgpu::BindGroupEntry {
                    binding: 5,
                    resource: bs.as_entire_binding(),
                },
                wgpu::BindGroupEntry {
                    binding: 6,
                    resource: out.as_entire_binding(),
                },
            ],
        });
        let mut enc = self.device.create_command_encoder(&Default::default());
        {
            let mut pass = enc.begin_compute_pass(&Default::default());
            pass.set_pipeline(&self.up_pl);
            pass.set_bind_group(0, &bg, &[]);
            pass.dispatch_workgroups(256, div_up((total / 4) as u32, ROW), 1);
        }
        self.queue.submit([enc.finish()]);
        out
    }

    pub fn read(&self, src: &wgpu::Buffer, n: usize) -> Vec<f32> {
        let bytes = (n * 4) as u64;
        let staging = self.device.create_buffer(&wgpu::BufferDescriptor {
            label: Some("staging"),
            size: bytes,
            usage: wgpu::BufferUsages::MAP_READ | wgpu::BufferUsages::COPY_DST,
            mapped_at_creation: false,
        });
        let mut enc = self.device.create_command_encoder(&Default::default());
        enc.copy_buffer_to_buffer(src, 0, &staging, 0, bytes);
        self.queue.submit([enc.finish()]);
        let slice = staging.slice(..);
        slice.map_async(wgpu::MapMode::Read, |_| {});
        let _ = self.device.poll(wgpu::PollType::wait_indefinitely());
        let data = slice.get_mapped_range().expect("map").to_vec();
        data.as_chunks::<4>()
            .0
            .iter()
            .map(|c| f32::from_le_bytes(*c))
            .collect()
    }
}

pub fn div_up(a: u32, b: u32) -> u32 {
    a.div_ceil(b)
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

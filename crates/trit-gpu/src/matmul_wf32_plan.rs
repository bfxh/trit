//! WF32 的「持久缓冲计划」：同一 `(m, n, k)` 形状只分配一次设备缓冲。
//!
//! [`crate::matmul_wf32`] 的 `run*` 每次都重建 A/B/out/staging，适合一次性调用，
//! 但生成器逐层推理会反复用同一形状：权重是常量、激活每帧换。这里把缓冲留住，
//! 每轮只剩「写激活 → 派发 → 回读」三段，并允许把多个层合进一个 submit（[`run_batch`]）。

use wgpu::util::DeviceExt;

use crate::matmul_wf32::{WORKGROUP, Wf32Gpu};

/// 三种 WF32 内核，与 [`Wf32Gpu`] 里的三条 pipeline 一一对应。
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum Wf32Kernel {
    /// 一线程一输出（基线）。
    Scalar,
    /// 四列阻塞 + vec4 装载。
    Reg4,
    /// 16×16 共享内存 tile。
    Tiled,
}

impl Wf32Kernel {
    /// 按形状选核：m、n 都够大走 tiled；否则 n>=4 走四列；再否则标量。
    pub fn pick(m: usize, n: usize) -> Self {
        if m >= 16 && n >= 16 {
            Self::Tiled
        } else if n >= 4 {
            Self::Reg4
        } else {
            Self::Scalar
        }
    }

    fn grid(self, m: usize, n: usize) -> (u32, u32, u32) {
        match self {
            Self::Scalar => ((((m * n) as u32).div_ceil(WORKGROUP)), 1, 1),
            Self::Reg4 => (
                ((m as u32) * (n as u32).div_ceil(4)).div_ceil(WORKGROUP),
                1,
                1,
            ),
            Self::Tiled => ((n as u32).div_ceil(16), (m as u32).div_ceil(16), 1),
        }
    }

    fn pipeline(self, gpu: &Wf32Gpu) -> &wgpu::ComputePipeline {
        match self {
            Self::Scalar => &gpu.pipeline,
            Self::Reg4 => &gpu.pipeline_reg4,
            Self::Tiled => &gpu.pipeline_tiled,
        }
    }
}

/// 一次 GPU 计算的三段墙钟耗时（毫秒）。
///
/// `compute_ms` 夹的是 submit + poll 到完成，不含建缓冲、不含上传与回读；
/// 它是「派发 + 内核」而非驱动时间戳，仍比端到端口径更接近内核本身。
#[derive(Clone, Copy, Default, Debug)]
pub struct Wf32Phases {
    pub compute_ms: f64,
    pub readback_ms: f64,
}

fn ms(elapsed: std::time::Duration) -> f64 {
    elapsed.as_secs_f64() * 1000.0
}

/// 复用设备缓冲的 WF32 计划：权重常驻，激活可换，输出留在显存里反复覆写。
pub struct Wf32Plan<'g> {
    gpu: &'g Wf32Gpu,
    m: usize,
    n: usize,
    k: usize,
    words: usize,
    a_pos: wgpu::Buffer,
    a_neg: wgpu::Buffer,
    b: wgpu::Buffer,
    out: wgpu::Buffer,
    staging: wgpu::Buffer,
    bind: wgpu::BindGroup,
}

impl<'g> Wf32Plan<'g> {
    /// 建缓冲并一次性灌入权重与激活。
    pub fn new(
        gpu: &'g Wf32Gpu,
        m: usize,
        n: usize,
        k: usize,
        a_pos: &[u32],
        a_neg: &[u32],
        b: &[f32],
    ) -> Self {
        let words = k.div_ceil(32);
        assert_eq!(a_pos.len(), m * words, "a_pos 长度");
        assert_eq!(a_neg.len(), m * words, "a_neg 长度");
        assert_eq!(b.len(), n * words * 32, "b 长度（每列补齐到 words*32）");

        let upload = |label: &str, contents: &[u8]| {
            let buffer = gpu.device.create_buffer(&wgpu::BufferDescriptor {
                label: Some(label),
                size: contents.len() as u64,
                usage: wgpu::BufferUsages::STORAGE | wgpu::BufferUsages::COPY_DST,
                mapped_at_creation: false,
            });
            gpu.queue.write_buffer(&buffer, 0, contents);
            buffer
        };
        let upload_u32 = |label: &str, data: &[u32]| upload(label, bytemuck::cast_slice(data));
        let params: [u32; 4] = [m as u32, n as u32, k as u32, words as u32];
        let out_bytes = (m * n * 4) as u64;

        let a_pos_buf = upload_u32("wf32-a-pos", a_pos);
        let a_neg_buf = upload_u32("wf32-a-neg", a_neg);
        let b_buf = upload("wf32-b", bytemuck::cast_slice(b));
        let out = gpu.device.create_buffer(&wgpu::BufferDescriptor {
            label: Some("wf32-out"),
            size: out_bytes,
            usage: wgpu::BufferUsages::STORAGE | wgpu::BufferUsages::COPY_SRC,
            mapped_at_creation: false,
        });
        let staging = gpu.device.create_buffer(&wgpu::BufferDescriptor {
            label: Some("wf32-staging"),
            size: out_bytes,
            usage: wgpu::BufferUsages::MAP_READ | wgpu::BufferUsages::COPY_DST,
            mapped_at_creation: false,
        });
        let params_buf = gpu
            .device
            .create_buffer_init(&wgpu::util::BufferInitDescriptor {
                label: Some("wf32-params"),
                contents: bytemuck::cast_slice(&params),
                usage: wgpu::BufferUsages::UNIFORM,
            });

        // 三条 pipeline 共用一个 bind group layout，所以 bind group 也可复用。
        let bind = gpu.device.create_bind_group(&wgpu::BindGroupDescriptor {
            label: None,
            layout: &gpu.layout,
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
                    resource: b_buf.as_entire_binding(),
                },
                wgpu::BindGroupEntry {
                    binding: 3,
                    resource: out.as_entire_binding(),
                },
                wgpu::BindGroupEntry {
                    binding: 4,
                    resource: params_buf.as_entire_binding(),
                },
            ],
        });

        Self {
            gpu,
            m,
            n,
            k,
            words,
            a_pos: a_pos_buf,
            a_neg: a_neg_buf,
            b: b_buf,
            out,
            staging,
            bind,
        }
    }

    pub fn shape(&self) -> (usize, usize, usize) {
        (self.m, self.n, self.k)
    }

    pub fn words(&self) -> usize {
        self.words
    }

    /// 换权重（长度必须与建计划时一致）。
    pub fn set_weights(&self, a_pos: &[u32], a_neg: &[u32]) {
        assert_eq!(a_pos.len(), self.m * self.words, "a_pos 长度");
        assert_eq!(a_neg.len(), self.m * self.words, "a_neg 长度");
        self.gpu
            .queue
            .write_buffer(&self.a_pos, 0, bytemuck::cast_slice(a_pos));
        self.gpu
            .queue
            .write_buffer(&self.a_neg, 0, bytemuck::cast_slice(a_neg));
    }

    /// 换激活（每个 batch 都不同，这是唯一必须每轮上传的张量）。
    pub fn set_activations(&self, b: &[f32]) {
        assert_eq!(b.len(), self.n * self.words * 32, "b 长度");
        self.gpu
            .queue
            .write_buffer(&self.b, 0, bytemuck::cast_slice(b));
    }

    /// 把一次派发写进调用者的 encoder：合批入口。
    pub fn encode_compute(
        &self,
        encoder: &mut wgpu::CommandEncoder,
        kernel: Wf32Kernel,
        grid: (u32, u32, u32),
    ) {
        let mut pass = encoder.begin_compute_pass(&wgpu::ComputePassDescriptor::default());
        pass.set_pipeline(kernel.pipeline(self.gpu));
        pass.set_bind_group(0, &self.bind, &[]);
        pass.dispatch_workgroups(grid.0, grid.1, grid.2);
    }

    /// 把 out → staging 的拷贝写进调用者的 encoder。
    pub fn encode_readback(&self, encoder: &mut wgpu::CommandEncoder) {
        let bytes = (self.m * self.n * 4) as u64;
        encoder.copy_buffer_to_buffer(&self.out, 0, &self.staging, 0, bytes);
    }

    /// `kernel` 为 `None` 时按形状自动选核。
    pub fn kernel_for(&self, kernel: Option<Wf32Kernel>) -> Wf32Kernel {
        kernel.unwrap_or_else(|| Wf32Kernel::pick(self.m, self.n))
    }

    fn grid(&self, kernel: Wf32Kernel) -> (u32, u32, u32) {
        kernel.grid(self.m, self.n)
    }

    /// 计算 + 回读合进一个 submit，然后映射读回。单次推理的推荐路径。
    pub fn run(&self, kernel: Option<Wf32Kernel>) -> Vec<f32> {
        let chosen = self.kernel_for(kernel);
        let grid = self.grid(chosen);
        let mut encoder = self
            .gpu
            .device
            .create_command_encoder(&wgpu::CommandEncoderDescriptor { label: None });
        self.encode_compute(&mut encoder, chosen, grid);
        self.encode_readback(&mut encoder);
        self.gpu.queue.submit([encoder.finish()]);
        self.read_back()
    }

    /// 分两次 submit，分别夹出「派发 + 内核」和「拷贝 + 映射读回」的墙钟。
    pub fn run_timed(&self, kernel: Option<Wf32Kernel>) -> (Vec<f32>, Wf32Phases) {
        let chosen = self.kernel_for(kernel);
        let grid = self.grid(chosen);

        let mut encoder = self
            .gpu
            .device
            .create_command_encoder(&wgpu::CommandEncoderDescriptor { label: None });
        self.encode_compute(&mut encoder, chosen, grid);

        let start = std::time::Instant::now();
        self.gpu.queue.submit([encoder.finish()]);
        self.gpu
            .device
            .poll(wgpu::PollType::wait_indefinitely())
            .expect("poll compute");
        let compute_ms = ms(start.elapsed());

        let mut encoder = self
            .gpu
            .device
            .create_command_encoder(&wgpu::CommandEncoderDescriptor { label: None });
        self.encode_readback(&mut encoder);

        let start = std::time::Instant::now();
        self.gpu.queue.submit([encoder.finish()]);
        let out = self.read_back();
        let readback_ms = ms(start.elapsed());

        (
            out,
            Wf32Phases {
                compute_ms,
                readback_ms,
            },
        )
    }

    /// `copy out→staging` + 映射读回（自带一次 submit）。
    pub fn copy_and_read(&self) -> Vec<f32> {
        let mut encoder = self
            .gpu
            .device
            .create_command_encoder(&wgpu::CommandEncoderDescriptor { label: None });
        self.encode_readback(&mut encoder);
        self.gpu.queue.submit([encoder.finish()]);
        self.read_back()
    }

    fn begin_map(&self) {
        self.staging
            .slice(..)
            .map_async(wgpu::MapMode::Read, |_| {});
    }

    fn finish_map(&self) -> Vec<f32> {
        let data = {
            let view = self.staging.slice(..).get_mapped_range().expect("map");
            view.to_vec()
        };
        // wgpu 30：BufferView 的 Drop 只摘掉 host 侧映射记录，显存侧要显式
        // `unmap()`，否则下一次 submit 判「staging 仍 mapped」。
        self.staging.unmap();
        data.as_chunks::<4>()
            .0
            .iter()
            .map(|c| f32::from_le_bytes(*c))
            .collect()
    }

    /// 等 GPU 把 staging 写完后读回（会 poll 到完成）。
    fn read_back(&self) -> Vec<f32> {
        self.begin_map();
        self.gpu
            .device
            .poll(wgpu::PollType::wait_indefinitely())
            .expect("poll map");
        self.finish_map()
    }
}

/// 多个层合进一个 submit：所有内核派发 + 所有回读拷贝，再一次映射等待。
///
/// `kernel` 为 `None` 时每个层各自按形状选核。返回顺序与 `plans` 一致。
pub fn run_batch(plans: &[&Wf32Plan<'_>], kernel: Option<Wf32Kernel>) -> Vec<Vec<f32>> {
    let Some(first) = plans.first() else {
        return Vec::new();
    };
    let mut encoder = first
        .gpu
        .device
        .create_command_encoder(&wgpu::CommandEncoderDescriptor {
            label: Some("wf32-batch"),
        });
    for plan in plans {
        let chosen = plan.kernel_for(kernel);
        let grid = plan.grid(chosen);
        plan.encode_compute(&mut encoder, chosen, grid);
    }
    for plan in plans {
        plan.encode_readback(&mut encoder);
    }
    first.gpu.queue.submit([encoder.finish()]);
    for plan in plans {
        plan.begin_map();
    }
    first
        .gpu
        .device
        .poll(wgpu::PollType::wait_indefinitely())
        .expect("poll batch");
    plans.iter().map(|plan| plan.finish_map()).collect()
}

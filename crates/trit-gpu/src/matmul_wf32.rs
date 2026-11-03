//! 三值权重 × f32 激活的 GPU 矩阵乘（WGSL）。
//!
//! 与 [`crate::matmul`] 的「三值 × 三值」不同：B 是 f32 激活，A 仍是双导轨三值权重。
//! 用途是一步生成器的 conv：im2col 后的补丁是 f32，权重是三值。

use wgpu::util::DeviceExt;

use crate::bench::{storage, storage_with, uniform};

pub const WORKGROUP: u32 = 64;

struct Wf32Run<'a> {
    grid: (u32, u32, u32),
    m: usize,
    n: usize,
    k: usize,
    a_pos: &'a [u32],
    a_neg: &'a [u32],
    b: &'a [f32],
}

/// 复用 device / pipeline 的 WF32 执行器；每次 `run` 只建数据缓冲。
pub struct Wf32Gpu {
    device: wgpu::Device,
    queue: wgpu::Queue,
    pipeline: wgpu::ComputePipeline,
    pipeline_reg4: wgpu::ComputePipeline,
    pipeline_tiled: wgpu::ComputePipeline,
    layout: wgpu::BindGroupLayout,
}

impl Wf32Gpu {
    pub fn new() -> Option<Self> {
        pollster::block_on(Self::new_async())
    }

    async fn new_async() -> Option<Self> {
        let instance = wgpu::Instance::new(wgpu::InstanceDescriptor::new_without_display_handle());
        let adapter = instance
            .request_adapter(&wgpu::RequestAdapterOptions::default())
            .await
            .ok()?;
        let (device, queue) = adapter
            .request_device(&wgpu::DeviceDescriptor::default())
            .await
            .ok()?;
        let shader = device.create_shader_module(wgpu::ShaderModuleDescriptor {
            label: Some("matmul-wf32"),
            source: wgpu::ShaderSource::Wgsl(include_str!("matmul_wf32.wgsl").into()),
        });
        let layout = device.create_bind_group_layout(&wgpu::BindGroupLayoutDescriptor {
            label: None,
            entries: &[
                storage(0),
                storage(1),
                storage(2),
                storage_with(3, false),
                uniform(4),
            ],
        });
        let pipeline_layout = device.create_pipeline_layout(&wgpu::PipelineLayoutDescriptor {
            label: None,
            bind_group_layouts: &[Some(&layout)],
            immediate_size: 0,
        });
        let pipeline = device.create_compute_pipeline(&wgpu::ComputePipelineDescriptor {
            label: Some("matmul-wf32"),
            layout: Some(&pipeline_layout),
            module: &shader,
            entry_point: Some("main"),
            compilation_options: Default::default(),
            cache: None,
        });
        let shader_reg4 = device.create_shader_module(wgpu::ShaderModuleDescriptor {
            label: Some("matmul-wf32-reg4"),
            source: wgpu::ShaderSource::Wgsl(include_str!("matmul_wf32_reg4.wgsl").into()),
        });
        let pipeline_reg4 = device.create_compute_pipeline(&wgpu::ComputePipelineDescriptor {
            label: Some("matmul-wf32-reg4"),
            layout: Some(&pipeline_layout),
            module: &shader_reg4,
            entry_point: Some("main"),
            compilation_options: Default::default(),
            cache: None,
        });
        let shader_tiled = device.create_shader_module(wgpu::ShaderModuleDescriptor {
            label: Some("matmul-wf32-tiled"),
            source: wgpu::ShaderSource::Wgsl(include_str!("matmul_wf32_tiled.wgsl").into()),
        });
        let pipeline_tiled = device.create_compute_pipeline(&wgpu::ComputePipelineDescriptor {
            label: Some("matmul-wf32-tiled"),
            layout: Some(&pipeline_layout),
            module: &shader_tiled,
            entry_point: Some("main"),
            compilation_options: Default::default(),
            cache: None,
        });
        Some(Self {
            device,
            queue,
            pipeline,
            pipeline_reg4,
            pipeline_tiled,
            layout,
        })
    }

    /// `C[m][n] = Σ_k A[m][k]·B[n][k]`。
    ///
    /// - `a_pos/a_neg`：`m` 行 × `words` 字（`words = ceil(k/32)`）；
    /// - `b`：`n` 列 × `words*32` 个 f32（每列补齐，列主序）；
    /// - 返回 `m × n` 行主序 f32。
    fn run_with(&self, pipeline: &wgpu::ComputePipeline, run: Wf32Run<'_>) -> Vec<f32> {
        let Wf32Run {
            grid,
            m,
            n,
            k,
            a_pos,
            a_neg,
            b,
        } = run;
        let words = k.div_ceil(32);
        assert_eq!(a_pos.len(), m * words, "a_pos 长度");
        assert_eq!(a_neg.len(), m * words, "a_neg 长度");
        assert_eq!(b.len(), n * words * 32, "b 长度（每列补齐到 words*32）");

        let make_u32 = |data: &[u32]| {
            self.device
                .create_buffer_init(&wgpu::util::BufferInitDescriptor {
                    label: None,
                    contents: bytemuck::cast_slice(data),
                    usage: wgpu::BufferUsages::STORAGE,
                })
        };
        let make_f32 = |data: &[f32]| {
            self.device
                .create_buffer_init(&wgpu::util::BufferInitDescriptor {
                    label: None,
                    contents: bytemuck::cast_slice(data),
                    usage: wgpu::BufferUsages::STORAGE,
                })
        };
        let a_pos_buf = make_u32(a_pos);
        let a_neg_buf = make_u32(a_neg);
        let b_buf = make_f32(b);
        let out = self.device.create_buffer(&wgpu::BufferDescriptor {
            label: None,
            size: (m * n * 4) as u64,
            usage: wgpu::BufferUsages::STORAGE | wgpu::BufferUsages::COPY_SRC,
            mapped_at_creation: false,
        });

        let params: [u32; 4] = [m as u32, n as u32, k as u32, words as u32];
        let params_buf = self
            .device
            .create_buffer_init(&wgpu::util::BufferInitDescriptor {
                label: None,
                contents: bytemuck::cast_slice(&params),
                usage: wgpu::BufferUsages::UNIFORM,
            });
        let bind = self.device.create_bind_group(&wgpu::BindGroupDescriptor {
            label: None,
            layout: &self.layout,
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

        let mut encoder = self
            .device
            .create_command_encoder(&wgpu::CommandEncoderDescriptor { label: None });
        {
            let mut pass = encoder.begin_compute_pass(&wgpu::ComputePassDescriptor::default());
            pass.set_pipeline(pipeline);
            pass.set_bind_group(0, &bind, &[]);
            pass.dispatch_workgroups(grid.0, grid.1, grid.2);
        }
        self.queue.submit([encoder.finish()]);

        let staging = self.device.create_buffer(&wgpu::BufferDescriptor {
            label: None,
            size: (m * n * 4) as u64,
            usage: wgpu::BufferUsages::MAP_READ | wgpu::BufferUsages::COPY_DST,
            mapped_at_creation: false,
        });
        let mut encoder = self
            .device
            .create_command_encoder(&wgpu::CommandEncoderDescriptor { label: None });
        encoder.copy_buffer_to_buffer(&out, 0, &staging, 0, (m * n * 4) as u64);
        self.queue.submit([encoder.finish()]);
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

    /// 原始内核：一线程一输出（保留为 A/B 基线）。
    pub fn run(
        &self,
        m: usize,
        n: usize,
        k: usize,
        a_pos: &[u32],
        a_neg: &[u32],
        b: &[f32],
    ) -> Vec<f32> {
        self.run_with(
            &self.pipeline,
            Wf32Run {
                grid: (((m * n) as u32).div_ceil(WORKGROUP), 1, 1),
                m,
                n,
                k,
                a_pos,
                a_neg,
                b,
            },
        )
    }

    /// 四列阻塞：每个 k word 的 A 位只取一次，B 用 vec4 装载。
    pub fn run_reg4(
        &self,
        m: usize,
        n: usize,
        k: usize,
        a_pos: &[u32],
        a_neg: &[u32],
        b: &[f32],
    ) -> Vec<f32> {
        let groups = (n as u32).div_ceil(4);
        let total = (m as u32) * groups;
        self.run_with(
            &self.pipeline_reg4,
            Wf32Run {
                grid: (total.div_ceil(WORKGROUP), 1, 1),
                m,
                n,
                k,
                a_pos,
                a_neg,
                b,
            },
        )
    }

    /// 16×16 共享内存 tile：B 在 workgroup 内被 16 行复用。
    pub fn run_tiled(
        &self,
        m: usize,
        n: usize,
        k: usize,
        a_pos: &[u32],
        a_neg: &[u32],
        b: &[f32],
    ) -> Vec<f32> {
        let grid = ((n as u32).div_ceil(16), (m as u32).div_ceil(16), 1);
        self.run_with(
            &self.pipeline_tiled,
            Wf32Run {
                grid,
                m,
                n,
                k,
                a_pos,
                a_neg,
                b,
            },
        )
    }

    /// 自动选核：m,n 都够大走 tiled；否则 n>=4 走四列；再否则标量。
    pub fn run_auto(
        &self,
        m: usize,
        n: usize,
        k: usize,
        a_pos: &[u32],
        a_neg: &[u32],
        b: &[f32],
    ) -> Vec<f32> {
        if m >= 16 && n >= 16 {
            self.run_tiled(m, n, k, a_pos, a_neg, b)
        } else if n >= 4 {
            self.run_reg4(m, n, k, a_pos, a_neg, b)
        } else {
            self.run(m, n, k, a_pos, a_neg, b)
        }
    }
}

/// 便捷入口：每次新建执行器（基准请复用 [`Wf32Gpu`]）。
pub fn matmul_wf32(
    m: usize,
    n: usize,
    k: usize,
    a_pos: &[u32],
    a_neg: &[u32],
    b: &[f32],
) -> Option<Vec<f32>> {
    Some(Wf32Gpu::new()?.run_auto(m, n, k, a_pos, a_neg, b))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn pack_rows(vals: &[i32], k: usize) -> (Vec<u32>, Vec<u32>) {
        let words = k.div_ceil(32);
        let rows = vals.len() / k;
        let mut pos = vec![0u32; rows * words];
        let mut neg = vec![0u32; rows * words];
        for r in 0..rows {
            for c in 0..k {
                match vals[r * k + c] {
                    1 => pos[r * words + c / 32] |= 1u32 << (c % 32),
                    -1 => neg[r * words + c / 32] |= 1u32 << (c % 32),
                    _ => {}
                }
            }
        }
        (pos, neg)
    }

    #[test]
    fn gpu_wf32_matches_cpu() {
        let (m, n, k) = (7usize, 9usize, 100usize);
        let words = k.div_ceil(32);
        let a: Vec<i32> = (0..m * k).map(|i| ((i * 7 + 1) % 3) as i32 - 1).collect();
        let b: Vec<f32> = (0..n * words * 32)
            .map(|i| ((i * 13 % 17) as f32) / 17.0)
            .collect();
        let (a_pos, a_neg) = pack_rows(&a, k);
        let Some(out) = matmul_wf32(m, n, k, &a_pos, &a_neg, &b) else {
            eprintln!("无 GPU adapter，跳过");
            return;
        };
        for r in 0..m {
            for c in 0..n {
                let mut want = 0f32;
                for kk in 0..k {
                    want += a[r * k + kk] as f32 * b[c * words * 32 + kk];
                }
                let got = out[r * n + c];
                assert!(
                    (got - want).abs() < 1e-4,
                    "r={r} c={c} got={got} want={want}"
                );
            }
        }
    }

    #[test]
    fn gpu_wf32_reg4_matches_cpu_and_scalar() {
        let Some(gpu) = Wf32Gpu::new() else {
            eprintln!("无 GPU adapter，跳过");
            return;
        };
        for (m, n, k) in [(7usize, 9usize, 100usize), (5, 8, 37), (4, 64, 100)] {
            let words = k.div_ceil(32);
            let a: Vec<i32> = (0..m * k).map(|i| ((i * 7 + 1) % 3) as i32 - 1).collect();
            let b: Vec<f32> = (0..n * words * 32)
                .map(|i| ((i * 13 % 17) as f32) / 17.0)
                .collect();
            let (a_pos, a_neg) = pack_rows(&a, k);
            let old = gpu.run(m, n, k, &a_pos, &a_neg, &b);
            let new = gpu.run_reg4(m, n, k, &a_pos, &a_neg, &b);
            let tiled = gpu.run_tiled(m, n, k, &a_pos, &a_neg, &b);
            assert_eq!(old.len(), m * n);
            assert_eq!(new.len(), m * n);
            assert_eq!(tiled.len(), m * n);
            for r in 0..m {
                for c in 0..n {
                    let mut want = 0f32;
                    for kk in 0..k {
                        want += a[r * k + kk] as f32 * b[c * words * 32 + kk];
                    }
                    let o = old[r * n + c];
                    let v = new[r * n + c];
                    let t = tiled[r * n + c];
                    assert!((o - want).abs() < 1e-4, "old r={r} c={c} {o} != {want}");
                    assert!((v - want).abs() < 1e-4, "reg4 r={r} c={c} {v} != {want}");
                    assert!((t - want).abs() < 1e-4, "tiled r={r} c={c} {t} != {want}");
                }
            }
        }
    }
}

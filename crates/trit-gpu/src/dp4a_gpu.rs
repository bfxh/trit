//! **DP4A 内核的 GPU 执行器**（`gpu` feature 下编译）。
//!
//! 与 `dp4a.rs` 的 CPU 参考严格同构：输入是**已打包**的 `u32` 数组（K 方向连续、4 个 int8/u32，
//! B 预转置成 N×K），输出 `f32` 的整型累加（scale 由调用方做）。
//!
//! 判据（测试里）：GPU 结果必须与 `reference_packed` **逐元素相等**（整型累加 ⇒ 应当精确相等；
//! 若不等，先查打包布局，不要先怀疑精度）。

#![cfg(feature = "gpu")]

use wgpu::util::DeviceExt;

/// 复用 device / pipeline 的 DP4A 执行器；每次 `run` 只建数据缓冲。
pub struct Dp4aGpu {
    device: wgpu::Device,
    queue: wgpu::Queue,
    pipeline: wgpu::ComputePipeline,
    layout: wgpu::BindGroupLayout,
}

impl Dp4aGpu {
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
            label: Some("dp4a"),
            source: wgpu::ShaderSource::Wgsl(include_str!("dp4a.wgsl").into()),
        });
        let layout = device.create_bind_group_layout(&wgpu::BindGroupLayoutDescriptor {
            label: None,
            entries: &[
                wgpu::BindGroupLayoutEntry {
                    binding: 0,
                    visibility: wgpu::ShaderStages::COMPUTE,
                    ty: wgpu::BindingType::Buffer {
                        ty: wgpu::BufferBindingType::Storage { read_only: true },
                        has_dynamic_offset: false,
                        min_binding_size: None,
                    },
                    count: None,
                },
                wgpu::BindGroupLayoutEntry {
                    binding: 1,
                    visibility: wgpu::ShaderStages::COMPUTE,
                    ty: wgpu::BindingType::Buffer {
                        ty: wgpu::BufferBindingType::Storage { read_only: true },
                        has_dynamic_offset: false,
                        min_binding_size: None,
                    },
                    count: None,
                },
                wgpu::BindGroupLayoutEntry {
                    binding: 2,
                    visibility: wgpu::ShaderStages::COMPUTE,
                    ty: wgpu::BindingType::Buffer {
                        ty: wgpu::BufferBindingType::Storage { read_only: false },
                        has_dynamic_offset: false,
                        min_binding_size: None,
                    },
                    count: None,
                },
                wgpu::BindGroupLayoutEntry {
                    binding: 3,
                    visibility: wgpu::ShaderStages::COMPUTE,
                    ty: wgpu::BindingType::Buffer {
                        ty: wgpu::BufferBindingType::Uniform,
                        has_dynamic_offset: false,
                        min_binding_size: None,
                    },
                    count: None,
                },
            ],
        });
        let pl = device.create_pipeline_layout(&wgpu::PipelineLayoutDescriptor {
            label: None,
            bind_group_layouts: &[Some(&layout)],
            immediate_size: 0,
        });
        let pipeline = device.create_compute_pipeline(&wgpu::ComputePipelineDescriptor {
            label: Some("dp4a"),
            layout: Some(&pl),
            module: &shader,
            entry_point: Some("main"),
            compilation_options: Default::default(),
            cache: None,
        });
        Some(Self {
            device,
            queue,
            pipeline,
            layout,
        })
    }

    /// `a`：M×K（每行 `k_words` 字）；`bt`：N×K（每列一行，`k_words` 字）；返回 M×N 的整型累加。
    pub fn run(&self, a: &[u32], bt: &[u32], m: u32, n: u32, k_words: u32) -> Vec<f32> {
        let dev = &self.device;
        let buf = |data: &[u32]| -> wgpu::Buffer {
            dev.create_buffer_init(&wgpu::util::BufferInitDescriptor {
                label: None,
                contents: bytemuck::cast_slice(data),
                usage: wgpu::BufferUsages::STORAGE,
            })
        };
        let a_buf = buf(a);
        let b_buf = buf(bt);
        let out_len = (m * n) as usize;
        let c_buf = dev.create_buffer(&wgpu::BufferDescriptor {
            label: None,
            size: (out_len * 4) as u64,
            usage: wgpu::BufferUsages::STORAGE | wgpu::BufferUsages::COPY_SRC,
            mapped_at_creation: false,
        });
        let dims = [m, n, k_words, 0u32];
        let d_buf = dev.create_buffer_init(&wgpu::util::BufferInitDescriptor {
            label: None,
            contents: bytemuck::cast_slice(&dims),
            usage: wgpu::BufferUsages::UNIFORM,
        });
        let bg = dev.create_bind_group(&wgpu::BindGroupDescriptor {
            label: None,
            layout: &self.layout,
            entries: &[
                wgpu::BindGroupEntry {
                    binding: 0,
                    resource: a_buf.as_entire_binding(),
                },
                wgpu::BindGroupEntry {
                    binding: 1,
                    resource: b_buf.as_entire_binding(),
                },
                wgpu::BindGroupEntry {
                    binding: 2,
                    resource: c_buf.as_entire_binding(),
                },
                wgpu::BindGroupEntry {
                    binding: 3,
                    resource: d_buf.as_entire_binding(),
                },
            ],
        });
        let mut enc = dev.create_command_encoder(&wgpu::CommandEncoderDescriptor { label: None });
        {
            let mut pass = enc.begin_compute_pass(&wgpu::ComputePassDescriptor {
                label: None,
                timestamp_writes: None,
            });
            pass.set_pipeline(&self.pipeline);
            pass.set_bind_group(0, &bg, &[]);
            pass.dispatch_workgroups(m.div_ceil(8), n.div_ceil(8), 1);
        }
        self.queue.submit(Some(enc.finish()));
        let staging = dev.create_buffer(&wgpu::BufferDescriptor {
            label: None,
            size: (out_len * 4) as u64,
            usage: wgpu::BufferUsages::COPY_DST | wgpu::BufferUsages::MAP_READ,
            mapped_at_creation: false,
        });
        let mut enc = dev.create_command_encoder(&wgpu::CommandEncoderDescriptor { label: None });
        enc.copy_buffer_to_buffer(&c_buf, 0, &staging, 0, (out_len * 4) as u64);
        self.queue.submit(Some(enc.finish()));
        let slice = staging.slice(..);
        let (tx, rx) = std::sync::mpsc::channel();
        slice.map_async(wgpu::MapMode::Read, move |r| {
            let _ = tx.send(r);
        });
        let _ = dev.poll(wgpu::PollType::wait_indefinitely());
        let _ = rx.recv();
        let Ok(data) = slice.get_mapped_range() else {
            return vec![f32::NAN; out_len];
        };
        let out: Vec<f32> = bytemuck::cast_slice(&data).to_vec();
        drop(data);
        staging.unmap();
        out
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::dp4a::{pack_for_kernel, reference_packed};

    /// **吞吐读数**：512³ 的整型 GEMM，5 批 × 20 次取中位数。
    #[test]
    fn dp4a_throughput() {
        let Some(g) = Dp4aGpu::new() else {
            println!("[dp4a] 无 GPU adapter ⇒ 跳过（不是通过）");
            return;
        };
        let (m, k, n) = (512usize, 512usize, 512usize);
        let a: Vec<i8> = (0..m * k).map(|i| [1i8, 0, -1][i % 3]).collect();
        let b: Vec<i8> = (0..k * n).map(|i| [1i8, -1, 0][i % 3]).collect();
        let (aw, btw, kw) = pack_for_kernel(&a, &b, m, k, n);
        // 预热（时钟爬坡会让第一个被测内核系统性偏慢）
        let _ = g.run(&aw, &btw, m as u32, n as u32, kw as u32);
        let mut samples = Vec::new();
        for _ in 0..5 {
            let t0 = std::time::Instant::now();
            for _ in 0..20 {
                let _ = g.run(&aw, &btw, m as u32, n as u32, kw as u32);
            }
            samples.push(t0.elapsed().as_secs_f64() / 20.0);
        }
        samples.sort_by(|a, b| a.partial_cmp(b).unwrap_or(std::cmp::Ordering::Equal));
        let med = samples[samples.len() / 2];
        let macs = (m * k * n) as f64;
        println!(
            "[dp4a] {m}×{k}×{n}: 中位 {:.3} ms/次，{:.1} GMAC/s（{:.3} ps/MAC）",
            med * 1e3,
            macs / med / 1e9,
            med / macs * 1e12
        );
        assert!(med > 0.0 && med.is_finite());
    }

    /// **GPU vs CPU 参考：必须逐元素相等**（整型累加，没有精度借口）。
    #[test]
    fn gpu_matches_cpu_reference() {
        let Some(g) = Dp4aGpu::new() else {
            println!("[dp4a] 无 GPU adapter ⇒ 跳过（不是通过）");
            return;
        };
        let (m, k, n) = (5usize, 13usize, 7usize);
        let a: Vec<i8> = (0..m * k).map(|i| [1i8, 0, -1][i % 3]).collect();
        let b: Vec<i8> = (0..k * n).map(|i| [1i8, -1, 0][i % 3]).collect();
        let (aw, btw, kw) = pack_for_kernel(&a, &b, m, k, n);
        let want = reference_packed(&aw, &btw, m, n, kw);
        let got = g.run(&aw, &btw, m as u32, n as u32, kw as u32);
        let got_i: Vec<i32> = got.iter().map(|v| *v as i32).collect();
        assert_eq!(got_i, want, "GPU 与 CPU 参考不一致");
    }
}

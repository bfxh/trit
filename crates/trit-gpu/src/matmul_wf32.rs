//! 三值权重 × f32 激活的 GPU 矩阵乘（WGSL）。
//!
//! 与 [`crate::matmul`] 的「三值 × 三值」不同：B 是 f32 激活，A 仍是双导轨三值权重。
//! 用途是一步生成器的 conv：im2col 后的补丁是 f32，权重是三值。

use crate::bench::{storage, storage_with, uniform};
use crate::matmul_wf32_plan::{Wf32Kernel, Wf32Plan};

pub const WORKGROUP: u32 = 64;

/// 复用 device / pipeline 的 WF32 执行器；每次 `run` 只建数据缓冲。
///
/// 同形状反复推理请改用 [`Wf32Plan`]：缓冲常驻显存，还能多层的合进一个 submit。
pub struct Wf32Gpu {
    pub(crate) device: wgpu::Device,
    pub(crate) queue: wgpu::Queue,
    pub(crate) pipeline: wgpu::ComputePipeline,
    pub(crate) pipeline_reg4: wgpu::ComputePipeline,
    pub(crate) pipeline_tiled: wgpu::ComputePipeline,
    pub(crate) layout: wgpu::BindGroupLayout,
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
    ///
    /// 下面的 `run*` 每次新建缓冲，适合一次性调用；同形状反复推理请建 [`Wf32Plan`]。
    pub fn plan(
        &self,
        m: usize,
        n: usize,
        k: usize,
        a_pos: &[u32],
        a_neg: &[u32],
        b: &[f32],
    ) -> Wf32Plan<'_> {
        Wf32Plan::new(self, m, n, k, a_pos, a_neg, b)
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
        self.plan(m, n, k, a_pos, a_neg, b)
            .run(Some(Wf32Kernel::Scalar))
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
        self.plan(m, n, k, a_pos, a_neg, b)
            .run(Some(Wf32Kernel::Reg4))
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
        self.plan(m, n, k, a_pos, a_neg, b)
            .run(Some(Wf32Kernel::Tiled))
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
        let kernel = Wf32Kernel::pick(m, n);
        self.plan(m, n, k, a_pos, a_neg, b).run(Some(kernel))
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

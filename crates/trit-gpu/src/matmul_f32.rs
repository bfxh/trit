//! f32 侧的可选变体：fp16 存储（`vec4<f16>` 装载、f32 累加）。
//!
//! 与 reg2-K8 同结构，只把 A/B 的全局与共享内存换成 `vec4<f16>`，
//! 用来单独量「f32 基线里有多少是带宽账」。输入来自三值矩阵（−1/0/1），
//! f16 可精确表示，所以本变体的误差为 0；这不代表任意 f32 输入下的精度。

use wgpu::util::DeviceExt;

use crate::bench::{median_ns_per, read_u32s, storage, storage_with, uniform};

/// 返回 `(ns_per_mac, 相对误差)`；adapter 不支持 `SHADER_F16` 时两者都是 NaN。
#[allow(clippy::too_many_arguments)]
pub(crate) fn run_f16(
    device: &wgpu::Device,
    queue: &wgpu::Queue,
    m: usize,
    n: usize,
    k: usize,
    fa: &[f32],
    fb: &[f32],
    expected: &[i64],
    usage: wgpu::BufferUsages,
) -> Option<(f64, f64)> {
    if !device.features().contains(wgpu::Features::SHADER_F16) {
        return Some((f64::NAN, f64::NAN));
    }
    let ha16: Vec<u16> = fa
        .iter()
        .map(|v| half::f16::from_f32(*v).to_bits())
        .collect();
    let hb16: Vec<u16> = fb
        .iter()
        .map(|v| half::f16::from_f32(*v).to_bits())
        .collect();
    let ha_buf = device.create_buffer_init(&wgpu::util::BufferInitDescriptor {
        label: Some("f16-a"),
        contents: bytemuck::cast_slice(&ha16),
        usage,
    });
    let hb_buf = device.create_buffer_init(&wgpu::util::BufferInitDescriptor {
        label: Some("f16-b"),
        contents: bytemuck::cast_slice(&hb16),
        usage,
    });
    let out = device.create_buffer(&wgpu::BufferDescriptor {
        label: Some("f16-out"),
        size: (m * n * 4) as u64,
        usage: usage | wgpu::BufferUsages::COPY_SRC,
        mapped_at_creation: false,
    });
    let shader = device.create_shader_module(wgpu::ShaderModuleDescriptor {
        label: Some("matmul-f32-f16"),
        // 独立文件才有 `enable f16;`：f32 基线共享的 matmul_f32.wgsl 不能带它，
        // 否则不支持 SHADER_F16 的 adapter 连 f32 着色器都建不出来。
        source: wgpu::ShaderSource::Wgsl(include_str!("matmul_f16.wgsl").into()),
    });
    let layout = device.create_bind_group_layout(&wgpu::BindGroupLayoutDescriptor {
        label: Some("f16"),
        entries: &[storage_with(2, false), uniform(3), storage(6), storage(7)],
    });
    let pipeline_layout = device.create_pipeline_layout(&wgpu::PipelineLayoutDescriptor {
        label: None,
        bind_group_layouts: &[Some(&layout)],
        immediate_size: 0,
    });
    let pipeline = device.create_compute_pipeline(&wgpu::ComputePipelineDescriptor {
        label: Some("matmul-f32-f16"),
        layout: Some(&pipeline_layout),
        module: &shader,
        entry_point: Some("main_tiled2k8_f16"),
        compilation_options: Default::default(),
        cache: None,
    });
    let params: [u32; 3] = [m as u32, n as u32, k as u32];
    let params_buf = device.create_buffer_init(&wgpu::util::BufferInitDescriptor {
        label: None,
        contents: bytemuck::cast_slice(&params),
        usage: wgpu::BufferUsages::UNIFORM,
    });
    let bind = device.create_bind_group(&wgpu::BindGroupDescriptor {
        label: None,
        layout: &layout,
        entries: &[
            wgpu::BindGroupEntry {
                binding: 2,
                resource: out.as_entire_binding(),
            },
            wgpu::BindGroupEntry {
                binding: 3,
                resource: params_buf.as_entire_binding(),
            },
            wgpu::BindGroupEntry {
                binding: 6,
                resource: ha_buf.as_entire_binding(),
            },
            wgpu::BindGroupEntry {
                binding: 7,
                resource: hb_buf.as_entire_binding(),
            },
        ],
    });
    let dispatch = || {
        let mut encoder =
            device.create_command_encoder(&wgpu::CommandEncoderDescriptor { label: None });
        {
            let mut pass = encoder.begin_compute_pass(&wgpu::ComputePassDescriptor::default());
            pass.set_pipeline(&pipeline);
            pass.set_bind_group(0, &bind, &[]);
            pass.dispatch_workgroups((n as u32).div_ceil(16), (m as u32).div_ceil(16), 1);
        }
        queue.submit([encoder.finish()]);
    };
    for _ in 0..20 {
        dispatch();
    }
    device
        .poll(wgpu::PollType::wait_indefinitely())
        .expect("poll");
    let ns = median_ns_per(device, m * n * k, dispatch);
    let bits = read_u32s(device, queue, &out, m * n)?;
    let err = bits
        .iter()
        .zip(expected.iter())
        .map(|(word, want)| {
            let got = f32::from_bits(*word);
            let scale = (*want as f32).abs().max(1.0) as f64;
            f64::from((got - *want as f32).abs()) / scale
        })
        .fold(0.0f64, f64::max);
    Some((ns, err))
}

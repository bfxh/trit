//! GPU 读取器（wgpu）：与 CPU 版同输入、同输出，只换执行位置。
//!
//! CPU 先算好 top-16、pair 行号、qb 位图、选项概念表（这些与读取器无关），
//! GPU 内核只做二阶/一阶读取与打分。这样对照的是「读取器」本身。

use std::time::Instant;

use bytemuck::{Pod, Zeroable};
use wgpu::util::DeviceExt;

use crate::model::{J, O_LEN, Q_LEN};
use crate::weights::Weights;

const PAIRS: usize = 256;
const NONE: u32 = u32::MAX;
const PRIME: i64 = 1_000_003;

#[repr(C)]
#[derive(Clone, Copy, Pod, Zeroable)]
struct Params {
    c: u32,
    r: u32,
    q_len: u32,
    o_len: u32,
    mix: f32,
    _pad: [u32; 3],
}

struct Prepared {
    pair_rows: Vec<u32>,
    top_idx: Vec<u32>,
    top_val: Vec<f32>,
    qb_ids: Vec<u32>,
    qb_vals: Vec<f32>,
    qb_mask: Vec<u32>,
    ob_ids: Vec<u32>,
    ob_w: Vec<f32>,
}

fn top16(qb: &[f32], c: usize) -> [(usize, f32); J] {
    let mut order: Vec<usize> = (1..c).collect();
    if c > J {
        order.select_nth_unstable_by(J - 1, |&x, &y| {
            qb[y].partial_cmp(&qb[x]).expect("有限值").then(x.cmp(&y))
        });
        order.truncate(J);
    }
    let mut out = [(0usize, 0f32); J];
    for (i, &c) in order.iter().enumerate() {
        out[i] = (c, qb[c]);
    }
    out
}

fn prepare(w: &Weights, q: &[i64], o: &[i64], n: usize) -> Prepared {
    let words = w.c.div_ceil(32);
    let mut pair_rows = vec![0u32; n * PAIRS];
    let mut top_idx = vec![0u32; n * J];
    let mut top_val = vec![0f32; n * J];
    let mut qb_mask = vec![0u32; n * words];
    let mut qb_ids = vec![NONE; n * Q_LEN];
    let mut qb_vals = vec![0f32; n * Q_LEN];
    let mut ob_ids = vec![NONE; n * 4 * O_LEN];
    let mut ob_w = vec![0f32; n * 4 * O_LEN];
    for i in 0..n {
        let mut qb = vec![0f32; w.c];
        for &id in &q[i * Q_LEN..(i + 1) * Q_LEN] {
            let c = w.concept[id as usize];
            if c > 0 {
                let c = c as usize;
                qb[c] = w.idf[c];
                qb_mask[i * words + c / 32] |= 1u32 << (c % 32);
            }
        }
        let mut used = 0usize;
        for (c, &v) in qb.iter().enumerate().skip(1) {
            if v != 0.0 {
                qb_ids[i * Q_LEN + used] = c as u32;
                qb_vals[i * Q_LEN + used] = v;
                used += 1;
            }
        }
        let top = top16(&qb, w.c);
        for d in 0..J {
            top_idx[i * J + d] = top[d].0 as u32;
            top_val[i * J + d] = top[d].1;
        }
        for a in 0..J {
            for b in (a + 1)..J {
                let (x, y) = (top[a].0, top[b].0);
                let (lo, hi) = if x < y { (x, y) } else { (y, x) };
                let row = ((lo as i64 * PRIME + hi as i64) % w.h as i64) as u32;
                pair_rows[i * PAIRS + a * J + b] = row;
            }
        }
        for k in 0..4 {
            let mut seen: Vec<usize> = Vec::new();
            for idx in 0..O_LEN {
                let c = w.concept[o[(i * 4 + k) * O_LEN + idx] as usize];
                if c > 0 {
                    let c = c as usize;
                    let in_q = (qb_mask[i * words + c / 32] >> (c % 32)) & 1 == 1;
                    if !in_q && !seen.contains(&c) {
                        seen.push(c);
                    }
                }
            }
            for (j, &c) in seen.iter().enumerate() {
                ob_ids[(i * 4 + k) * O_LEN + j] = c as u32;
                ob_w[(i * 4 + k) * O_LEN + j] = w.idf[c];
            }
        }
    }
    Prepared {
        pair_rows,
        top_idx,
        top_val,
        qb_ids,
        qb_vals,
        qb_mask,
        ob_ids,
        ob_w,
    }
}

fn storage_entry(binding: u32, read_only: bool) -> wgpu::BindGroupLayoutEntry {
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

struct Gpu {
    device: wgpu::Device,
    queue: wgpu::Queue,
    info: wgpu::AdapterInfo,
}

impl Gpu {
    fn open() -> Option<Self> {
        let instance = wgpu::Instance::new(wgpu::InstanceDescriptor::new_without_display_handle());
        let adapter =
            pollster::block_on(instance.request_adapter(&wgpu::RequestAdapterOptions::default()))
                .ok()?;
        let info = adapter.get_info();
        let limits = adapter.limits();
        let (device, queue) = pollster::block_on(adapter.request_device(&wgpu::DeviceDescriptor {
            required_limits: limits,
            ..Default::default()
        }))
        .ok()?;
        Some(Self {
            device,
            queue,
            info,
        })
    }
}

/// 跑 GPU 读取器；返回每题的四个 logits 与 GPU 推理毫秒（中位）。
pub fn run(w: &Weights, q: &[i64], o: &[i64], n: usize) -> Option<(Vec<f32>, f64)> {
    let t0 = Instant::now();
    let prep = prepare(w, q, o, n);
    println!(
        "CPU 预处理（top-16 / 位图 / 选项表）：{:.2} ms",
        t0.elapsed().as_secs_f64() * 1000.0
    );
    let gpu = Gpu::open()?;
    let device = &gpu.device;
    let queue = &gpu.queue;

    let params = Params {
        c: w.c as u32,
        r: w.r as u32,
        q_len: Q_LEN as u32,
        o_len: O_LEN as u32,
        mix: w.mix,
        _pad: [0; 3],
    };
    let row_ids32: Vec<i32> = w.row_ids.iter().map(|&v| v as i32).collect();
    let buffers = [
        device.create_buffer_init(&wgpu::util::BufferInitDescriptor {
            label: None,
            contents: bytemuck::cast_slice(&w.a),
            usage: wgpu::BufferUsages::STORAGE,
        }),
        device.create_buffer_init(&wgpu::util::BufferInitDescriptor {
            label: None,
            contents: bytemuck::cast_slice(&row_ids32),
            usage: wgpu::BufferUsages::STORAGE,
        }),
        device.create_buffer_init(&wgpu::util::BufferInitDescriptor {
            label: None,
            contents: bytemuck::cast_slice(&w.row_w),
            usage: wgpu::BufferUsages::STORAGE,
        }),
        device.create_buffer_init(&wgpu::util::BufferInitDescriptor {
            label: None,
            contents: bytemuck::cast_slice(&prep.pair_rows),
            usage: wgpu::BufferUsages::STORAGE,
        }),
        device.create_buffer_init(&wgpu::util::BufferInitDescriptor {
            label: None,
            contents: bytemuck::cast_slice(&prep.top_idx),
            usage: wgpu::BufferUsages::STORAGE,
        }),
        device.create_buffer_init(&wgpu::util::BufferInitDescriptor {
            label: None,
            contents: bytemuck::cast_slice(&prep.top_val),
            usage: wgpu::BufferUsages::STORAGE,
        }),
        device.create_buffer_init(&wgpu::util::BufferInitDescriptor {
            label: None,
            contents: bytemuck::cast_slice(&prep.qb_ids),
            usage: wgpu::BufferUsages::STORAGE,
        }),
        device.create_buffer_init(&wgpu::util::BufferInitDescriptor {
            label: None,
            contents: bytemuck::cast_slice(&prep.qb_vals),
            usage: wgpu::BufferUsages::STORAGE,
        }),
        device.create_buffer_init(&wgpu::util::BufferInitDescriptor {
            label: None,
            contents: bytemuck::cast_slice(&prep.qb_mask),
            usage: wgpu::BufferUsages::STORAGE,
        }),
        device.create_buffer_init(&wgpu::util::BufferInitDescriptor {
            label: None,
            contents: bytemuck::cast_slice(&prep.ob_ids),
            usage: wgpu::BufferUsages::STORAGE,
        }),
        device.create_buffer_init(&wgpu::util::BufferInitDescriptor {
            label: None,
            contents: bytemuck::cast_slice(&prep.ob_w),
            usage: wgpu::BufferUsages::STORAGE,
        }),
    ];
    let out_buf = device.create_buffer(&wgpu::BufferDescriptor {
        label: None,
        size: (n * 4 * 4) as u64,
        usage: wgpu::BufferUsages::STORAGE | wgpu::BufferUsages::COPY_SRC,
        mapped_at_creation: false,
    });
    let params_buf = device.create_buffer_init(&wgpu::util::BufferInitDescriptor {
        label: None,
        contents: bytemuck::bytes_of(&params),
        usage: wgpu::BufferUsages::UNIFORM,
    });

    let shader = device.create_shader_module(wgpu::ShaderModuleDescriptor {
        label: Some("wsp-reader"),
        source: wgpu::ShaderSource::Wgsl(include_str!("reader.wgsl").into()),
    });
    let mut entries: Vec<wgpu::BindGroupLayoutEntry> =
        (0..11).map(|b| storage_entry(b, true)).collect();
    entries.push(storage_entry(11, false));
    entries.push(wgpu::BindGroupLayoutEntry {
        binding: 12,
        visibility: wgpu::ShaderStages::COMPUTE,
        ty: wgpu::BindingType::Buffer {
            ty: wgpu::BufferBindingType::Uniform,
            has_dynamic_offset: false,
            min_binding_size: None,
        },
        count: None,
    });
    let layout = device.create_bind_group_layout(&wgpu::BindGroupLayoutDescriptor {
        label: None,
        entries: &entries,
    });
    let pipeline_layout = device.create_pipeline_layout(&wgpu::PipelineLayoutDescriptor {
        label: None,
        bind_group_layouts: &[Some(&layout)],
        immediate_size: 0,
    });
    let pipeline = device.create_compute_pipeline(&wgpu::ComputePipelineDescriptor {
        label: Some("wsp-reader"),
        layout: Some(&pipeline_layout),
        module: &shader,
        entry_point: Some("main"),
        compilation_options: Default::default(),
        cache: None,
    });
    let mut bind_entries: Vec<wgpu::BindGroupEntry> = buffers
        .iter()
        .enumerate()
        .map(|(i, b)| wgpu::BindGroupEntry {
            binding: i as u32,
            resource: b.as_entire_binding(),
        })
        .collect();
    bind_entries.push(wgpu::BindGroupEntry {
        binding: 11,
        resource: out_buf.as_entire_binding(),
    });
    bind_entries.push(wgpu::BindGroupEntry {
        binding: 12,
        resource: params_buf.as_entire_binding(),
    });
    let bind = device.create_bind_group(&wgpu::BindGroupDescriptor {
        label: None,
        layout: &layout,
        entries: &bind_entries,
    });

    let dispatch = || {
        let mut encoder =
            device.create_command_encoder(&wgpu::CommandEncoderDescriptor { label: None });
        {
            let mut pass = encoder.begin_compute_pass(&wgpu::ComputePassDescriptor::default());
            pass.set_pipeline(&pipeline);
            pass.set_bind_group(0, &bind, &[]);
            pass.dispatch_workgroups(n as u32, 1, 1);
        }
        queue.submit([encoder.finish()]);
    };

    for _ in 0..3 {
        dispatch();
    }
    device
        .poll(wgpu::PollType::wait_indefinitely())
        .expect("poll");

    let mut samples = Vec::with_capacity(5);
    for _ in 0..5 {
        let start = Instant::now();
        dispatch();
        device
            .poll(wgpu::PollType::wait_indefinitely())
            .expect("poll");
        samples.push(start.elapsed().as_secs_f64() * 1000.0);
    }
    samples.sort_by(|a, b| a.partial_cmp(b).expect("有限值"));
    println!("GPU 设备：{}｜后端：{:?}", gpu.info.name, gpu.info.backend);

    // 读回最后一次 dispatch 的结果。
    let size = (n * 4 * 4) as u64;
    let staging = device.create_buffer(&wgpu::BufferDescriptor {
        label: None,
        size,
        usage: wgpu::BufferUsages::MAP_READ | wgpu::BufferUsages::COPY_DST,
        mapped_at_creation: false,
    });
    let mut encoder =
        device.create_command_encoder(&wgpu::CommandEncoderDescriptor { label: None });
    encoder.copy_buffer_to_buffer(&out_buf, 0, &staging, 0, size);
    queue.submit([encoder.finish()]);
    let slice = staging.slice(..);
    slice.map_async(wgpu::MapMode::Read, |_| {});
    device
        .poll(wgpu::PollType::wait_indefinitely())
        .expect("poll");
    let mapped = slice.get_mapped_range().expect("map");
    let out = bytemuck::cast_slice::<u8, f32>(&mapped).to_vec();
    Some((out, samples[samples.len() / 2]))
}

/// 跑 GPU 读取器，与 CPU forward 逐题比较，打印时间与最大误差。
pub fn report(w: &Weights, q: &[i64], o: &[i64], y: &[i64], n: usize) {
    let Some((out, ms)) = run(w, q, o, n) else {
        eprintln!("没有可用的 GPU adapter");
        return;
    };
    let mut max_err = 0f32;
    let mut pred_diff = 0usize;
    let mut cpu_right = 0usize;
    let mut gpu_right = 0usize;
    let mut answered = 0usize;
    for i in 0..n {
        let cpu = crate::model::forward(
            w,
            &q[i * Q_LEN..(i + 1) * Q_LEN],
            &o[i * 4 * O_LEN..(i + 1) * 4 * O_LEN],
        );
        let gpu = &out[i * 4..i * 4 + 4];
        for k in 0..4 {
            max_err = max_err.max((cpu[k] - gpu[k]).abs());
        }
        if crate::model::argmax(&cpu) != crate::model::argmax(gpu) {
            pred_diff += 1;
        }
        if cpu.iter().cloned().fold(f32::NEG_INFINITY, f32::max)
            > cpu.iter().cloned().fold(f32::INFINITY, f32::min)
        {
            answered += 1;
            if crate::model::argmax(&cpu) == y[i] as usize {
                cpu_right += 1;
            }
            if crate::model::argmax(gpu) == y[i] as usize {
                gpu_right += 1;
            }
        }
    }
    let nf = n as f64;
    println!(
        "GPU 读取器：{ms:.2} ms（{n} 题）｜CPU↔GPU 最大绝对误差 {max_err:.6}｜预测不同 {pred_diff} 题"
    );
    println!(
        "GPU accuracy {:.6}%（answered {:.4}%）｜CPU accuracy {:.6}%",
        (gpu_right as f64 / nf + (n - answered) as f64 / nf / 4.0) * 100.0,
        answered as f64 / nf * 100.0,
        (cpu_right as f64 / nf + (n - answered) as f64 / nf / 4.0) * 100.0
    );
}

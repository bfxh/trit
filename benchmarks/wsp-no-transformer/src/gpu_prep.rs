//! GPU 预处理 pipeline：从 q/o 生成 top-16 / pair 行号 / 位图 / 选项表。

use wgpu::util::DeviceExt;

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

pub struct Prep {
    pub pipeline: wgpu::ComputePipeline,
    pub bind: wgpu::BindGroup,
}

/// 创建 prepare pipeline 与它要读的 q/o/concept/idf buffer。
pub fn make(
    device: &wgpu::Device,
    buffers: &[wgpu::Buffer; 11],
    params_buf: &wgpu::Buffer,
    q32: &[i32],
    o32: &[i32],
    concept32: &[i32],
    idf: &[f32],
) -> Prep {
    let q_buf = device.create_buffer_init(&wgpu::util::BufferInitDescriptor {
        label: None,
        contents: bytemuck::cast_slice(q32),
        usage: wgpu::BufferUsages::STORAGE,
    });
    let o_buf = device.create_buffer_init(&wgpu::util::BufferInitDescriptor {
        label: None,
        contents: bytemuck::cast_slice(o32),
        usage: wgpu::BufferUsages::STORAGE,
    });
    let concept_buf = device.create_buffer_init(&wgpu::util::BufferInitDescriptor {
        label: None,
        contents: bytemuck::cast_slice(concept32),
        usage: wgpu::BufferUsages::STORAGE,
    });
    let idf_buf = device.create_buffer_init(&wgpu::util::BufferInitDescriptor {
        label: None,
        contents: bytemuck::cast_slice(idf),
        usage: wgpu::BufferUsages::STORAGE,
    });
    let shader = device.create_shader_module(wgpu::ShaderModuleDescriptor {
        label: Some("wsp-prepare"),
        source: wgpu::ShaderSource::Wgsl(include_str!("prepare.wgsl").into()),
    });
    let mut entries: Vec<wgpu::BindGroupLayoutEntry> =
        (0..4).map(|b| storage_entry(b, true)).collect();
    entries.extend((4..10).map(|b| storage_entry(b, false)));
    entries.push(wgpu::BindGroupLayoutEntry {
        binding: 10,
        visibility: wgpu::ShaderStages::COMPUTE,
        ty: wgpu::BindingType::Buffer {
            ty: wgpu::BufferBindingType::Uniform,
            has_dynamic_offset: false,
            min_binding_size: None,
        },
        count: None,
    });
    entries.extend((11..=12).map(|b| storage_entry(b, false)));
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
        label: Some("wsp-prepare"),
        layout: Some(&pipeline_layout),
        module: &shader,
        entry_point: Some("main"),
        compilation_options: Default::default(),
        cache: None,
    });
    let bind = device.create_bind_group(&wgpu::BindGroupDescriptor {
        label: None,
        layout: &layout,
        entries: &[
            wgpu::BindGroupEntry {
                binding: 0,
                resource: q_buf.as_entire_binding(),
            },
            wgpu::BindGroupEntry {
                binding: 1,
                resource: o_buf.as_entire_binding(),
            },
            wgpu::BindGroupEntry {
                binding: 2,
                resource: concept_buf.as_entire_binding(),
            },
            wgpu::BindGroupEntry {
                binding: 3,
                resource: idf_buf.as_entire_binding(),
            },
            wgpu::BindGroupEntry {
                binding: 4,
                resource: buffers[4].as_entire_binding(),
            },
            wgpu::BindGroupEntry {
                binding: 5,
                resource: buffers[5].as_entire_binding(),
            },
            wgpu::BindGroupEntry {
                binding: 6,
                resource: buffers[3].as_entire_binding(),
            },
            wgpu::BindGroupEntry {
                binding: 7,
                resource: buffers[8].as_entire_binding(),
            },
            wgpu::BindGroupEntry {
                binding: 8,
                resource: buffers[9].as_entire_binding(),
            },
            wgpu::BindGroupEntry {
                binding: 9,
                resource: buffers[10].as_entire_binding(),
            },
            wgpu::BindGroupEntry {
                binding: 10,
                resource: params_buf.as_entire_binding(),
            },
            wgpu::BindGroupEntry {
                binding: 11,
                resource: buffers[6].as_entire_binding(),
            },
            wgpu::BindGroupEntry {
                binding: 12,
                resource: buffers[7].as_entire_binding(),
            },
        ],
    });
    Prep { pipeline, bind }
}

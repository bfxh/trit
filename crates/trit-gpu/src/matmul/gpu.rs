//! 打开 GPU：一个 adapter / device / queue，加固定的 buffer usage。

pub(crate) struct Gpu {
    pub device: wgpu::Device,
    pub queue: wgpu::Queue,
    pub info: wgpu::AdapterInfo,
    pub usage: wgpu::BufferUsages,
}

impl Gpu {
    pub async fn open() -> Option<Self> {
        let instance = wgpu::Instance::new(wgpu::InstanceDescriptor::new_without_display_handle());
        let adapter = instance
            .request_adapter(&wgpu::RequestAdapterOptions::default())
            .await
            .ok()?;
        let info = adapter.get_info();
        let limits = adapter.limits();
        // fp16 变体需要 SHADER_F16；adapter 不支持时只跑 f32 基线。
        let mut required_features = wgpu::Features::empty();
        if adapter.features().contains(wgpu::Features::SHADER_F16) {
            required_features |= wgpu::Features::SHADER_F16;
        }
        let (device, queue) = adapter
            .request_device(&wgpu::DeviceDescriptor {
                required_features,
                required_limits: limits,
                ..Default::default()
            })
            .await
            .ok()?;
        Some(Self {
            device,
            queue,
            info,
            usage: wgpu::BufferUsages::STORAGE,
        })
    }
}

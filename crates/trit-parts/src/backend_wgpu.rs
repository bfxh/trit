//! wgpu 后端的 `Backend` 适配器：把现有 `Gpu` + `Net` 包成统一接口
//! （见 `docs/MULTI-CHIP.md` §4.1）。

use crate::gpu::Gpu;
use crate::model::{Backend, Caps};
use crate::net::Net;
use crate::weights::Tensors;

pub struct WgpuBackend {
    gpu: Gpu,
    net: Net,
}

impl WgpuBackend {
    pub fn new(t: &Tensors) -> Result<Self, String> {
        let gpu = Gpu::new().ok_or("没有可用的 GPU 适配器")?;
        let mut net = Net::from_tensors(t)?;
        net.upload_all(&gpu);
        println!("[wgpu] 适配器：{}", gpu.adapter);
        Ok(Self { gpu, net })
    }

    pub fn adapter(&self) -> &str {
        &self.gpu.adapter
    }
}

impl Backend for WgpuBackend {
    fn caps(&self) -> Caps {
        Caps {
            name: "wgpu",
            max_buffer: self.gpu.device.limits().max_buffer_size,
            int8: false,
            lut: false,
        }
    }

    fn forward(&mut self, x: &[f32], res: u32) -> Result<Vec<f32>, String> {
        let hw = (res * res) as usize;
        let xb = self.gpu.buf(x, "x");
        let tr = self.net.forward(&self.gpu, xb, res);
        Ok(self.gpu.read(&tr.out, hw * 16))
    }
}

//! 把 trit-gpu 的 WGSL 源码编译成 SPIR-V（naga 前端 + SPIR-V 后端）。

/// 编译顺序：WGSL 解析 → 验证 → SPIR-V 输出。
pub fn to_spirv(wgsl: &str) -> Result<Vec<u32>, String> {
    let module = naga::front::wgsl::parse_str(wgsl).map_err(|e| format!("解析 WGSL 失败：{e}"))?;
    let mut validator = naga::valid::Validator::new(
        naga::valid::ValidationFlags::all(),
        naga::valid::Capabilities::all(),
    );
    let info = validator
        .validate(&module)
        .map_err(|e| format!("验证 WGSL 失败：{e}"))?;
    naga::back::spv::write_vec(&module, &info, &naga::back::spv::Options::default(), None)
        .map_err(|e| format!("生成 SPIR-V 失败：{e}"))
}

/// split-K 的 atomicAdd 需要输出先清零；wgpu 侧用 `clear_buffer`，
/// gpulib 没有专门的 clear API，所以用一个 64 线程的清零内核等价替代。
pub const CLEAR_WGSL: &str = r#"
@group(0) @binding(0) var<storage, read_write> out: array<u32>;

@compute @workgroup_size(64)
fn main(@builtin(global_invocation_id) gid: vec3<u32>) {
    if (gid.x < arrayLength(&out)) {
        out[gid.x] = 0u;
    }
}
"#;

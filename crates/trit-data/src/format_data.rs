//! 数据 / 二进制格式：GGUF、tritpack、npy、glb、ply、obj。

pub fn detect(text: &str) -> Vec<&'static str> {
    let lower = text.to_lowercase();
    let mut out = Vec::new();
    if lower.contains("gguf") || lower.contains("type_trit") || lower.contains("general.architecture")
    {
        out.push("gguf");
    }
    if lower.contains("tritpack") || lower.contains("trit-codec") || lower.contains("5 trits") {
        out.push("tritpack");
    }
    if lower.contains("numpy") {
        out.push("npy");
    }
    if lower.contains("gltf") || lower.contains(".glb") {
        out.push("glb");
    }
    if lower.contains("format ascii 1.0") || lower.contains("format binary_little_endian 1.0") {
        out.push("ply");
    }
    let lines: Vec<&str> = text.lines().collect();
    if lines.iter().any(|l| l.starts_with("v "))
        && lines.iter().any(|l| l.starts_with("f "))
    {
        out.push("obj");
    }
    out
}

use crate::mesh::Mesh;

use super::UvTris;

/// 解析 OBJ 文本为 `(Mesh, 三角角 UV)`。
///
/// 顶点号与 UV 号都是 **1-based**，而且 `f` 可以引用在文本更靠后定义的 `v` / `vt`，
/// 所以两件事必须分开做：
///
/// * 解析阶段只把 `0` 拒掉 —— `0 - 1` 会在 u32 上溢出（debug 直接 panic，release
///   绕成 4_294_967_295，下游按这个数索引一样炸）；
/// * 上界统一放到整份文本读完再校验 —— 边读边查会误拒"顶点后置"的合法文件。
pub fn parse_obj(text: &str) -> Result<(Mesh, UvTris), String> {
    let mut verts = Vec::new();
    let mut uvs = Vec::new();
    let mut tris = Vec::new();
    let mut pending_uv: Vec<[u32; 3]> = Vec::new();
    for line in text.lines() {
        let mut it = line.split_whitespace();
        match it.next() {
            Some("v") => {
                let q: Vec<f32> = it.filter_map(|s| s.parse().ok()).collect();
                if q.len() < 3 {
                    return Err("v 少于 3 个分量".into());
                }
                verts.push([q[0], q[1], q[2]]);
            }
            Some("vt") => {
                let q: Vec<f32> = it.filter_map(|s| s.parse().ok()).collect();
                if q.len() < 2 {
                    return Err("vt 少于 2 个分量".into());
                }
                uvs.push([q[0], q[1]]);
            }
            Some("f") => {
                let mut vi = [0u32; 3];
                let mut ti = [0u32; 3];
                for k in 0..3 {
                    let c = it.next().ok_or("f 少于 3 个角")?;
                    let mut parts = c.split('/');
                    let raw_v = parts
                        .next()
                        .and_then(|s| s.parse::<u32>().ok())
                        .ok_or("f 顶点号坏")?;
                    if raw_v == 0 {
                        return Err("f 顶点号从 1 开始，0 非法".into());
                    }
                    vi[k] = raw_v - 1;
                    // 省略 `/uv` 时沿用顶点号：用 `raw_v` 而不是 `vi[k] + 1`，
                    // 后者在 raw_v 极大时会溢出。
                    let raw_t = parts
                        .next()
                        .and_then(|s| s.parse::<u32>().ok())
                        .unwrap_or(raw_v);
                    if raw_t == 0 {
                        return Err("f UV 号从 1 开始，0 非法".into());
                    }
                    ti[k] = raw_t - 1;
                }
                tris.push(vi);
                pending_uv.push(ti);
            }
            _ => {}
        }
    }
    if tris.is_empty() || verts.is_empty() {
        return Err("OBJ 空".into());
    }
    // 上界校验：`uvs[...]` 以前是裸索引，越界就把进程带走。
    for (tri, ti) in tris.iter().zip(pending_uv.iter()) {
        for k in 0..3 {
            if tri[k] as usize >= verts.len() {
                return Err(format!("f 引用了不存在的顶点 {}", tri[k] + 1));
            }
            if ti[k] as usize >= uvs.len() {
                return Err(format!("f 引用了不存在的 UV {}", ti[k] + 1));
            }
        }
    }
    let tri_uv: UvTris = pending_uv
        .iter()
        .map(|ti| {
            [
                uvs[ti[0] as usize],
                uvs[ti[1] as usize],
                uvs[ti[2] as usize],
            ]
        })
        .collect();
    Ok((Mesh { verts, tris }, tri_uv))
}

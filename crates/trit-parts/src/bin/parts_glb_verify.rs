//! `parts-glb-verify`：GLB 资产（A2）的**独立复核**。
//!
//! 重算三个输入清单的 SHA，按同一套 OBJ/PNG/布局**重新打包**一遍，与盘上的 GLB 逐字节比对；
//! 同时检查 GLB 头/分块与对象数、PNG 数。不一致就写 `.verify.reject.json` 并退出码 1。
//!
//! ```text
//! parts-glb-verify --glb runs/scene-full/scene0084/objects-mesh/scene.glb
//! ```

use std::path::{Path, PathBuf};

use trit_parts::glb::{build_with, load_scene};
use trit_parts::partsrun::{created_unix, git_out, json_str};

fn arg(argv: &[String], k: &str) -> Option<String> {
    argv.iter()
        .position(|a| a == k)
        .and_then(|i| argv.get(i + 1).cloned())
}

fn usage() -> ! {
    eprintln!("用法: parts-glb-verify --glb <路径> [--json-out <path>]");
    std::process::exit(2);
}

fn field(txt: &str, key: &str) -> Option<String> {
    let pat = format!("\"{key}\": \"");
    let i = txt.find(&pat)? + pat.len();
    let rest = &txt[i..];
    let end = rest.find('"')?;
    Some(rest[..end].to_string())
}

fn number(txt: &str, key: &str) -> Option<f64> {
    let pat = format!("\"{key}\": ");
    let i = txt.find(&pat)? + pat.len();
    let rest = &txt[i..];
    let end = rest
        .find(|c: char| !(c.is_ascii_digit() || c == '.' || c == '-' || c == 'e' || c == '+'))
        .unwrap_or(rest.len());
    rest[..end].parse().ok()
}

fn block<'a>(txt: &'a str, key: &str) -> Option<&'a str> {
    let pat = format!("\"{key}\": {{");
    let i = txt.find(&pat)? + pat.len();
    let rest = &txt[i..];
    let mut depth = 1usize;
    let (mut in_str, mut esc) = (false, false);
    for (j, ch) in rest.char_indices() {
        if in_str {
            if esc {
                esc = false;
            } else if ch == '\\' {
                esc = true;
            } else if ch == '"' {
                in_str = false;
            }
        } else if ch == '"' {
            in_str = true;
        } else if ch == '{' {
            depth += 1;
        } else if ch == '}' {
            depth -= 1;
            if depth == 0 {
                return Some(&rest[..j]);
            }
        }
    }
    None
}

fn resolve(base: &Path, p: String) -> PathBuf {
    let p = PathBuf::from(p);
    if p.is_absolute() { p } else { base.join(p) }
}

fn sha(p: &Path) -> String {
    trit_core::fingerprint::file_sha256(p).unwrap_or_default()
}

fn read(p: &Path) -> String {
    std::fs::read_to_string(p).unwrap_or_else(|e| {
        eprintln!("读不了 {}: {e}", p.display());
        std::process::exit(2);
    })
}

/// GLB 结构：magic/版本/总长/两个分块；返回 (JSON 文本, BIN 切片)。
fn chunks(b: &[u8]) -> Result<(String, &[u8]), String> {
    if b.len() < 20 || &b[0..4] != b"glTF" {
        return Err("不是 GLB".into());
    }
    let ver = u32::from_le_bytes([b[4], b[5], b[6], b[7]]);
    let total = u32::from_le_bytes([b[8], b[9], b[10], b[11]]) as usize;
    if ver != 2 || total != b.len() {
        return Err(format!("版本/长度不对：v{ver} / {total} vs {}", b.len()));
    }
    let jl = u32::from_le_bytes([b[12], b[13], b[14], b[15]]) as usize;
    if &b[16..20] != b"JSON" || 20 + jl + 8 > b.len() {
        return Err("JSON 分块坏".into());
    }
    let json = String::from_utf8_lossy(&b[20..20 + jl]).to_string();
    let bo = 20 + jl;
    let bl = u32::from_le_bytes([b[bo], b[bo + 1], b[bo + 2], b[bo + 3]]) as usize;
    if &b[bo + 4..bo + 8] != b"BIN\0" || bo + 8 + bl != b.len() {
        return Err("BIN 分块坏".into());
    }
    Ok((json, &b[bo + 8..]))
}

fn png_count(bin: &[u8]) -> usize {
    bin.windows(4)
        .filter(|w| *w == [0x89, b'P', b'N', b'G'])
        .count()
}

fn main() {
    let argv: Vec<String> = std::env::args().skip(1).collect();
    let glb = PathBuf::from(arg(&argv, "--glb").unwrap_or_else(|| usage()));
    let mut mp = glb.as_os_str().to_owned();
    mp.push(".manifest.json");
    let mpath = PathBuf::from(mp);
    let base = glb
        .parent()
        .map(|p| p.to_path_buf())
        .unwrap_or_else(|| PathBuf::from("."));
    let manifest = read(&mpath);
    let cfg = block(&manifest, "config").unwrap_or_else(|| usage());
    let inputs = block(&manifest, "inputs").unwrap_or_else(|| usage());
    let gb = block(&manifest, "glb").unwrap_or_else(|| usage());
    let generator = field(cfg, "generator").unwrap_or_else(|| "trit-parts".into());
    let spin = number(cfg, "spin").unwrap_or(0.0) as f32;
    let mesh_in = block(inputs, "objects_mesh").unwrap_or_else(|| usage());
    let obj_in = block(inputs, "objects_manifest").unwrap_or_else(|| usage());
    let lay_in = block(inputs, "layout").unwrap_or_else(|| usage());
    let mesh_p = resolve(&base, field(mesh_in, "path").unwrap_or_else(|| usage()));
    let obj_p = resolve(&base, field(obj_in, "path").unwrap_or_else(|| usage()));
    let lay_p = resolve(&base, field(lay_in, "path").unwrap_or_else(|| usage()));
    let ok_inputs = field(mesh_in, "sha256")
        .map(|s| s == sha(&mesh_p))
        .unwrap_or(false)
        && field(obj_in, "sha256")
            .map(|s| s == sha(&obj_p))
            .unwrap_or(false)
        && field(lay_in, "sha256")
            .map(|s| s == sha(&lay_p))
            .unwrap_or(false);

    let (want_parts, want_verts, want_tris) = (
        number(gb, "parts").unwrap_or(-1.0) as usize,
        number(gb, "verts").unwrap_or(-1.0) as usize,
        number(gb, "tris").unwrap_or(-1.0) as usize,
    );
    let mesh_dir = mesh_p.parent().unwrap_or_else(|| usage());
    let loaded = load_scene(mesh_dir, Some(&lay_p)).unwrap_or_else(|e| fail(&e));
    let ok_paths = loaded.objects_manifest == obj_p || sha(&loaded.objects_manifest) == sha(&obj_p);
    let rebuilt = build_with(
        &loaded.parts,
        &generator,
        if spin > 0.0 { Some(spin) } else { None },
    );
    let delivered = std::fs::read(&glb).unwrap_or_default();
    let ok_bytes = delivered == rebuilt
        && field(gb, "sha256").map(|s| s == sha(&glb)).unwrap_or(false)
        && number(gb, "bytes") == Some(delivered.len() as f64);
    let ok_stats =
        want_parts == loaded.parts.len() && want_verts == loaded.verts && want_tris == loaded.tris;
    let (ok_struct, detail) = match chunks(&delivered) {
        Ok((json, bin)) => {
            let nodes = json.matches("\"mesh\": ").count();
            let pngs = png_count(bin);
            let anims = json.matches("\"animations\": [").count();
            let want_anim = usize::from(spin > 0.0);
            let ok_anim = anims == want_anim
                && (spin <= 0.0
                    || (json.contains("\"name\": \"turntable\"")
                        && json.contains("\"path\": \"rotation\"")));
            (
                json.contains("\"meshes\": [")
                    && nodes == loaded.parts.len()
                    && pngs == loaded.parts.len()
                    && ok_anim,
                format!("节点 {nodes}、PNG {pngs}、动画 {anims}"),
            )
        }
        Err(e) => (false, e),
    };

    let pass = ok_inputs && ok_paths && ok_bytes && ok_stats && ok_struct;
    println!(
        "复核 {}：输入 {}｜对象 {}/{}｜顶点 {}/{}｜字节 {}｜结构（{}）",
        mpath.display(),
        if ok_inputs && ok_paths {
            "一致"
        } else {
            "不一致"
        },
        loaded.parts.len(),
        want_parts,
        loaded.verts,
        want_verts,
        if ok_bytes {
            "逐字节一致"
        } else {
            "不一致"
        },
        detail
    );
    println!(
        "{}",
        if pass {
            "复核 PASS：输入清单 / 逐字节重建 / 读回结构全一致"
        } else {
            "复核 FAIL：与清单或重建结果不一致"
        }
    );

    let body = format!(
        "{{\n  \"schema\": 1,\n  \"kind\": \"extract-glb-verify\",\n  \"created_unix\": {},\n  \"commit\": {},\n  \"glb\": {{\"path\": {}, \"sha256\": \"{}\", \"bytes\": {}}},\n  \"inputs_ok\": {ok_inputs},\n  \"parts\": {{\"got\": {}, \"want\": {want_parts}, \"ok\": {ok_stats}}},\n  \"bytes_ok\": {ok_bytes},\n  \"struct_ok\": {ok_struct},\n  \"spin\": {spin},\n  \"verdict\": \"{}\"\n}}\n",
        created_unix(),
        json_str(git_out(&["rev-parse", "--short", "HEAD"]).trim()),
        json_str(&trit_parts::scenefp::norm_root(&glb)),
        sha(&glb),
        delivered.len(),
        loaded.parts.len(),
        if pass { "pass" } else { "reject" }
    );
    let custom = arg(&argv, "--json-out").map(PathBuf::from);
    let vp = custom.clone().unwrap_or_else(|| {
        let mut s = glb.as_os_str().to_owned();
        s.push(if pass {
            ".verify.json"
        } else {
            ".verify.reject.json"
        });
        PathBuf::from(s)
    });
    if custom.is_none() {
        let mut s = glb.as_os_str().to_owned();
        s.push(if pass {
            ".verify.reject.json"
        } else {
            ".verify.json"
        });
        let _ = std::fs::remove_file(PathBuf::from(s));
    }
    std::fs::write(&vp, body).unwrap_or_else(|e| {
        eprintln!("写复核记录 {}: {e}", vp.display());
        std::process::exit(2);
    });
    println!(
        "复核记录 {}（{}）",
        vp.display(),
        if pass { "pass" } else { "reject" }
    );
    if !pass {
        std::process::exit(1);
    }
}

fn fail(msg: &str) -> ! {
    eprintln!("FAIL {msg}");
    std::process::exit(2);
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn chunks_and_structure_checks() {
        let part = trit_parts::glb::Part {
            name: "obj-1".into(),
            mesh: trit_parts::mesh::Mesh {
                verts: vec![[0.0, 0.0, 0.0], [1.0, 0.0, 0.0], [0.0, 1.0, 0.0]],
                tris: vec![[0, 1, 2]],
            },
            uv: vec![[[0.0, 0.0], [1.0, 0.0], [0.0, 1.0]]],
            png: vec![0x89, b'P', b'N', b'G'],
            translation: [0.0; 3],
            rotation: [0.0, 0.0, 0.0, 1.0],
        };
        let b = trit_parts::glb::build(&[part], "t");
        let (json, bin) = chunks(&b).unwrap();
        assert!(json.contains("\"meshes\": ["));
        assert_eq!(json.matches("\"mesh\": ").count(), 1);
        assert_eq!(png_count(bin), 1);
        assert!(chunks(&b[..10]).is_err());
        let mut bad = b.clone();
        bad[8] ^= 1; // 总长与文件长度不符
        assert!(chunks(&bad).is_err());
    }

    #[test]
    fn manifest_fields_parse() {
        let man = "{\"config\": {\"generator\": \"trit-parts\"}, \"inputs\": {\"objects_mesh\": {\"path\": \"a.json\", \"sha256\": \"m\"}, \"objects_manifest\": {\"path\": \"../o.json\", \"sha256\": \"o\"}, \"layout\": {\"path\": \"../l.json\", \"sha256\": \"l\"}}, \"glb\": {\"path\": \"s.glb\", \"sha256\": \"g\", \"bytes\": 10, \"parts\": 7, \"verts\": 100, \"tris\": 50}}";
        let g = block(man, "glb").unwrap();
        assert_eq!(
            (number(g, "parts"), number(g, "verts")),
            (Some(7.0), Some(100.0))
        );
        let inp = block(man, "inputs").unwrap();
        assert_eq!(
            field(block(inp, "layout").unwrap(), "sha256").as_deref(),
            Some("l")
        );
        assert_eq!(
            field(block(man, "config").unwrap(), "generator").as_deref(),
            Some("trit-parts")
        );
    }
}

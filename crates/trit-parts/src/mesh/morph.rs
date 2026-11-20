use super::build::Mesh;

pub(super) fn clamp_radius(r: usize, nx: usize, ny: usize, nz: usize) -> usize {
    r.min(nx.min(ny).min(nz))
}
fn ball_offsets(r: usize) -> Vec<(isize, isize, isize)> {
    let r = r as isize;
    let mut out = Vec::new();
    for dz in -r..=r {
        for dy in -r..=r {
            for dx in -r..=r {
                if dx * dx + dy * dy + dz * dz <= r * r {
                    out.push((dx, dy, dz));
                }
            }
        }
    }
    out
}

pub fn erode(occ: &[u8], nx: usize, ny: usize, nz: usize, r: usize) -> Vec<u8> {
    let r = clamp_radius(r, nx, ny, nz);
    if r == 0 {
        return occ.to_vec();
    }
    let offs = ball_offsets(r);
    let mut out = vec![0u8; occ.len()];
    for z in 0..nz {
        for y in 0..ny {
            for x in 0..nx {
                let i = x + nx * (y + ny * z);
                if occ[i] == 0 {
                    continue;
                }
                let mut keep = true;
                for &(dx, dy, dz) in &offs {
                    let (xx, yy, zz) = (x as isize + dx, y as isize + dy, z as isize + dz);
                    if xx < 0
                        || yy < 0
                        || zz < 0
                        || xx >= nx as isize
                        || yy >= ny as isize
                        || zz >= nz as isize
                    {
                        keep = false;
                        break;
                    }
                    if occ[xx as usize + nx * (yy as usize + ny * zz as usize)] == 0 {
                        keep = false;
                        break;
                    }
                }
                out[i] = keep as u8;
            }
        }
    }
    out
}

/// 膨胀：球核覆盖到的任一位置为 1 就置 1。
pub fn dilate(occ: &[u8], nx: usize, ny: usize, nz: usize, r: usize) -> Vec<u8> {
    let r = clamp_radius(r, nx, ny, nz);
    if r == 0 {
        return occ.to_vec();
    }
    let offs = ball_offsets(r);
    let mut out = vec![0u8; occ.len()];
    for z in 0..nz {
        for y in 0..ny {
            for x in 0..nx {
                let mut on = false;
                for &(dx, dy, dz) in &offs {
                    let (xx, yy, zz) = (x as isize + dx, y as isize + dy, z as isize + dz);
                    if xx < 0
                        || yy < 0
                        || zz < 0
                        || xx >= nx as isize
                        || yy >= ny as isize
                        || zz >= nz as isize
                    {
                        continue;
                    }
                    if occ[xx as usize + nx * (yy as usize + ny * zz as usize)] != 0 {
                        on = true;
                        break;
                    }
                }
                out[x + nx * (y + ny * z)] = on as u8;
            }
        }
    }
    out
}

pub fn open_solid(occ: &[u8], nx: usize, ny: usize, nz: usize, r: usize) -> Vec<u8> {
    let e = erode(occ, nx, ny, nz, r);
    dilate(&e, nx, ny, nz, r)
}

pub fn multiface_edges(m: &Mesh) -> usize {
    use std::collections::HashMap;
    let mut cnt: HashMap<(u32, u32), usize> = HashMap::new();
    for t in &m.tris {
        for (a, b) in [(t[0], t[1]), (t[1], t[2]), (t[2], t[0])] {
            let e = if a < b { (a, b) } else { (b, a) };
            *cnt.entry(e).or_insert(0) += 1;
        }
    }
    cnt.values().filter(|&&c| c > 2).count()
}

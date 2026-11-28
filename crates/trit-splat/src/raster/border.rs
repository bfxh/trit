/// `--bg auto` 用：GT 帧边界 4px 环带的均值作背景色（对拍用，注明口径）。
pub fn border_mean(rgb: &[u8], width: u32, height: u32, band: u32) -> [f32; 3] {
    let w = width as i32;
    let h = height as i32;
    let band = band as i32;
    let (mut r, mut g, mut b, mut n) = (0.0f64, 0.0f64, 0.0f64, 0u64);
    for y in 0..h {
        for x in 0..w {
            let edge = x < band || x >= w - band || y < band || y >= h - band;
            if !edge {
                continue;
            }
            let o = ((y as u32) * width + x as u32) as usize * 3;
            r += rgb[o] as f64;
            g += rgb[o + 1] as f64;
            b += rgb[o + 2] as f64;
            n += 1;
        }
    }
    if n == 0 {
        return [0.4, 0.4, 0.4];
    }
    let n = n as f64;
    [
        (r / n) as f32 / 255.0,
        (g / n) as f32 / 255.0,
        (b / n) as f32 / 255.0,
    ]
}

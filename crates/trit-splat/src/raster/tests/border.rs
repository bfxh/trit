use crate::raster::border_mean;

#[test]
fn border_mean_of_uniform_image_is_its_color() {
    let w = 8u32;
    let h = 8u32;
    let mut rgb = vec![0u8; (w * h * 3) as usize];
    for px in rgb.as_chunks_mut::<3>().0 {
        px.copy_from_slice(&[200, 100, 50]);
    }
    let mean = border_mean(&rgb, w, h, 2);
    assert!((mean[0] - 200.0 / 255.0).abs() < 1e-4);
    assert!((mean[2] - 50.0 / 255.0).abs() < 1e-4);
}

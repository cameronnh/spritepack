use image::{imageops::FilterType, DynamicImage, RgbImage};

const SIM_SIZE: u32 = 128;
const SIM_THRESHOLD: f64 = 0.85;

/// Match SpriteMaker.py: resize to 128×128 RGB, then similarity = 1 - mean(abs diff) / 255.
pub fn image_similarity(a: &RgbImage, b: &RgbImage) -> f64 {
    debug_assert_eq!(a.dimensions(), (SIM_SIZE, SIM_SIZE));
    debug_assert_eq!(b.dimensions(), (SIM_SIZE, SIM_SIZE));
    let mut sum: u64 = 0;
    for (pa, pb) in a.pixels().zip(b.pixels()) {
        for c in 0..3 {
            sum += pa.0[c].abs_diff(pb.0[c]) as u64;
        }
    }
    let denom = (SIM_SIZE as u64) * (SIM_SIZE as u64) * 255 * 3;
    1.0 - (sum as f64 / denom as f64)
}

pub fn to_similarity_rgb(img: &DynamicImage) -> RgbImage {
    let rgb = img.to_rgb8();
    image::imageops::resize(&rgb, SIM_SIZE, SIM_SIZE, FilterType::Triangle)
}

/// Greedy clustering like `sort_similar_images` in SpriteMaker.py.
/// Returns `(path, image)` pairs in the order they should appear on the sheet.
pub fn sort_similar_items(
    items: Vec<(std::path::PathBuf, DynamicImage)>,
) -> Vec<(std::path::PathBuf, DynamicImage)> {
    let mut images: Vec<_> = items
        .into_iter()
        .map(|(p, img)| {
            let rgb = to_similarity_rgb(&img);
            (p, img, rgb)
        })
        .collect();

    let mut ordered = Vec::new();

    while !images.is_empty() {
        let (anchor_path, anchor_img, anchor_rgb) = images.remove(0);
        let mut cluster = vec![(anchor_path, anchor_img)];

        let mut i = 0;
        while i < images.len() {
            if image_similarity(&anchor_rgb, &images[i].2) > SIM_THRESHOLD {
                let (p, img, _rgb) = images.remove(i);
                cluster.push((p, img));
            } else {
                i += 1;
            }
        }

        ordered.extend(cluster);
    }

    ordered
}

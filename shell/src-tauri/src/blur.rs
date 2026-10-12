//! 6m-a "vibrancy": kompact can't blur what's behind a surface, so the shell blurs the wallpaper
//! image itself, once, at a small size; the menu bar and Launchpad show that image (scaled up,
//! placed exactly like the wallpaper) under a tint. Pure and tested; `main.rs` does the decoding.

/// Blur an RGBA image (4 bytes per pixel, `stride` bytes per row) in place: three box-blur passes
/// per direction, close to a Gaussian with sigma ≈ `radius`. Edges are clamped.
pub fn blur_rgba(pixels: &mut [u8], width: usize, height: usize, stride: usize, radius: usize) {
    if width == 0 || height == 0 || radius == 0 {
        return;
    }
    let mut line = Vec::new();
    for _ in 0..3 {
        for y in 0..height {
            let row = y * stride;
            box_pass(pixels, row, 4, width, radius, &mut line);
        }
        for x in 0..width {
            box_pass(pixels, x * 4, stride, height, radius, &mut line);
        }
    }
}

/// One box blur along a line of `len` pixels starting at byte `start`, `step` bytes apart.
fn box_pass(pixels: &mut [u8], start: usize, step: usize, len: usize, radius: usize, line: &mut Vec<[u8; 4]>) {
    line.clear();
    line.extend((0..len).map(|i| {
        let p = start + i * step;
        [pixels[p], pixels[p + 1], pixels[p + 2], pixels[p + 3]]
    }));
    let at = |i: isize| line[i.clamp(0, len as isize - 1) as usize];
    let window = (2 * radius + 1) as u32;
    let mut sum = [0u32; 4];
    for i in -(radius as isize)..=radius as isize {
        let px = at(i);
        for c in 0..4 {
            sum[c] += px[c] as u32;
        }
    }
    for i in 0..len {
        let p = start + i * step;
        for c in 0..4 {
            pixels[p + c] = ((sum[c] + window / 2) / window) as u8;
        }
        let (add, sub) = (at(i as isize + radius as isize + 1), at(i as isize - radius as isize));
        for c in 0..4 {
            sum[c] = sum[c] + add[c] as u32 - sub[c] as u32;
        }
    }
}

/// Size to decode the wallpaper at before blurring: the longer side `max_side`, aspect kept.
pub fn small_size(width: i32, height: i32, max_side: i32) -> (i32, i32) {
    if width <= 0 || height <= 0 {
        return (1, 1);
    }
    let scale = (max_side as f64 / width.max(height) as f64).min(1.0);
    (((width as f64 * scale).round() as i32).max(1), ((height as f64 * scale).round() as i32).max(1))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn flat_image_stays_flat() {
        let mut px = [10u8, 20, 30, 255].repeat(7 * 5);
        blur_rgba(&mut px, 7, 5, 7 * 4, 2);
        assert!(px.chunks(4).all(|p| p == [10, 20, 30, 255]));
    }

    #[test]
    fn a_bright_dot_spreads_out() {
        let (w, h) = (21, 21);
        let mut px = vec![0u8; w * h * 4];
        let centre = (10 * w + 10) * 4;
        px[centre..centre + 4].copy_from_slice(&[255, 255, 255, 255]);
        blur_rgba(&mut px, w, h, w * 4, 2);
        let at = |x: usize, y: usize| px[(y * w + x) * 4];
        assert!(at(10, 10) < 255 && at(10, 10) > 0);
        assert!(at(12, 10) > 0 && at(10, 12) > 0, "spreads both ways");
        assert!(at(10, 10) >= at(12, 10) && at(12, 10) >= at(15, 10), "falls off from the centre");
        assert_eq!(at(0, 0), 0, "far corner untouched");
    }

    #[test]
    fn stride_padding_is_left_alone() {
        // 2x2 image, rows padded to 12 bytes.
        let mut px = vec![9u8; 24];
        for y in 0..2 {
            px[y * 12 + 8..y * 12 + 12].copy_from_slice(&[1, 2, 3, 4]);
        }
        blur_rgba(&mut px, 2, 2, 12, 1);
        assert_eq!(&px[8..12], &[1, 2, 3, 4]);
        assert_eq!(&px[20..24], &[1, 2, 3, 4]);
    }

    #[test]
    fn small_sizes() {
        assert_eq!(small_size(3840, 2160, 480), (480, 270));
        assert_eq!(small_size(1000, 2000, 480), (240, 480));
        assert_eq!(small_size(100, 50, 480), (100, 50), "never scaled up");
        assert_eq!(small_size(0, 50, 480), (1, 1));
    }
}

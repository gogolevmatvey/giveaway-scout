use tauri::image::Image;

/// Draw the unread badge in memory; keep the original app icon unchanged.
pub fn tray_image(icon: &Image<'_>, has_unread: bool) -> Image<'static> {
    if !has_unread || icon.width() == 0 || icon.height() == 0 {
        return icon.clone().to_owned();
    }

    let width = icon.width();
    let height = icon.height();
    let radius = f64::from(width.min(height)) * 0.23;
    let center_x = f64::from(width) - radius - 1.0;
    let center_y = f64::from(height) - radius - 1.0;
    let inner_radius = radius * 0.72;
    let mut rgba = icon.rgba().to_vec();
    for (index, pixel) in rgba.as_chunks_mut::<4>().0.iter_mut().enumerate() {
        let x = (index % width as usize) as f64 + 0.5;
        let y = (index / width as usize) as f64 + 0.5;
        let distance_squared = (x - center_x).powi(2) + (y - center_y).powi(2);
        if distance_squared <= inner_radius.powi(2) {
            *pixel = [234, 76, 96, 255];
        } else if distance_squared <= radius.powi(2) {
            *pixel = [255, 255, 255, 255];
        }
    }
    Image::new_owned(rgba, width, height)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn unread_attention_changes_icon_and_read_state_restores_original() {
        for (width, height) in [(16, 16), (32, 32), (64, 32)] {
            let pixels = [20, 30, 40, 255].repeat(width as usize * height as usize);
            let original = Image::new_owned(pixels.clone(), width, height);
            let unread = tray_image(&original, true);
            assert_ne!(unread.rgba(), pixels);
            assert_eq!((unread.width(), unread.height()), (width, height));
            assert_eq!(&unread.rgba()[..4], &pixels[..4]);
            assert_eq!(original.rgba(), pixels);
            assert_eq!(tray_image(&original, false).rgba(), pixels);
        }
    }

    #[test]
    fn empty_icon_does_not_panic_or_gain_pixels() {
        let icon = Image::new_owned(Vec::new(), 0, 0);
        assert!(tray_image(&icon, true).rgba().is_empty());
    }
}

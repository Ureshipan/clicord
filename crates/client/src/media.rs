//! Cross-platform media helpers: MIME guessing, inline terminal thumbnails for
//! images, and opening a downloaded file with the OS's default application.
//!
//! Image previews use Unicode half-block cells (`▀`): each character shows two
//! stacked pixels — the upper via the foreground colour, the lower via the
//! background. That only needs a truecolour terminal, so it renders the same on
//! Windows Terminal, macOS and Linux without sixel/kitty-specific protocols.

use std::path::Path;

/// One character cell of a thumbnail: two vertically-stacked pixels.
#[derive(Clone, Copy)]
pub struct Cell {
    pub top: (u8, u8, u8),
    pub bottom: (u8, u8, u8),
}

/// A decoded, downscaled image ready to draw as rows of half-block cells.
#[derive(Clone)]
pub struct ImageArt {
    pub rows: Vec<Vec<Cell>>,
}

/// Guess a MIME type from a file's extension. Falls back to a generic binary
/// type so unknown files still upload fine.
pub fn guess_mime(path: &Path) -> String {
    let ext = path
        .extension()
        .and_then(|e| e.to_str())
        .map(|e| e.to_ascii_lowercase())
        .unwrap_or_default();
    let m = match ext.as_str() {
        "png" => "image/png",
        "jpg" | "jpeg" => "image/jpeg",
        "gif" => "image/gif",
        "webp" => "image/webp",
        "bmp" => "image/bmp",
        "svg" => "image/svg+xml",
        "pdf" => "application/pdf",
        "txt" | "log" | "md" => "text/plain",
        "json" => "application/json",
        "zip" => "application/zip",
        "gz" | "tgz" => "application/gzip",
        "mp3" => "audio/mpeg",
        "mp4" => "video/mp4",
        "wav" => "audio/wav",
        _ => "application/octet-stream",
    };
    m.to_string()
}

/// Decode `bytes` and downscale to fit within `max_cols` columns and `max_rows`
/// character rows (each row is two pixels tall). Returns `None` if the bytes
/// aren't a decodable image.
pub fn render_thumbnail(bytes: &[u8], max_cols: u32, max_rows: u32) -> Option<ImageArt> {
    use image::imageops::FilterType;

    let img = image::load_from_memory(bytes).ok()?;
    let (w, h) = (img.width().max(1), img.height().max(1));

    let max_w = max_cols.max(1);
    let max_h = max_rows.max(1) * 2; // two pixels per character row
    let scale = f64::min(max_w as f64 / w as f64, max_h as f64 / h as f64).min(1.0);
    let tw = ((w as f64) * scale).round().clamp(1.0, max_w as f64) as u32;
    let th = ((h as f64) * scale).round().clamp(2.0, max_h as f64) as u32;

    let rgba = img.resize_exact(tw, th, FilterType::Triangle).to_rgba8();
    let sample = |x: u32, y: u32| -> (u8, u8, u8) {
        let p = rgba.get_pixel(x, y).0;
        // Flatten any transparency over a black background.
        let a = p[3] as u32;
        let blend = |c: u8| ((c as u32 * a) / 255) as u8;
        (blend(p[0]), blend(p[1]), blend(p[2]))
    };

    let mut rows = Vec::new();
    let mut y = 0;
    while y < th {
        let mut row = Vec::with_capacity(tw as usize);
        for x in 0..tw {
            let top = sample(x, y);
            let bottom = if y + 1 < th { sample(x, y + 1) } else { top };
            row.push(Cell { top, bottom });
        }
        rows.push(row);
        y += 2;
    }
    Some(ImageArt { rows })
}

/// Open a file with the operating system's default handler.
pub fn open_external(path: &Path) -> std::io::Result<()> {
    use std::process::Command;

    #[cfg(target_os = "windows")]
    {
        // `start` is a cmd builtin; the empty "" is the (ignored) window title.
        Command::new("cmd")
            .args(["/C", "start", "", &path.to_string_lossy()])
            .spawn()?;
    }
    #[cfg(target_os = "macos")]
    {
        Command::new("open").arg(path).spawn()?;
    }
    #[cfg(all(unix, not(target_os = "macos")))]
    {
        Command::new("xdg-open").arg(path).spawn()?;
    }
    Ok(())
}

/// A human-friendly byte size like `12.3 KB`.
pub fn human_size(bytes: i64) -> String {
    const UNITS: [&str; 5] = ["B", "KB", "MB", "GB", "TB"];
    let mut size = bytes as f64;
    let mut unit = 0;
    while size >= 1024.0 && unit < UNITS.len() - 1 {
        size /= 1024.0;
        unit += 1;
    }
    if unit == 0 {
        format!("{bytes} B")
    } else {
        format!("{size:.1} {}", UNITS[unit])
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::path::Path;

    #[test]
    fn mime_by_extension() {
        assert_eq!(guess_mime(Path::new("a/b/photo.PNG")), "image/png");
        assert_eq!(guess_mime(Path::new("x.jpeg")), "image/jpeg");
        assert_eq!(guess_mime(Path::new("no_ext")), "application/octet-stream");
    }

    #[test]
    fn human_size_scales() {
        assert_eq!(human_size(512), "512 B");
        assert_eq!(human_size(1536), "1.5 KB");
        assert_eq!(human_size(5 * 1024 * 1024), "5.0 MB");
    }

    #[test]
    fn thumbnail_fits_bounds_and_rejects_junk() {
        // Encode a small wide image, then check the thumbnail fits the bounds.
        let mut img = image::RgbImage::new(80, 20);
        for p in img.pixels_mut() {
            *p = image::Rgb([200, 50, 50]);
        }
        let mut buf = std::io::Cursor::new(Vec::new());
        image::DynamicImage::ImageRgb8(img)
            .write_to(&mut buf, image::ImageFormat::Png)
            .unwrap();
        let png = buf.into_inner();

        let art = render_thumbnail(&png, 40, 12).expect("decodes png");
        assert!(!art.rows.is_empty());
        assert!(art.rows.len() <= 12);
        assert!(art.rows.iter().all(|r| r.len() <= 40));

        assert!(render_thumbnail(b"not an image", 40, 12).is_none());
    }
}

//! 图片预览：PNG、JPEG、GIF、WebP、BMP、ICO/CUR 解码为 RGBA，按窗口大小缩放显示；SVG 由
//! [`crate::svg`] 按显示大小现场绘制。
//!
//! 解码在后台线程进行，标签先打开。很大的图先缩到长边不超过 `KEEP_MAX`（2560 像素）再保留：
//! 一张 4000×3000 的照片按 RGBA8 原样保留要 48 MB，缩小后约 20 MB。PNG 边解码边缩小；macOS
//! 上要缩小的 JPEG 由 `ImageIO` 按目标大小解码（见 `raster/imageio.rs`）。

use std::path::Path;
use std::sync::Arc;

#[cfg(all(test, target_os = "macos"))]
use crate::raster::imageio;
#[cfg(test)]
use crate::raster::{ADAM7, decode_jpeg, decode_jpeg_fitted, decode_png, resize_rows, shrink};
use crate::raster::{FILE_MAX, KEEP_MAX, decode, decode_fitted, sniff};
pub use crate::raster::{Pixels, decode_bytes, resize};
use crate::svg::{Drawing, Glyphs};

/// 图片内容：位图像素，或 SVG 图形（按显示大小现场绘制，任意缩放都保持清晰）。
#[derive(Debug, Clone)]
pub enum Source {
    /// 位图，已解码成 RGBA8。
    Raster(Pixels),
    /// SVG 图形，按显示大小临时绘制。
    Vector(Arc<Drawing>),
}

/// 一张用于预览的图片。
#[derive(Debug, Clone)]
pub struct Picture {
    /// 原图宽度（像素），状态栏显示；SVG 取 SVG 声明的大小。
    pub width: u32,
    /// 原图高度（像素）。
    pub height: u32,
    /// 格式名，例如 `PNG`、`JPEG`、`SVG`。
    pub format: &'static str,
    /// 文件大小（字节）。
    pub file_size: u64,
    /// 图片内容：位图或 SVG 图形。
    pub source: Source,
    /// 是否含透明像素；为 `true` 时绘制在棋盘格上。
    pub transparent: bool,
    /// 每个逻辑像素对应的像素数（见 [`density`]）。
    pub density: u32,
}

impl Picture {
    /// 渲染为 `width` × `height` 像素的位图。
    pub fn render(&self, width: u32, height: u32) -> Pixels {
        match &self.source {
            Source::Raster(pixels) => resize(pixels, width, height),
            Source::Vector(drawing) => drawing.render(width, height),
        }
    }

    /// 返回 SVG 中未能绘制的内容的名称（如文字、滤镜，用于状态栏提示）；位图返回空切片。
    pub fn missing(&self) -> &[&'static str] {
        match &self.source {
            Source::Vector(drawing) => &drawing.missing,
            Source::Raster(_) => &[],
        }
    }
}

/// 是否按扩展名识别为可直接预览的图片。
///
/// SVG 是文本，仍按文本打开，见 [`is_svg`]。
pub fn is_image(path: &Path) -> bool {
    format_of(path).is_some()
}

/// 是否为 SVG 文件（按扩展名判断）；SVG 按文本打开，可切换到预览。
pub fn is_svg(path: &Path) -> bool {
    path.extension()
        .and_then(|e| e.to_str())
        .is_some_and(|e| e.eq_ignore_ascii_case("svg"))
}

fn format_of(path: &Path) -> Option<&'static str> {
    let ext = path.extension()?.to_str()?.to_ascii_lowercase();
    Some(match ext.as_str() {
        "png" | "apng" => "PNG",
        "jpg" | "jpeg" | "jpe" | "jfif" => "JPEG",
        "gif" => "GIF",
        "webp" => "WebP",
        "bmp" | "dib" => "BMP",
        "ico" => "ICO",
        "cur" => "CUR",
        _ => return None,
    })
}

/// 读取并解码一张图片。
///
/// 长边超过 `KEEP_MAX`（2560 像素）的图会被缩小，但 `width`、`height` 仍是原图的大小。
///
/// # Errors
///
/// 格式不支持、文件无法读取、不是普通文件、超过 `FILE_MAX`（256 MiB）或解码失败时返回原因。
pub fn load(path: &Path) -> Result<Picture, String> {
    let named = format_of(path).ok_or(winc_i18n::t!("preview-image-format-unsupported"))?;
    let metadata = winc_backend::fs::metadata(path).map_err(|e| e.to_string())?;
    // 管道、设备文件即使有图片扩展名也不能读取：读取没有终点，会使后台线程一直阻塞。
    if !metadata.is_file() {
        return Err(winc_i18n::t!("preview-image-not-regular-file").to_owned());
    }
    let file_size = metadata.len;
    if file_size > FILE_MAX {
        return Err(winc_i18n::t!("preview-image-too-large").to_owned());
    }
    let bytes = winc_backend::fs::read(path).map_err(|e| e.to_string())?;
    // 扩展名与内容不符（`.png` 实为 JPEG）时，以内容识别的格式为准。
    let format = sniff(&bytes).unwrap_or(named);
    let (pixels, (width, height)) = decode(format, &bytes)
        .map_err(|e| winc_i18n::t!("preview-image-decode-failed", error = e.to_string()))?;
    // 在缩小后的像素上检查透明度，需要检查的像素少得多。
    let transparent = format != "JPEG" && pixels.rgba.as_chunks::<4>().0.iter().any(|p| p[3] < 255);
    Ok(Picture {
        width,
        height,
        format,
        file_size,
        source: Source::Raster(pixels),
        transparent,
        density: density(Some(path), &bytes),
    })
}

/// 读取一张 SVG 用于预览。
///
/// `base` 是 SVG 文件所在的目录，`<image>` 引用同目录的位图时从这里查找；
/// `glyphs` 提供文字所需的字形，为 `None` 时不绘制文字。
///
/// # Errors
///
/// XML 语法错误或根元素不是 `<svg>` 时返回原因。
pub fn load_svg(
    text: &str,
    base: Option<&Path>,
    glyphs: Option<Arc<dyn Glyphs>>,
) -> Result<Picture, String> {
    let drawing = Drawing::parse(text, base, glyphs)?;
    Ok(Picture {
        width: drawing.width.round().max(1.0) as u32,
        height: drawing.height.round().max(1.0) as u32,
        format: "SVG",
        file_size: text.len() as u64,
        source: Source::Vector(Arc::new(drawing)),
        transparent: true,
        density: 1,
    })
}

/// 返回图片的像素密度：每个逻辑像素对应的像素数。
///
/// 主名以 `@2x`、`@3x` 结尾时取 2、3；否则按 PNG 的 `pHYs`：约 144 dpi 为 2，
/// 约 216 dpi 为 3，其余为 1。
pub fn density(path: Option<&Path>, bytes: &[u8]) -> u32 {
    let stem = path.and_then(|p| p.file_stem()).and_then(|s| s.to_str());
    for (suffix, n) in [("@2x", 2), ("@3x", 3)] {
        if stem.is_some_and(|s| s.ends_with(suffix)) {
            return n;
        }
    }
    match png_dpi(bytes) {
        Some(140..=150) => 2,
        Some(210..=222) => 3,
        _ => 1,
    }
}

/// 返回 PNG 的 `pHYs` 中的横向分辨率（dpi）；不是 PNG、没有 `pHYs` 或单位不是米时为 `None`。
///
/// `pHYs` 位于图像数据之前，读到 `IDAT` 时停止。
fn png_dpi(bytes: &[u8]) -> Option<u32> {
    let mut at = bytes.strip_prefix(b"\x89PNG\r\n\x1a\n").map(|_| 8)?;
    while let Some(head) = bytes.get(at..at + 8) {
        let len = u32::from_be_bytes(head[..4].try_into().ok()?) as usize;
        let kind = &head[4..8];
        if kind == b"IDAT" {
            return None;
        }
        if kind == b"pHYs" {
            let data = bytes.get(at + 8..at + 8 + 9)?;
            let ppm = u32::from_be_bytes(data[..4].try_into().ok()?);
            return (data[8] == 1).then(|| (ppm as f64 * 0.0254).round() as u32);
        }
        at = at.checked_add(12 + len)?;
    }
    None
}

/// 按内容识别格式并解码内存中的一张图片 `bytes`（docx 等文档包中的图片）。
///
/// 与 [`load`] 相同，长边超过 `KEEP_MAX` 的图会被缩小。
///
/// # Errors
///
/// 无法识别格式或解码失败时返回原因。
pub fn load_bytes(bytes: &[u8]) -> Result<Picture, String> {
    let format = sniff(bytes).ok_or(winc_i18n::t!("preview-image-format-unknown"))?;
    let (pixels, (width, height)) = decode(format, bytes)
        .map_err(|e| winc_i18n::t!("preview-image-decode-failed", error = e.to_string()))?;
    let transparent = format != "JPEG" && pixels.rgba.as_chunks::<4>().0.iter().any(|p| p[3] < 255);
    Ok(Picture {
        width,
        height,
        format,
        file_size: bytes.len() as u64,
        source: Source::Raster(pixels),
        transparent,
        density: density(None, bytes),
    })
}

/// Decode a thumbnail with a bounded long edge, preserving the image's proportions.
///
/// # Errors
///
/// Returns an error if the format is unknown or the image cannot be decoded.
pub fn decode_thumbnail(path: &Path, bytes: &[u8], edge: u32) -> Result<Pixels, String> {
    let format = sniff(bytes)
        .or_else(|| format_of(path))
        .ok_or(winc_i18n::t!("preview-image-format-unknown"))?;
    decode_fitted(format, bytes, edge.clamp(1, KEEP_MAX)).map(|(pixels, _)| pixels)
}

/// 把文件大小格式化为「245 KB」「1.4 MB」这样的文字（1 KB = 1024 字节）。
pub fn size_label(bytes: u64) -> String {
    match bytes {
        b if b < 1024 => format!("{b} B"),
        b if b < 1 << 20 => format!("{} KB", (b + 512) >> 10),
        b => format!("{:.1} MB", b as f64 / f64::from(1 << 20)),
    }
}

#[cfg(test)]
mod tests;

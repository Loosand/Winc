//! Bounded previews for attachment cards and folder covers.

use std::fs::{self, File};
use std::io::Read;
use std::path::Path;

use winc_preview::image::{self, Pixels};
use winc_preview::{json, svg};

// Sized for the typical library card on a high-density display.
const THUMBNAIL_EDGE: u32 = 448;
const IMAGE_BYTES: u64 = 32 * 1024 * 1024;
const SVG_BYTES: u64 = 2 * 1024 * 1024;
const JSON_LINES: usize = 14;
const JSON_CHARS: usize = 160;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum AttachmentKind {
    Raster,
    Svg,
    Json,
}

impl AttachmentKind {
    pub(super) fn of(path: &Path) -> Option<Self> {
        if image::is_image(path) {
            Some(Self::Raster)
        } else if image::is_svg(path) {
            Some(Self::Svg)
        } else if path
            .extension()
            .is_some_and(|ext| ext.eq_ignore_ascii_case("json"))
        {
            Some(Self::Json)
        } else {
            None
        }
    }
}

pub(crate) enum AttachmentPreview {
    Image(Pixels),
    Json(JsonExcerpt),
}

pub(crate) enum JsonExcerpt {
    Fields { rows: Vec<json::Row>, more: bool },
    Source(Vec<String>),
}

pub(super) fn load(root: &Path, path: &Path, kind: AttachmentKind) -> Option<AttachmentPreview> {
    let limit = match kind {
        AttachmentKind::Raster => IMAGE_BYTES,
        AttachmentKind::Svg => SVG_BYTES,
        AttachmentKind::Json => json::MAX_BYTES as u64,
    };
    let mut bytes = read_preview(root, path, limit)?;
    let truncated = bytes.len() as u64 > limit;
    if kind == AttachmentKind::Json {
        bytes.truncate(limit as usize);
        let text = String::from_utf8_lossy(&bytes);
        return json_excerpt(text.trim_start_matches('\u{feff}'), truncated)
            .map(AttachmentPreview::Json);
    }
    if truncated {
        return None;
    }
    let pixels = match kind {
        AttachmentKind::Raster => image::decode_thumbnail(path, &bytes, THUMBNAIL_EDGE),
        AttachmentKind::Svg => svg::thumbnail(
            std::str::from_utf8(&bytes).ok()?,
            Some(crate::app::preview::images::system_glyphs()),
            THUMBNAIL_EDGE,
        ),
        AttachmentKind::Json => unreachable!(),
    };
    pixels.ok().map(AttachmentPreview::Image)
}

#[allow(
    clippy::disallowed_methods,
    reason = "Library cards read a bounded prefix of a local regular file"
)]
fn read_preview(root: &Path, path: &Path, limit: u64) -> Option<Vec<u8>> {
    let metadata = fs::symlink_metadata(path).ok()?;
    if !metadata.is_file()
        || crate::system::is_dataless(&metadata)
        || !winc_config::files::canonicalize(path)
            .ok()?
            .starts_with(root)
    {
        return None;
    }
    let mut bytes = Vec::new();
    File::open(path)
        .ok()?
        .take(limit + 1)
        .read_to_end(&mut bytes)
        .ok()?;
    Some(bytes)
}

fn json_excerpt(text: &str, truncated: bool) -> Option<JsonExcerpt> {
    let text = text.trim_start();
    if !truncated
        && let Ok(graph) = json::parse(text)
        && let Some(mut rows) = graph.into_iter().next()
    {
        let more = rows.len() > JSON_LINES;
        rows.truncate(JSON_LINES);
        return Some(JsonExcerpt::Fields { rows, more });
    }
    let rows: Vec<_> = text
        .lines()
        .take(JSON_LINES)
        .map(|line| {
            let mut chars = line.trim_end().chars();
            let mut prefix: String = chars.by_ref().take(JSON_CHARS).collect();
            if chars.next().is_some() {
                prefix.push('…');
            }
            prefix
        })
        .collect();
    (!rows.is_empty()).then_some(JsonExcerpt::Source(rows))
}

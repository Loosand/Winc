//! 已安装的字体：设置中的字体菜单从这里取字族，按名字找不到字体时也在这里按宽松的写法再找一次。
//!
//! macOS 向 CoreText 查询字族；其他平台读取字体目录下每个字体文件的名字表。

use std::path::PathBuf;
use std::sync::OnceLock;

use swash::FontRef;
#[cfg(any(not(target_os = "macos"), test))]
use swash::{Style, Weight};

use super::face::{Face, load, select};
use super::font_path::{FontFile, normalize};

/// 一次字体请求：按名字查系统，或直接使用给定的路径。
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum FontSpec {
    /// 字体名或字族名，例如 `Menlo`、`Helvetica`、`苹方-简`。
    ///
    /// macOS 由 CoreText 查询，识别字族名、PostScript 名和本地化名称；其他平台按文件名前缀匹配，
    /// 忽略空格、连字符、下划线和大小写。这些方式都找不到时，再按同样宽松的写法匹配已安装的字族名。
    Name(String),
    /// 字体文件路径。
    Path(PathBuf),
}

/// 不能使用字体的原因。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum FontError {
    /// 系统里没有该名字的字体，或路径不是文件。
    NotFound,
    /// 找到了字体文件，但无法打开，或其中的字形格式画不出来。
    Unsupported,
}

impl FontSpec {
    /// 确认字体存在，并且能用来显示文字。
    ///
    /// 渲染层遇到不能用的字体时静默改用默认字体，调用方据此提前告知用户原因。
    ///
    /// # Errors
    ///
    /// 找不到字体时返回 [`FontError::NotFound`]，找到了但画不出字形时返回
    /// [`FontError::Unsupported`]。
    pub fn check(&self) -> Result<(), FontError> {
        let file = self.resolve().ok_or(FontError::NotFound)?;
        Face::open(&file).map(drop).ok_or(FontError::Unsupported)
    }

    /// 解析成字体文件；路径不是文件，或系统里没有该名字的字体时返回 `None`。
    ///
    /// 返回 `None` 时调用方退回内置候选。
    pub(crate) fn resolve(&self) -> Option<FontFile> {
        match self {
            FontSpec::Path(path) => path.is_file().then(|| FontFile::first(path.clone())),
            FontSpec::Name(name) => resolve_name(name.trim()),
        }
    }
}

/// 返回字体名 `name` 对应的字体文件；系统里没有该字体时返回 `None`。
///
/// 系统界面字体无法按名字向 CoreText 查询，`SF Mono`、`SF Pro` 直接对应系统字体文件；用户
/// 自行安装了同名字体时优先使用安装的字体。
#[cfg(target_os = "macos")]
fn resolve_name(name: &str) -> Option<FontFile> {
    super::font_path::exact_font(name)
        .or_else(|| find_family(name).map(|family| family.file.clone()))
}

/// 返回字体名 `name` 对应的字体文件：先按文件名查找，再按字族名查找（文件名与字族名无关的
/// 字体，例如 `msyh.ttc`），找不到时返回 `None`。
#[cfg(not(target_os = "macos"))]
fn resolve_name(name: &str) -> Option<FontFile> {
    super::font_path::resolve_file_name(name)
        .or_else(|| find_family(name).map(|family| family.file.clone()))
}

/// 一个已安装的字族。
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct FontFamily {
    /// 字族名，可以直接作为字体设置的值。
    pub name: String,
    /// 是否为等宽字体。
    pub monospace: bool,
    /// 字族中常规体所在的文件。
    pub(crate) file: FontFile,
}

/// 返回已安装的字族，按名字排序（不区分大小写）。
///
/// 第一次调用时读取字体文件，字体多时耗时明显，应先在后台线程调用一次；之后直接返回结果。程序
/// 运行期间新安装的字体需要重启后才能列出。
pub fn installed_fonts() -> &'static [FontFamily] {
    static FONTS: OnceLock<Vec<FontFamily>> = OnceLock::new();
    FONTS.get_or_init(|| sorted(list()))
}

/// 返回规范化后的名字（见 [`normalize`]）与 `name` 相同的字族；没有时返回 `None`。
pub(crate) fn find_family(name: &str) -> Option<&'static FontFamily> {
    let target = normalize(name);
    if target.is_empty() {
        return None;
    }
    installed_fonts()
        .iter()
        .find(|family| normalize(&family.name) == target)
}

/// 按名字排序；规范化后相同的名字只留一个。
fn sorted(mut families: Vec<FontFamily>) -> Vec<FontFamily> {
    families.sort_by_cached_key(|family| (family.name.to_lowercase(), family.name.clone()));
    families.dedup_by_key(|family| normalize(&family.name));
    families
}

/// 返回 CoreText 列出的字族，以及它不列出但可以按名字使用的系统字体；以 `.` 开头的名字是系统保留
/// 的，不列出。
#[cfg(target_os = "macos")]
fn list() -> Vec<FontFamily> {
    use super::font_path::{exact_font, macos_families};

    macos_families()
        .into_iter()
        .chain(["SF Mono".to_owned(), "SF Pro".to_owned()])
        .filter(|name| !name.starts_with('.'))
        .filter_map(|name| {
            let file = exact_font(&name)?;
            let monospace = file_is_monospace(&file);
            Some(FontFamily {
                name,
                monospace,
                file,
            })
        })
        .collect()
}

/// 返回 `file` 是否为等宽字体；无法读取时返回 `false`。
#[cfg(target_os = "macos")]
fn file_is_monospace(file: &FontFile) -> bool {
    let Some(data) = load(&file.path) else {
        return false;
    };
    select(&data, file.postscript.as_deref()).is_some_and(|selected| is_monospace(&selected.font))
}

/// 返回字体目录下各字体文件中的字族。
#[cfg(not(target_os = "macos"))]
fn list() -> Vec<FontFamily> {
    let paths = super::font_path::font_index().iter().map(|(_, path)| path);
    scan(paths)
}

/// 读取 `paths` 中各字体文件的名字表，返回其中的字族；字族中有多个字体时取最接近常规体的一个。
///
/// 不是字体或画不出字形的文件跳过。
#[cfg(any(not(target_os = "macos"), test))]
fn scan<'a>(paths: impl Iterator<Item = &'a std::path::PathBuf>) -> Vec<FontFamily> {
    use std::collections::HashMap;

    let mut found: HashMap<String, (u32, FontFamily)> = HashMap::new();
    for path in paths {
        let is_font = path.extension().and_then(|e| e.to_str()).is_some_and(|e| {
            ["ttf", "otf", "ttc", "otc"]
                .iter()
                .any(|known| e.eq_ignore_ascii_case(known))
        });
        if !is_font {
            continue;
        }
        let Some(data) = load(path) else {
            continue;
        };
        let count = swash::FontDataRef::new(&data).map_or(0, |d| d.len());
        for font in (0..count).filter_map(|i| FontRef::from_index(&data, i)) {
            let Some(name) = family_name(&font).filter(|n| !n.starts_with('.')) else {
                continue;
            };
            let postscript = super::face::postscript_name(&font);
            let usable = select(&data, postscript.as_deref()).is_some();
            if !usable {
                continue;
            }
            let distance = distance_from_regular(&font);
            let family = FontFamily {
                name: name.clone(),
                monospace: is_monospace(&font),
                file: FontFile {
                    path: path.clone(),
                    postscript,
                },
            };
            let key = normalize(&name);
            let closer = found.get(&key).is_none_or(|(best, _)| distance < *best);
            if closer {
                found.insert(key, (distance, family));
            }
        }
    }
    found.into_values().map(|(_, family)| family).collect()
}

/// 返回字体所属的字族名：优先取把各字重归为一族的排版字族名。
#[cfg(any(not(target_os = "macos"), test))]
fn family_name(font: &FontRef) -> Option<String> {
    let strings = font.localized_strings();
    let name = strings
        .find_by_id(swash::StringId::TypographicFamily, None)
        .or_else(|| strings.find_by_id(swash::StringId::Family, None))?;
    let name: String = name.chars().collect();
    let name = name.trim();
    (!name.is_empty()).then(|| name.to_owned())
}

/// 返回字体与常规体的差距，越小越接近：斜体的差距大于任何字重的差距。
#[cfg(any(not(target_os = "macos"), test))]
fn distance_from_regular(font: &FontRef) -> u32 {
    let (_, weight, style) = font.attributes().parts();
    let weight = u32::from(weight.0.abs_diff(Weight::NORMAL.0));
    match style {
        Style::Normal => weight,
        _ => weight + 1000,
    }
}

/// 返回字体是否为等宽字体。
///
/// 字体声明的等宽标志不可靠（含中文字形的等宽字体常常不设置），因此同时比较窄字母和宽字母的
/// 字宽。
fn is_monospace(font: &FontRef) -> bool {
    if font.metrics(&[]).is_monospace {
        return true;
    }
    let charmap = font.charmap();
    let glyphs = font.glyph_metrics(&[]);
    let [narrow, wide] = ['i', 'W'].map(|c| glyphs.advance_width(charmap.map(c)));
    charmap.map('i') != 0 && charmap.map('W') != 0 && narrow > 0.0 && narrow == wide
}

#[cfg(test)]
mod tests;

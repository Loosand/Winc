//! 单个字体文件的加载、选择和可变字体字重调整。
//!
//! 负责按需映射或读取字体文件（[`load`]），在字体集合中按 PostScript 名选择字体或命名实例
//! （[`select`]），以及按字重偏移打开单个 [`Face`]。

use std::fs::{File, Metadata};
use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex, PoisonError, Weak};
use std::time::SystemTime;

use swash::{CacheKey, FontRef, StringId};

use crate::text::font_path::FontFile;
use crate::text::system_outline::{Coverage, GlyphPath, SystemOutlines};

/// 由系统轮廓接口光栅化的结果。
pub(crate) enum SystemFill {
    /// 字体不由系统提供轮廓，调用方应改用 swash。
    NotSystem,
    /// 系统提供了轮廓，结果为位图（字形为空或无轮廓时为 `None`）。
    Mask(Option<Coverage>),
}

/// 一个已经打开的字体文件。
pub(crate) struct Face {
    /// 字体文件的内容，与打开同一个文件的其他字体共用，见 [`load`]。
    data: Arc<FontData>,
    /// 所选字体在文件中的偏移（字节）。
    offset: u32,
    /// swash 用来区分不同字体的缓存键。
    key: CacheKey,
    /// 可变字体的归一化坐标：请求的是命名实例（如 `PingFangSC-Regular`）时取该实例的坐标，
    /// 否则把 `wght` 轴固定为 400 字重，再加上打开时给定的偏移；不是可变字体，或以不固定字重的
    /// 方式打开时为空。
    ///
    /// 系统等宽字体 `SFNSMono.ttf` 的 `wght` 轴默认值约为 295（取自该字体的 `fvar` 表），
    /// 低于常规字重 400，因此需要固定。
    pub(crate) coords: Vec<i16>,
    /// 字形只放在 `hvgl` 表中时，由系统提供轮廓；否则为 `None`，由 swash 读取。
    system: Option<SystemOutlines>,
    /// 笔画加宽的量与字号之比，补足字体自身达不到的字重。
    pub(crate) embolden: f32,
}

impl Face {
    /// 返回指向映射数据的字体引用。
    pub(crate) fn font(&self) -> FontRef<'_> {
        FontRef {
            data: &self.data,
            offset: self.offset,
            key: self.key,
        }
    }

    /// 打开 `file` 指定的字体（`.ttc` 集合中按 PostScript 名选取），可变字体不是命名实例时
    /// 把字重固定为 400（见 [`Self::coords`]）。
    ///
    /// 文件无法打开、映射或解析，或字体中没有可绘制的字形数据时返回 `None`。
    pub(crate) fn open(file: &FontFile) -> Option<Face> {
        Self::open_at(file, 0)
    }

    /// 同 [`Self::open`]，字重在字体自身的基础上加 `shift`（正数更粗）。
    ///
    /// 依次采用：沿 `wght` 轴调整、改用同目录中同族的字体（见 `sibling`）、加宽笔画
    /// （[`Self::embolden`]）。笔画只能加宽，调细时达不到的部分不处理。
    pub(crate) fn open_at(file: &FontFile, shift: i16) -> Option<Face> {
        Self::open_as(file, Some(shift))
    }

    /// 同 [`Self::open`]；`pin_weight` 为假时不固定字重，可变字体不是命名实例时坐标为空，由
    /// 调用方按需要的字重另算（SVG 文字的轮廓，见 `outline` 模块）。
    pub(crate) fn open_with(file: &FontFile, pin_weight: bool) -> Option<Face> {
        Self::open_as(file, pin_weight.then_some(0))
    }

    /// 打开字体；`shift` 为 `None` 时不固定字重，否则是字重的偏移。
    fn open_as(file: &FontFile, shift: Option<i16>) -> Option<Face> {
        let data = load(&file.path)?;
        let selected = select(&data, file.postscript.as_deref())?;
        let font = selected.font;
        let (offset, key) = (font.offset, font.key);
        let wanted = f32::from(shift.unwrap_or(0));
        // 没有字重轴：改用同族中字重更接近的字体。
        if wanted != 0.0
            && !selected.system
            && weight_range(&font).is_none()
            && let Some((sibling, reached)) = sibling(file, &font, wanted)
            && let Some(mut face) = Self::open_as(&sibling, Some(0))
        {
            face.embolden = (wanted - reached).max(0.0) / BOLD_STEP * crate::BOLD_RATIO;
            return Some(face);
        }
        // 系统提供轮廓时，CoreText 字体就是所请求的实例（或默认实例），字宽要与之一致，
        // 不再固定字重，也不沿字重轴调整。
        let axis = weight_range(&font).filter(|_| !selected.system);
        let (coords, reached) = match (selected.instance, axis) {
            (Some(instance), Some(axis)) if wanted != 0.0 => {
                let (coords, weight) = instance.shifted(&font, wanted, axis);
                (coords, weight - instance.weight(&font))
            }
            (Some(instance), _) => (instance.coords, 0.0),
            (None, Some((min, max))) if shift.is_some() => {
                let weight = (REGULAR + wanted).clamp(min, max);
                let coords = font.variations().normalized_coords([("wght", weight)]);
                // 轴的范围不含 400 时，以其中最接近 400 的值为基准。
                (coords.collect(), weight - REGULAR.clamp(min, max))
            }
            (None, _) => (Vec::new(), 0.0),
        };
        let embolden = (wanted - reached).max(0.0) / BOLD_STEP * crate::BOLD_RATIO;
        let system = match selected.system {
            true => {
                let name = file.postscript.clone().or_else(|| postscript_name(&font))?;
                Some(SystemOutlines::new(&file.path, &name, &font)?)
            }
            false => None,
        };

        Some(Face {
            data,
            offset,
            key,
            coords,
            system,
            embolden,
        })
    }

    /// 轮廓能否按字重调整：字体有 `wght`（字重）轴，且轮廓由 swash 读取。
    ///
    /// 系统提供的轮廓固定为所请求的实例，不能按字重调整。
    pub(crate) fn has_weight_axis(&self) -> bool {
        self.system.is_none() && weight_axis(&self.font())
    }

    /// 如果本字体由系统提供轮廓，按给定参数光栅化字形；否则返回 [`SystemFill::NotSystem`]。
    pub(crate) fn fill_system(
        &self,
        glyph: u16,
        size_px: f32,
        dx: f32,
        slant: f32,
        widen: f32,
    ) -> SystemFill {
        match &self.system {
            Some(system) => SystemFill::Mask(system.fill(glyph, size_px, dx, slant, widen)),
            None => SystemFill::NotSystem,
        }
    }

    /// 如果本字体由系统提供轮廓，取出 `glyph` 在 `upem` 单位下的路径；否则返回 `None`。
    pub(crate) fn system_outline(&self, glyph: u16, upem: f32) -> Option<GlyphPath> {
        let system = self.system.as_ref()?;
        let mut path = GlyphPath::default();
        system.outline(glyph, upem, &mut path);
        Some(path)
    }

    /// 如果本字体由系统提供轮廓，返回系统报告的字宽；否则返回 `None`。
    #[cfg(all(test, target_os = "macos"))]
    pub(crate) fn system_advance(&self, glyph: u16, em: f32) -> Option<f32> {
        Some(self.system.as_ref()?.advance(glyph, em))
    }
}

/// 字体是否有 `wght`（字重）轴。
fn weight_axis(font: &FontRef) -> bool {
    weight_range(font).is_some()
}

/// 返回 `wght` 轴的最小值和最大值；没有该轴时返回 `None`。
pub(super) fn weight_range(font: &FontRef) -> Option<(f32, f32)> {
    let axis = font.variations().find(|v| v.tag() == WGHT)?;
    Some((axis.min_value(), axis.max_value()))
}

/// 在 `file` 所在的目录中找与 `font` 同族、同样式的字体：字重介于自身与「自身加 `shift`」
/// 之间，且最接近后者。返回该字体和实际得到的字重差。
///
/// 只打开文件名前 [`SIBLING_PREFIX`] 个字符与 `file` 相同的字体文件，包括 `file` 自身。
fn sibling(file: &FontFile, font: &FontRef, shift: f32) -> Option<(FontFile, f32)> {
    let family = weight_family(font)?;
    let (stretch, own, style) = font.attributes().parts();
    let own = f32::from(own.0);
    let wanted = own + shift;
    let prefix = name_prefix(&file.path)?;
    let mut best: Option<(FontFile, f32)> = None;
    for entry in std::fs::read_dir(file.path.parent()?).ok()?.flatten() {
        let path = entry.path();
        let is_font = path.extension().and_then(|e| e.to_str()).is_some_and(|e| {
            ["ttf", "otf", "ttc", "otc"]
                .iter()
                .any(|known| e.eq_ignore_ascii_case(known))
        });
        if !is_font || name_prefix(&path).as_ref() != Some(&prefix) {
            continue;
        }
        let Some(data) = load(&path) else {
            continue;
        };
        let count = swash::FontDataRef::new(&data).map_or(1, |d| d.len());
        for other in (0..count).filter_map(|i| FontRef::from_index(&data, i)) {
            let (other_stretch, weight, other_style) = other.attributes().parts();
            let weight = f32::from(weight.0);
            let between = weight != own && weight >= own.min(wanted) && weight <= own.max(wanted);
            let closer = best
                .as_ref()
                .is_none_or(|(_, reached)| (wanted - weight).abs() < (shift - reached).abs());
            if between
                && closer
                && (other_stretch, other_style) == (stretch, style)
                && weight_range(&other).is_none()
                && weight_family(&other).as_deref() == Some(family.as_str())
            {
                let found = FontFile {
                    path: path.clone(),
                    postscript: postscript_name(&other),
                };
                best = Some((found, weight - own));
            }
        }
    }
    best
}

/// 查找同族字体时比较的文件名前缀的长度（字符）。
///
/// 同族各字重的文件名开头相同（如 `segoeui.ttf` 与 `seguisb.ttf`），据此不必打开目录中的
/// 每个字体文件。
const SIBLING_PREFIX: usize = 3;

/// 返回文件名的前 [`SIBLING_PREFIX`] 个字符（小写）；不足该长度时返回 `None`。
fn name_prefix(path: &std::path::Path) -> Option<String> {
    let name = path.file_name()?.to_string_lossy().to_lowercase();
    let prefix: String = name.chars().take(SIBLING_PREFIX).collect();
    (prefix.chars().count() == SIBLING_PREFIX).then_some(prefix)
}

/// 返回字族名，优先取把各字重归为一族的排版字族名。
fn weight_family(font: &FontRef) -> Option<String> {
    let strings = font.localized_strings();
    let name = strings
        .find_by_id(StringId::TypographicFamily, None)
        .or_else(|| strings.find_by_id(StringId::Family, None))?;
    Some(name.to_string())
}

/// `wght` 轴的标签。
const WGHT: swash::Tag = swash::tag_from_bytes(b"wght");

/// 常规体的字重。
pub(super) const REGULAR: f32 = 400.0;

/// `BOLD_RATIO` 对应的字重差：常规体 400 到粗体 700。
const BOLD_STEP: f32 = 300.0;

/// 可变字体的一个命名实例。
pub(crate) struct Instance {
    /// 实例的归一化坐标。
    coords: Vec<i16>,
    /// 实例在各轴上的取值，顺序与字体的轴相同。
    values: Vec<f32>,
}

impl Instance {
    /// 返回实例的字重；没有 `wght` 轴时为 400。
    fn weight(&self, font: &FontRef) -> f32 {
        let axis = font.variations().position(|v| v.tag() == WGHT);
        axis.and_then(|at| self.values.get(at).copied())
            .unwrap_or(REGULAR)
    }

    /// 返回字重加 `shift`（限制在 `range` 之内）后的归一化坐标和字重，其余各轴不变。
    fn shifted(&self, font: &FontRef, shift: f32, range: (f32, f32)) -> (Vec<i16>, f32) {
        let weight = (self.weight(font) + shift).clamp(range.0, range.1);
        let settings = font.variations().zip(&self.values).map(|(axis, &value)| {
            let value = if axis.tag() == WGHT { weight } else { value };
            (axis.tag(), value)
        });
        let coords = font.variations().normalized_coords(settings).collect();
        (coords, weight)
    }
}

/// swash 能读的字形表：TrueType 轮廓、CFF 轮廓、彩色位图。
///
/// 这几种之外的字形格式 swash 无法绘制：这样的字体字符映射和字宽都正常，排版不出错，但每个字
/// 都是空白。
pub(super) const GLYPH_TABLES: [&[u8; 4]; 5] = [b"glyf", b"CFF ", b"CFF2", b"sbix", b"CBDT"];

/// Apple 未公开格式的字形表。
///
/// macOS 15 起系统自带的苹方只有该表，轮廓由系统提供（见 `SystemOutlines`）。
const SYSTEM_GLYPH_TABLE: &[u8; 4] = b"hvgl";

/// 从字体文件中选出的字体。
pub(crate) struct Selected<'a> {
    /// 选中的字体。
    pub font: FontRef<'a>,
    /// 请求的 PostScript 名是可变字体的命名实例时为该实例；否则为 `None`。
    pub instance: Option<Instance>,
    /// 字形只放在 `SYSTEM_GLYPH_TABLE`（`hvgl` 表）中，轮廓要由系统提供。
    pub system: bool,
}

/// 返回字体文件数据 `data` 中 PostScript 名为 `postscript` 的字体。
///
/// 名字先与各字体自身的 PostScript 名比对，再与可变字体的命名实例比对（例如苹方的
/// `PingFangSC-Regular` 是 `PingFangSC-Medium` 字体中的一个实例）。`postscript` 为 `None` 或
/// 文件中没有该名字时取第一个字体；数据无法解析或选中的字体没有可绘制的字形时返回 `None`。
pub(crate) fn select<'a>(data: &'a [u8], postscript: Option<&str>) -> Option<Selected<'a>> {
    let first = FontRef::from_index(data, 0)?;
    let (font, instance) = postscript
        .and_then(|postscript| find(data, postscript))
        .unwrap_or((first, None));
    let has = |tag: &[u8; 4]| font.table(swash::tag_from_bytes(tag)).is_some();
    let system = if GLYPH_TABLES.iter().any(|tag| has(tag)) {
        false
    } else if cfg!(target_os = "macos") && has(SYSTEM_GLYPH_TABLE) {
        true
    } else {
        return None;
    };
    Some(Selected {
        font,
        instance,
        system,
    })
}

/// 在字体文件数据 `data` 中查找 PostScript 名为 `postscript` 的字体，返回该字体和命名实例
/// （不是命名实例时为 `None`）；找不到时返回 `None`。
fn find<'a>(data: &'a [u8], postscript: &str) -> Option<(FontRef<'a>, Option<Instance>)> {
    let count = swash::FontDataRef::new(data).map_or(1, |d| d.len());
    let fonts = || (0..count).filter_map(|i| FontRef::from_index(data, i));
    let named = |name: Option<swash::LocalizedString>| {
        name.is_some_and(|name| name.chars().eq(postscript.chars()))
    };
    if let Some(font) = fonts().find(|font| {
        named(
            font.localized_strings()
                .find_by_id(StringId::PostScript, None),
        )
    }) {
        return Some((font, None));
    }
    fonts().find_map(|font| {
        let instance = font.instances().find(|instance| {
            instance_postscript(&font, instance.index()).is_some_and(|name| name == postscript)
        })?;
        let instance = Instance {
            coords: instance.normalized_coords().collect(),
            values: instance.values().collect(),
        };
        Some((font, Some(instance)))
    })
}

/// 返回可变字体 `font` 中第 `index` 个命名实例的 PostScript 名；没有时返回 `None`。
///
/// 直接读 `fvar` 表中实例记录末尾的名字编号：swash 0.2 读该编号时少加了记录本身的偏移，读到的
/// 是表头中的数据。实例的坐标由 swash 读取。
pub(super) fn instance_postscript(font: &FontRef, index: usize) -> Option<String> {
    let fvar = font.table(swash::tag_from_bytes(b"fvar"))?;
    let read = |at: usize| {
        let bytes = fvar.get(at..at + 2)?;
        Some(u16::from_be_bytes([bytes[0], bytes[1]]))
    };
    let axes = usize::from(read(4)?);
    let axis_count = usize::from(read(8)?);
    let axis_size = usize::from(read(10)?);
    let size = usize::from(read(14)?);
    // 实例记录依次为：子族名编号、标志、每个轴一个坐标，最后是可选的 PostScript 名编号。
    if size < 6 + 4 * axis_count {
        return None;
    }
    let record = axes + axis_count * axis_size + index * size;
    let id = read(record + 4 + 4 * axis_count)?;
    let name = font
        .localized_strings()
        .find_by_id(StringId::Other(id), None)?;
    Some(name.chars().collect())
}

/// 返回字体自身的 PostScript 名；字体中没有该名字时返回 `None`。
pub(crate) fn postscript_name(font: &FontRef) -> Option<String> {
    let name = font
        .localized_strings()
        .find_by_id(StringId::PostScript, None)?;
    Some(name.chars().collect())
}

/// 字体文件的内容：当前用户无法修改的文件映射进内存，其余的读进内存，见 [`load`]。
pub(crate) enum FontData {
    /// 映射的文件。
    Mapped(memmap2::Mmap),
    /// 读进内存的文件内容。
    ///
    /// Windows 上被映射的文件其他进程无法修改，因此总是映射，不使用该变体。
    #[cfg_attr(not(unix), allow(dead_code))]
    Owned(Vec<u8>),
}

impl std::ops::Deref for FontData {
    type Target = [u8];

    fn deref(&self) -> &[u8] {
        match self {
            FontData::Mapped(map) => map,
            FontData::Owned(bytes) => bytes,
        }
    }
}

/// 一份字体文件内容的来源：路径、长度和修改时间都相同时是同一份内容。
#[derive(PartialEq)]
pub(super) struct Source {
    pub(super) path: PathBuf,
    /// 文件的长度（字节）。
    len: u64,
    /// 文件的修改时间；平台不提供时为 `None`。
    modified: Option<SystemTime>,
}

/// 进程中正在使用的字体文件内容：同一个文件只保留一份，各字重、各字号和各窗口共用。
///
/// 只持有弱引用：最后一个使用者释放后内容随之释放，留下的空项在下一次载入时清除。
pub(super) static LOADED: Mutex<Vec<(Source, Weak<FontData>)>> = Mutex::new(Vec::new());

/// 返回字体文件 `path` 的内容；无法读取时返回 `None`。
///
/// 该文件的内容正在使用、文件此后没有变化时，返回已有的那一份（见 [`LOADED`]）。
///
/// 映射的文件被其他进程截断后，访问被截掉的页会收到 SIGBUS，进程中止。因此只映射当前用户
/// 无法修改的文件（见 [`is_modifiable`]），与其他进程共用系统的页缓存，只有访问过的页占用
/// 内存；当前用户可以修改的字体可能在使用中被替换或截断，读进内存。
pub(crate) fn load(path: &Path) -> Option<Arc<FontData>> {
    let file = File::open(path).ok()?;
    let metadata = file.metadata().ok()?;
    let source = Source {
        path: path.to_owned(),
        len: metadata.len(),
        modified: metadata.modified().ok(),
    };
    let lock = || LOADED.lock().unwrap_or_else(PoisonError::into_inner);
    let in_use = |loaded: &[(Source, Weak<FontData>)]| {
        let (_, data) = loaded.iter().find(|(known, _)| *known == source)?;
        data.upgrade()
    };
    if let Some(data) = in_use(&lock()) {
        return Some(data);
    }
    // 读取时不持有锁：读一个大文件期间，其他线程仍可以载入别的字体。
    let data = Arc::new(read(&file, &metadata)?);
    let mut loaded = lock();
    loaded.retain(|(_, data)| data.strong_count() > 0);
    // 读取期间另一个线程载入了同一个文件：改用已登记的那一份。
    if let Some(data) = in_use(&loaded) {
        return Some(data);
    }
    loaded.push((source, Arc::downgrade(&data)));
    Some(data)
}

/// 读取或映射 `file` 的内容，`metadata` 是该文件的信息；失败时返回 `None`。
fn read(file: &File, metadata: &Metadata) -> Option<FontData> {
    #[cfg(unix)]
    if modifiable(metadata) {
        let mut bytes = Vec::new();
        std::io::Read::read_to_end(&mut &*file, &mut bytes).ok()?;
        return Some(FontData::Owned(bytes));
    }
    #[cfg(not(unix))]
    let _ = metadata;
    map(file).map(FontData::Mapped)
}

/// 权限位中组和其他用户的写权限（`S_IWGRP | S_IWOTH`）。
#[cfg(unix)]
const WRITABLE_BY_OTHERS: u32 = 0o022;

/// 是否可能在使用中被当前用户修改，`metadata` 是文件的信息；见 [`is_modifiable`]。
#[cfg(unix)]
// 本函数只有对 `libc::geteuid` 的一次调用需要 `unsafe`，因此只在此处放开 `unsafe_code`。
#[allow(unsafe_code)]
fn modifiable(metadata: &Metadata) -> bool {
    use std::os::unix::fs::MetadataExt;
    // SAFETY: `geteuid` 没有参数，不访问内存，总是成功。
    let user = unsafe { libc::geteuid() };
    is_modifiable(metadata.uid(), metadata.mode(), user)
}

/// 是否可能在使用中被当前用户修改：`owner`、`mode` 是文件的属主和权限位，`user` 是当前用户。
///
/// 属主是当前用户，或组、其他用户有写权限的文件可能被修改。属于 root 或其他账号的文件（macOS
/// 按需下载的系统字体属于系统的服务账号）只有管理员或该账号能修改，由系统更新。以 root 运行时
/// 其他账号的文件仍可能被其属主修改。
#[cfg(unix)]
pub(super) fn is_modifiable(owner: u32, mode: u32, user: u32) -> bool {
    let by_owner = owner != 0 && (owner == user || user == 0);
    by_owner || mode & WRITABLE_BY_OTHERS != 0
}

/// 把 `file` 以只读方式映射进内存；映射失败时返回 `None`。
#[allow(unsafe_code)]
fn map(file: &File) -> Option<memmap2::Mmap> {
    // SAFETY: `file` 是调用方持有的有效文件句柄，映射为只读。映射期间文件内容必须保持不变：
    // 这里只映射当前用户无法修改的文件（见 `load`），这类文件只有管理员或系统的服务账号能
    // 截断；被截断后访问被截掉的页会触发 SIGBUS，这是内存映射文件固有的限制。
    unsafe { memmap2::Mmap::map(file) }.ok()
}

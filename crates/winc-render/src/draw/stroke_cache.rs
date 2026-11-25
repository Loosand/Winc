//! 图标描边的覆盖率蒙版缓存。
//!
//! 文件树、快速打开的每一行都有文件图标，每帧都要重画；逐像素求到每个形状的距离，比整块蒙版贴
//! 一遍贵一两个数量级。一组形状（量化后、相对整像素位置）的覆盖率只取决于形状本身，与颜色无关，
//! 因此按形状缓存蒙版，绘制时按颜色混合上去，结果与直接描边相同。

use std::hash::{Hash, Hasher};

use crate::cache::{FxHasher, Generations};
use crate::draw::canvas::{IRect, Stroke, StrokeRaster, stroke_area};

/// 描边蒙版缓存一代的软上限（字节，512 KiB）。
///
/// 一个 2× 屏上的文件图标约 30×30 像素，蒙版不到 1 KiB，一屏的图标全部放下仍有余量；每帧角度
/// 都不同的折叠箭头靠上限淘汰。该值是经验值。
const STROKE_CACHE_BYTES: usize = 512 << 10;

/// 缓存蒙版的最大面积（像素，64×64）。
///
/// 图标都比这个上限小；长线、大圆这类更大的描边很少重复，缓存占用的内存不划算，改为直接绘制。
/// 该值是经验值。
const STROKE_CACHE_MAX_PIXELS: usize = 64 * 64;

/// 形状坐标的量化精度（每物理像素的等分数，无量纲）。
///
/// 量化后相同的两组形状共用一张蒙版，覆盖率的差异不超过一两级，肉眼难以分辨。该值是经验值。
const QUANTUM: f32 = 64.0;

/// 圆弧角度的量化精度（每弧度的等分数，无量纲）。
///
/// 取 4096 时，半径 64 像素的圆周上相邻角度相差不到 1/64 像素，量化误差可以忽略。该值是经验值。
const ANGLE_QUANTUM: f32 = 4096.0;

/// 一组描边形状的覆盖率蒙版。
struct StrokeMask {
    /// 量化后的形状和笔宽（相对蒙版左上角，单位为 1/`QUANTUM` 物理像素），
    /// 用来确认哈希值相同的确实是同一组形状。
    key: Box<[i32]>,
    /// 蒙版的宽高（物理像素）。
    size: (i32, i32),
    /// 每像素 1 字节的覆盖率（0–255），按行排列，每行 `size.0` 字节。
    data: Box<[u8]>,
}

/// 图标描边的覆盖率蒙版缓存。
pub(crate) struct StrokeCache {
    /// 量化后形状的哈希值 → 蒙版。
    masks: Generations<u64, StrokeMask>,
    /// 存放量化形状的缓冲，复用以免每次调用都分配。
    scratch: Vec<i32>,
}

impl StrokeCache {
    /// 创建空的描边蒙版缓存。
    pub(crate) fn new() -> Self {
        Self {
            masks: Generations::new(STROKE_CACHE_BYTES),
            scratch: Vec::new(),
        }
    }

    /// 返回用笔宽 `width`（逻辑像素）描 `shapes` 得到的蒙版和它在画布上的位置（物理像素）。
    ///
    /// `scale` 是屏幕缩放系数。蒙版面积超过 `STROKE_CACHE_MAX_PIXELS`、不值得缓存时返回 `None`，
    /// 由调用方直接描边。
    pub(crate) fn get(
        &mut self,
        shapes: &[Stroke],
        width: f32,
        scale: f32,
    ) -> Option<(IRect, &[u8])> {
        let area = stroke_area(shapes, width, scale)?;
        let (w, h) = (area.width(), (area.y1 - area.y0) as usize);
        if w * h > STROKE_CACHE_MAX_PIXELS {
            return None;
        }

        let (ox, oy) = (area.x0 as f32, area.y0 as f32);
        let q = |v: f32| (v * QUANTUM).round() as i32;
        let (qx, qy) = (|x: f32| q(x * scale - ox), |y: f32| q(y * scale - oy));
        let key = &mut self.scratch;
        key.clear();
        key.push(q(width * scale));
        for &shape in shapes {
            match shape {
                Stroke::Line(x0, y0, x1, y1) => key.extend([0, qx(x0), qy(y0), qx(x1), qy(y1)]),
                Stroke::Circle(cx, cy, r) => key.extend([1, qx(cx), qy(cy), q(r * scale)]),
                Stroke::Arc(cx, cy, r, from, to) => {
                    let a = |v: f32| (v * ANGLE_QUANTUM).round() as i32;
                    key.extend([2, qx(cx), qy(cy), q(r * scale), a(from), a(to)]);
                }
            }
        }
        let mut hasher = FxHasher::default();
        key.hash(&mut hasher);

        let rasterize = |key: &[i32]| StrokeMask {
            key: key.into(),
            size: (area.x1 - area.x0, area.y1 - area.y0),
            data: StrokeRaster::new(shapes, width, scale)
                .expect("有范围的形状不为空")
                .mask(),
        };
        let cost = |m: &StrokeMask| {
            m.data.len() + m.key.len() * 4 + std::mem::size_of::<(u64, StrokeMask)>()
        };
        let mask = self
            .masks
            .get_or_insert_with(hasher.finish(), cost, || rasterize(key));
        // 哈希值相同而量化后的形状不同（实际不会发生）：重新生成，替换原来的那张蒙版。
        if *mask.key != **key {
            *mask = rasterize(key);
        }
        // 量化后相同的形状，像素范围的取整可能相差一个像素；按蒙版的宽高贴，多出的一圈覆盖率为 0。
        let at = IRect::new(area.x0, area.y0, mask.size.0, mask.size.1);
        Some((at, &mask.data))
    }
}

//! Markdown 预览中的图片：后台读取与解码、按显示大小绘制，以及释放屏幕之外的图片数据。

use std::collections::VecDeque;

use super::*;

/// 切换文档时最多保留的本地图片缓存数量与渲染像素总字节上限。
const RECENT_IMAGE_SLOTS: usize = 12;
const RECENT_IMAGE_BYTES: usize = 32 * 1024 * 1024;

struct CachedImage {
    path: PathBuf,
    url: String,
    width: u32,
    height: u32,
    density: u32,
    pixels: Vec<Arc<Pixels>>,
    bytes: usize,
}

impl CachedImage {
    fn to_slot(&self) -> ImageSlot {
        ImageSlot::Ready {
            width: self.width,
            height: self.height,
            density: self.density,
            picture: None,
            pixels: self.pixels.clone(),
            drawing: Vec::new(),
        }
    }
}

/// Markdown 预览的图片缓存池：统一管理图片槽位、原始地址、内嵌媒体和失效版本。
#[derive(Default)]
pub(crate) struct ImagePool {
    /// 用到的图片，键为解析后的本地路径。
    slots: HashMap<PathBuf, ImageSlot>,
    /// 各图片在文件中写的地址，键同 `slots`：解码后的图片释放之后据此重新读取。
    urls: HashMap<PathBuf, String>,
    /// docx / EPUB 包中的内嵌图片内容，读取时直接取这里的数据，不读磁盘。
    embedded: HashMap<PathBuf, Arc<[u8]>>,
    /// 刚读取完成（或读取失败）的图片：只重排用到这些图片的块，无需整份重排。
    loaded: Vec<PathBuf>,
    /// 最近访问文档中已按显示尺寸渲染好的本地图片，用于文档来回切换时首帧直接复用。
    recent: VecDeque<CachedImage>,
    /// `recent` 中已渲染像素的总字节数。
    recent_bytes: usize,
    /// 图片全部作废的计数：联网设置改变或切换文档时递增，触发整份重排。
    version: u64,
}

impl ImagePool {
    /// 丢弃已读取的图片并递增版本号（联网设置改变或完全清理缓存时调用）。
    pub(crate) fn forget(&mut self) {
        self.slots.clear();
        self.urls.clear();
        self.recent.clear();
        self.recent_bytes = 0;
        self.version += 1;
    }

    /// 切换文档时保留有界数量的最近本地渲染图片供切回时首帧复用，释放内嵌媒体与待刷新队列。
    pub(super) fn reset(&mut self, seed_recent: bool) {
        for (path, slot) in self.slots.drain() {
            if self.embedded.contains_key(&path)
                || self.urls.get(&path).is_some_and(|u| u.contains("://"))
            {
                continue;
            }
            let ImageSlot::Ready {
                width,
                height,
                density,
                pixels,
                drawing,
                ..
            } = slot
            else {
                continue;
            };
            if !drawing.is_empty() {
                continue;
            }
            let existing = self.recent.iter().position(|c| c.path == path);
            if pixels.is_empty() && existing.is_some() {
                // 上一个文档没用到这张图时 `release_unused` 会清空其像素，保留 `recent` 里已有的渲染像素。
                continue;
            }
            let url = self.urls.get(&path).cloned().unwrap_or_default();
            let bytes: usize = pixels.iter().map(|p| p.rgba.len()).sum();
            if bytes > RECENT_IMAGE_BYTES {
                continue;
            }
            if let Some(pos) = existing
                && let Some(old) = self.recent.remove(pos)
            {
                self.recent_bytes = self.recent_bytes.saturating_sub(old.bytes);
            }
            self.recent_bytes += bytes;
            self.recent.push_front(CachedImage {
                path,
                url,
                width,
                height,
                density,
                pixels,
                bytes,
            });
            while self.recent.len() > RECENT_IMAGE_SLOTS || self.recent_bytes > RECENT_IMAGE_BYTES {
                if let Some(evicted) = self.recent.pop_back() {
                    self.recent_bytes = self.recent_bytes.saturating_sub(evicted.bytes);
                } else {
                    break;
                }
            }
        }
        self.urls.clear();
        self.embedded.clear();
        self.loaded.clear();
        self.version += 1;
        if seed_recent {
            for cached in &self.recent {
                self.slots.insert(cached.path.clone(), cached.to_slot());
                self.urls.insert(cached.path.clone(), cached.url.clone());
            }
        }
    }

    /// 磁盘上的图片文件发生变化时淘汰对应缓存，避免展示旧像素。
    pub(crate) fn evict_paths(&mut self, rescan: bool, paths: &[PathBuf]) {
        let mut removed = false;
        if rescan {
            if !self.recent.is_empty() {
                self.recent.clear();
                self.recent_bytes = 0;
                removed = true;
            }
            let embedded = &self.embedded;
            let urls = &self.urls;
            let before = self.slots.len();
            self.slots.retain(|path, _| {
                embedded.contains_key(path) || urls.get(path).is_some_and(|u| u.contains("://"))
            });
            removed |= self.slots.len() != before;
        } else {
            for path in paths {
                if let Some(pos) = self.recent.iter().position(|c| &c.path == path)
                    && let Some(old) = self.recent.remove(pos)
                {
                    self.recent_bytes = self.recent_bytes.saturating_sub(old.bytes);
                    removed = true;
                }
                if !self.embedded.contains_key(path) && self.slots.remove(path).is_some() {
                    self.urls.remove(path);
                    removed = true;
                }
            }
        }
        if removed {
            self.version += 1;
        }
    }

    /// 用转换结果里的内嵌媒体替换当前内嵌图片集合。
    pub(super) fn replace_embedded(&mut self, items: HashMap<PathBuf, Arc<[u8]>>) {
        self.embedded = items;
    }

    /// 清空内嵌图片集合。
    pub(super) fn clear_embedded(&mut self) {
        self.embedded.clear();
    }

    /// 接收后台读取完成的结果；若加载期间缓存已被作废则忽略，返回是否已应用。
    pub(super) fn finish_load(
        &mut self,
        path: PathBuf,
        result: Result<Arc<Picture>, String>,
    ) -> bool {
        if !matches!(self.slots.get(&path), Some(ImageSlot::Loading)) {
            return false;
        }
        let slot = match result {
            Ok(picture) => ImageSlot::Ready {
                width: picture.width,
                height: picture.height,
                density: picture.density,
                picture: Some(picture),
                pixels: Vec::new(),
                drawing: Vec::new(),
            },
            Err(_) => ImageSlot::Failed,
        };
        self.slots.insert(path.clone(), slot);
        self.loaded.push(path);
        true
    }

    /// 接收后台按目标尺寸绘制完成的像素缓存。
    pub(super) fn finish_draw(&mut self, path: &Path, drawn: Arc<Pixels>) {
        if let Some(ImageSlot::Ready {
            pixels, drawing, ..
        }) = self.slots.get_mut(path)
        {
            let size = (drawn.width, drawn.height);
            drawing.retain(|&d| d != size);
            pixels.retain(|p| (p.width, p.height) != size);
            pixels.insert(0, drawn);
            pixels.truncate(SIZES);
        }
    }

    /// 后台重新读取并绘制失败时清除正在绘制的尺寸记录。
    pub(super) fn cancel_draw(&mut self, path: &Path, size: (u32, u32)) {
        if let Some(ImageSlot::Ready { drawing, .. }) = self.slots.get_mut(path) {
            drawing.retain(|&d| d != size);
        }
    }

    /// 排版开始前取出当前图片版本号并清空本帧新读好的图片队列。
    pub(crate) fn begin_layout(&mut self) -> (u64, Vec<PathBuf>) {
        (self.version, std::mem::take(&mut self.loaded))
    }

    /// 返回供排版器查询尺寸的图片表。
    pub(crate) fn as_typeset_map(&self) -> &HashMap<PathBuf, ImageSlot> {
        &self.slots
    }

    /// 查询图片 `path` 在目标物理尺寸 `size` 下已绘制好的像素。
    pub(crate) fn pixels_for(&self, path: &Path, size: (u32, u32)) -> Option<&Arc<Pixels>> {
        match self.slots.get(path) {
            Some(ImageSlot::Ready { pixels, .. }) => {
                pixels.iter().find(|px| (px.width, px.height) == size)
            }
            _ => None,
        }
    }

    /// 图片 `path` 是否为本地文件（右键菜单可提供「在新标签中打开图片」）。
    pub(crate) fn is_local(&self, path: &Path) -> bool {
        self.urls.get(path).is_none_or(|u| !u.contains("://"))
    }

    /// 释放这一帧未使用的像素与解码图，内存占用仅与屏幕可见图片相关。
    fn release_unused(&mut self, shown: &[PathBuf], wanted: &[PathBuf]) {
        for (path, slot) in &mut self.slots {
            let ImageSlot::Ready {
                picture,
                pixels,
                drawing,
                ..
            } = slot
            else {
                continue;
            };
            // 正在后台绘制的图片保持不变：绘制的结果仍然要采用。
            if !drawing.is_empty() {
                continue;
            }
            if !shown.contains(path) {
                pixels.clear();
                *picture = None;
            } else if !wanted.contains(path) {
                *picture = None;
            }
        }
    }

    #[cfg(test)]
    pub(super) fn is_busy(&self) -> bool {
        self.slots.values().any(|slot| match slot {
            ImageSlot::Loading => true,
            ImageSlot::Ready { drawing, .. } => !drawing.is_empty(),
            ImageSlot::Failed => false,
        })
    }

    #[cfg(test)]
    pub(crate) fn get(&self, path: &Path) -> Option<&ImageSlot> {
        self.slots.get(path)
    }

    #[cfg(test)]
    pub(crate) fn is_empty(&self) -> bool {
        self.slots.is_empty()
    }

    #[cfg(test)]
    pub(crate) fn iter(&self) -> std::collections::hash_map::Iter<'_, PathBuf, ImageSlot> {
        self.slots.iter()
    }

    #[cfg(test)]
    pub(crate) fn first_slot(&self) -> Option<&ImageSlot> {
        self.slots.values().next()
    }

    #[cfg(test)]
    pub(crate) fn first_key(&self) -> Option<PathBuf> {
        self.slots.keys().next().cloned()
    }

    #[cfg(test)]
    pub(crate) fn insert_for_test(&mut self, path: PathBuf, slot: ImageSlot) {
        self.slots.insert(path, slot);
    }

    #[cfg(test)]
    pub(crate) fn mark_loaded_for_test(&mut self, path: PathBuf) {
        self.loaded.push(path);
    }

    #[cfg(test)]
    pub(crate) fn set_height_for_test(&mut self, path: &Path, new_height: u32) {
        if let Some(ImageSlot::Ready { height, .. }) = self.slots.get_mut(path) {
            *height = new_height;
        }
        self.loaded.push(path.to_path_buf());
        self.version += 1;
    }
}

impl App {
    /// 请求图片 `path`（文件中写的地址为 `url`）：尚未读取时交给后台读取，网址的图片先下载。
    pub(crate) fn request_markdown_image(&mut self, path: &Path, url: &str) {
        if self.preview.markdown.images.slots.contains_key(path) {
            return;
        }
        let md = &mut self.preview.markdown;
        md.images
            .slots
            .insert(path.to_path_buf(), ImageSlot::Loading);
        md.images.urls.insert(path.to_path_buf(), url.to_owned());
        let (sender, proxy, path) = (
            md.sender.clone(),
            self.link.proxy.clone(),
            path.to_path_buf(),
        );
        let read = self.markdown_image_reader(&path, url);
        let load = move || {
            if sender
                .send(Done::Loaded(path, read().map(Arc::new)))
                .is_ok()
                && let Some(proxy) = proxy
            {
                let _ = proxy.send_event(UserEvent::ImageReady);
            }
        };
        // 网络图片由 `curl` 下载，最长等待 `REMOTE_TIMEOUT`；
        // 在单独的线程上等待，不占用共用的线程池。
        if is_remote(url) {
            winc_editor::background::spawn_blocking(load);
        } else {
            winc_editor::background::spawn(load);
        }
    }

    /// 返回在后台线程上读取并解码图片 `path`（文件中写的地址为 `url`）的函数：文档内嵌的图片
    /// 取内嵌的数据，网址的图片先下载，其余从磁盘读取。
    pub(super) fn markdown_image_reader(
        &self,
        path: &Path,
        url: &str,
    ) -> impl FnOnce() -> Result<Picture, String> + Send + 'static {
        let path = path.to_path_buf();
        let remote = is_remote(url).then(|| url.to_owned());
        let embedded = self.preview.markdown.images.embedded.get(&path).cloned();
        move || match (&remote, embedded) {
            (_, Some(bytes)) if image::is_svg(&path) => std::str::from_utf8(&bytes)
                .map_err(|e| e.to_string())
                .and_then(|text| image::load_svg(text, None, Some(system_glyphs()))),
            (_, Some(bytes)) => image::load_bytes(&bytes),
            (Some(url), None) => download(url, &path).and_then(|()| load_downloaded(&path)),
            (None, None) if image::is_svg(&path) => winc_backend::fs::read_to_string(&path)
                .map_err(|e| e.to_string())
                .and_then(|text| image::load_svg(&text, path.parent(), Some(system_glyphs()))),
            (None, None) => image::load(&path),
        }
    }

    /// 请求把图片 `path` 按 `size`（物理像素）绘制：已有该尺寸的像素时不提交。
    ///
    /// 每张图同一时刻只在后台绘制一个尺寸：拖动改变窗口大小时每帧尺寸都不同，正在绘制时不提交，
    /// 当前尺寸完成后界面重绘，届时仍缺的尺寸再提交。解码后的图片已经释放时
    /// （见 [`Self::release_markdown_images`]）先重新读取。
    pub(crate) fn request_markdown_pixels(&mut self, path: &Path, size: (u32, u32)) {
        let Some(ImageSlot::Ready {
            picture,
            pixels,
            drawing,
            ..
        }) = self.preview.markdown.images.slots.get_mut(path)
        else {
            return;
        };
        if pixels.iter().any(|p| (p.width, p.height) == size) || !drawing.is_empty() {
            return;
        }
        drawing.push(size);
        let picture = picture.clone();
        let url = self.preview.markdown.images.urls.get(path).cloned();
        let reread = picture.is_none().then(|| {
            let url = url.as_deref().unwrap_or_default();
            (is_remote(url), self.markdown_image_reader(path, url))
        });
        let (sender, proxy, path) = (
            self.preview.markdown.sender.clone(),
            self.link.proxy.clone(),
            path.to_path_buf(),
        );
        let remote = reread.as_ref().is_some_and(|(remote, _)| *remote);
        let draw = move || {
            let picture = match (picture, reread) {
                (Some(picture), _) => Ok(picture),
                (None, Some((_, read))) => read().map(Arc::new),
                (None, None) => Err(String::new()),
            };
            let done = match picture {
                Ok(picture) => Done::Drawn(path, Arc::new(picture.render(size.0, size.1))),
                Err(_) => Done::Undrawn(path, size),
            };
            if sender.send(done).is_ok()
                && let Some(proxy) = proxy
            {
                let _ = proxy.send_event(UserEvent::ImageReady);
            }
        };
        // 重新读取网址的图片可能要再次下载，与首次读取相同，在单独的线程上等待。
        if remote {
            winc_editor::background::spawn_blocking(draw);
        } else {
            winc_editor::background::spawn(draw);
        }
    }

    /// 释放这一帧没有用到的图片数据：`shown` 是这一帧绘制范围内的图片，`wanted` 是其中还缺
    /// 显示大小的像素、已请求绘制的图片。
    ///
    /// 排版只需要图片的宽高，绘制只需要显示大小的像素。不在绘制范围内的图片释放像素和解码后
    /// 的图片；在绘制范围内、像素已经齐全的图片只释放解码后的图片。占用的内存因此只取决于
    /// 屏幕上的图片，与文档中图片的数量无关；图片再次进入绘制范围时重新读取。
    pub(crate) fn release_markdown_images(&mut self, shown: &[PathBuf], wanted: &[PathBuf]) {
        self.preview.markdown.images.release_unused(shown, wanted);
    }
}

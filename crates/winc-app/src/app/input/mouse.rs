//! 鼠标：悬停、点击、拖动和滚轮。
//!
//! 绘制每帧把可点击的区域记入 `*_hits` 表（标签、文件树、编辑区、按钮等），鼠标事件按当前
//! 焦点和这些表找到目标。拖动包括标签排序、文件树拖动、滚动条和文本选区。

mod click;
mod drag;
mod regions;
mod target;

use target::*;

use std::time::Instant;

use winit::event::{MouseButton, MouseScrollDelta, TouchPhase};
use winit::window::CursorIcon;

use winc_core::{Lines, Selection, movement};
use winc_render::Renderer;

use crate::app::{
    App, Click, DOUBLE_CLICK, Drag, Focus, Hover, MenuCommand, TreeDrag, WindowControl,
    resize_cursor,
};
use crate::layout::{
    RowAction, Scrollbar, SearchParts, SettingControl, SettingsPart, SidebarView, hit,
    hscrollbar_thumb, scroll_for_thumb, scrollbar_thumb, settings_thumb, settings_track,
};
use crate::theme::*;

/// 触控板调整字号时，每调整一档所需的滚动距离（逻辑像素）。
///
/// 取自调参，只影响触控板调整字号的灵敏度。
pub(in crate::app) const ZOOM_WHEEL_PIXELS: f32 = 48.0;

impl App {
    /// 处理鼠标按键 `button` 松开：结束拖动，并在需要时关闭标签或执行窗口按钮。
    ///
    /// 左键按在关闭按钮上、或中键按在标签上之后，松开时鼠标仍在同一个标签上才关闭该标签；
    /// 左键按在窗口按钮上之后，松开时鼠标仍在同一个按钮上才执行该按钮。
    pub(in crate::app) fn on_mouse_up(&mut self, button: MouseButton) {
        // 拖动标签时悬停目标被清除，松开后重新判断鼠标下方的部件。
        let was_tab = self.tab_dragging();
        self.pointer.drag_scrolled_at = None;
        if button == MouseButton::Left {
            self.diagram_view_mouse_up();
            if let Some(pressed) = self.take_pressed_folder()
                && let Target::Library(crate::app::workspace::library::LibraryTarget::Card(index)) =
                    self.target_at(self.pointer.mouse.0, self.pointer.mouse.1)
            {
                self.release_library_folder(index, &pressed);
            }
        }
        if button == MouseButton::Left {
            self.release_tree_item();
            self.release_tab();
        }

        if matches!(button, MouseButton::Left | MouseButton::Middle)
            && let Some(id) = self.pointer.close_pressed.take()
            && let Some((index, _)) = self.tab_at(self.pointer.mouse.0, self.pointer.mouse.1)
            && self.editor.documents.get(index).is_some_and(|d| d.id == id)
        {
            self.tabs.width_lock = self.tabs.widths.get(index).map(|&w| (index, w));
            self.close_tab(index);
        }

        if button == MouseButton::Left
            && let Some(control) = self.pointer.control_pressed.take()
            && self.window_control_at(self.pointer.mouse.0, self.pointer.mouse.1) == Some(control)
        {
            self.press_window_control(control);
        }

        self.terminal_mouse_up();
        self.release_terminal_tab();
        let was_scrollbar = matches!(self.pointer.drag, Drag::Scrollbar { .. });
        let was_divider = matches!(
            self.pointer.drag,
            Drag::Splitter { .. }
                | Drag::TerminalDivider { .. }
                | Drag::SplitDivider { .. }
                | Drag::PaneDivider { .. }
                | Drag::OutlineDivider { .. }
        );
        if matches!(self.pointer.drag, Drag::TerminalDivider { .. }) {
            self.terminal_dragging(false);
        }
        if matches!(self.pointer.drag, Drag::Splitter { .. }) {
            self.remember_panel_width();
        }
        if matches!(self.pointer.drag, Drag::ImageResize) {
            self.finish_image_resize();
        }
        if button == MouseButton::Left && matches!(self.pointer.drag, Drag::MarkdownText) {
            self.finish_markdown_drag();
            self.offer_selection_toolbar();
        }
        self.pointer.drag = Drag::None;
        // 拖动分隔条期间不重新断行（`sync_wrap`）；松开后重绘，重绘前的布局按最终宽度断行。
        if was_divider {
            self.request_redraw();
        }
        if was_scrollbar || was_tab {
            self.refresh_pointer_hover();
        }
        self.update_cursor_icon();
    }

    /// 处理窗口的滚轮事件：按住主修饰键时调整字号（见 [`Self::wheel_zoom`]），否则滚动。
    ///
    /// 触控板的手势阶段 `phase` 用于区分两次手势：调整字号与滚动的方向都在手势开始时确定。
    pub(in crate::app) fn on_wheel_event(&mut self, delta: MouseScrollDelta, phase: TouchPhase) {
        let fresh = self.note_wheel_gesture(delta, phase);
        let wheel = move |app: &mut App| {
            if !app.wheel_json(delta) && !app.wheel_zoom(delta, fresh) {
                app.scroll_by_wheel(delta, true);
            }
        };
        // 在前台以外的窗格上滚动：滚动或缩放那个窗格，前台不变。
        let (x, y) = self.pointer.mouse;
        if let Some(pane) = self.background_pane_at(x, y) {
            self.with_pane(pane, wheel);
            self.refresh_pointer_hover();
            self.request_redraw();
        } else if let Some(at) = self.terminal_pane_at(x, y) {
            self.with_terminal_pane(at, wheel);
            self.request_redraw();
        } else {
            wheel(self);
        }
    }

    /// 只按住主修饰键滚动时，调整鼠标所在的编辑区或终端的字号，或缩放 PDF、图片，
    /// 返回事件是否已处理。
    ///
    /// 触控板的一次手势是否调整字号在手势开始时确定：滚动途中按下修饰键仍是滚动，调整途中
    /// 松开修饰键后余下的滚动量丢弃。每个事件至多调整一档。`fresh` 表示触控板开始了新的手势。
    fn wheel_zoom(&mut self, delta: MouseScrollDelta, fresh: bool) -> bool {
        let keys = self.pointer.modifiers;
        let held = self.command_held()
            && keys.control_key() != keys.super_key()
            && !keys.shift_key()
            && !keys.alt_key();
        let zoomed = match self.target_at(self.pointer.mouse.0, self.pointer.mouse.1) {
            Target::Editor if self.on_pdf_body() => Some(Zoomed::Pdf),
            Target::Picture => Some(Zoomed::Picture),
            Target::Editor | Target::Gutter => Some(Zoomed::Editor),
            Target::Terminal | Target::TerminalBlock(_) => Some(Zoomed::Terminal),
            _ => None,
        };
        let amount = match delta {
            MouseScrollDelta::LineDelta(_, y) => {
                if !held || zoomed.is_none() {
                    self.scroll.wheel.zoom = 0.0;
                    return false;
                }
                // macOS 按滚动速度给出不足一格的量，每个事件按一格计。
                if cfg!(target_os = "macos") {
                    y.signum()
                } else {
                    y
                }
            }
            MouseScrollDelta::PixelDelta(delta) => {
                if fresh {
                    self.scroll.wheel.zooming = held && zoomed.is_some();
                    self.scroll.wheel.zoom = 0.0;
                }
                if !self.scroll.wheel.zooming {
                    return false;
                }
                if !held {
                    return true;
                }
                let scale = self
                    .frame
                    .renderer
                    .as_ref()
                    .map_or(1.0, Renderer::scale_factor);
                delta.y as f32 / scale / ZOOM_WHEEL_PIXELS
            }
        };
        let Some(zoomed) = zoomed else {
            return true;
        };
        self.scroll.wheel.zoom += amount;
        if self.scroll.wheel.zoom.abs() >= 1.0 {
            let up = self.scroll.wheel.zoom > 0.0;
            self.scroll.wheel.zoom = 0.0;
            self.dismiss_hint();
            // PDF 和图片缩放时鼠标处的内容保持在原位。
            match zoomed {
                Zoomed::Pdf => drop(self.zoom_pdf(Some(up), Some(self.pointer.mouse))),
                Zoomed::Picture => drop(self.zoom_picture(Some(up), Some(self.pointer.mouse))),
                Zoomed::Editor | Zoomed::Terminal => {}
            }
            if matches!(zoomed, Zoomed::Pdf | Zoomed::Picture) {
                return true;
            }
            // 调整字号会把光标滚入视野；这里保持编辑区顶部的行不变。
            let top = self.editor.active().map(|doc| doc.scroll_line);
            self.step_font_size_in(zoomed == Zoomed::Terminal, Some(up));
            if let Some(top) = top {
                self.set_active_scroll(top);
                self.clamp_scroll();
            }
        }
        true
    }

    /// 处理顶栏空白处的点击：拖动窗口；双击时按系统设置缩放或最小化窗口。
    pub(super) fn click_title_bar(&mut self, x: f32, y: f32) {
        if self.title_bar_pressed(x, y) && self.title_bar_zooms_at(x) {
            self.title_bar_double_click();
        } else if let Some(window) = &self.window.handle {
            let _ = window.drag_window();
        }
    }

    /// 是否在双击顶栏空白处横坐标 `x`（逻辑像素）的位置时执行标题栏的双击动作。
    ///
    /// macOS 上该动作由系统偏好决定，任何位置都执行；其他系统上见
    /// [`Self::title_bar_double_click_zooms`]。
    pub(in crate::app) fn title_bar_zooms_at(&self, x: f32) -> bool {
        cfg!(target_os = "macos") || self.title_bar_double_click_zooms(x)
    }

    /// 记录顶栏空白处按下的位置和时刻，返回本次按下是否构成双击。
    ///
    /// 构成双击时清除记录，下一次按下重新计数。鼠标点击与触摸点按共用该判断。
    pub(in crate::app) fn title_bar_pressed(&mut self, x: f32, y: f32) -> bool {
        let slop = self.repeat_slop();
        let double = self.pointer.title_click.take().is_some_and(|c| {
            c.at.elapsed() < DOUBLE_CLICK && (c.x - x).abs() < slop && (c.y - y).abs() < slop
        });

        if double {
            return true;
        }

        self.pointer.title_click = Some(Click {
            at: Instant::now(),
            x,
            y,
            count: 1,
        });

        false
    }

    /// 记录这次点击并返回连击次数：1 为单击，2 为双击，3 为三击，再点击则重新从 1 开始。
    ///
    /// 相邻两次点击的间隔小于 `DOUBLE_CLICK`，且横纵坐标之差均小于 [`App::repeat_slop`]
    /// 返回的容差时，计为连击。
    pub(in crate::app) fn click_count(&mut self, x: f32, y: f32) -> u8 {
        let slop = self.repeat_slop();
        let count = match self.pointer.last_click {
            Some(c)
                if c.at.elapsed() < DOUBLE_CLICK
                    && (c.x - x).abs() < slop
                    && (c.y - y).abs() < slop =>
            {
                c.count % 3 + 1
            }
            _ => 1,
        };
        self.pointer.last_click = Some(Click {
            at: Instant::now(),
            x,
            y,
            count,
        });
        count
    }
}

/// 按住主修饰键滚动时调整的对象。
#[derive(Clone, Copy, PartialEq, Eq)]
enum Zoomed {
    Editor,
    Terminal,
    Pdf,
    Picture,
}

# `winc-cpui`

Minc 基于 CPU 渲染的声明式桌面界面层（CPUI）。

- **纯 CPU 渲染**：不依赖 GPU、浏览器引擎或系统原生控件，可在离屏环境完整布局、绘制与验证。
- **无状态声明式树**：视图是应用状态的纯函数，交互通过动作标识 `A` 与 `HitMap<A>` 表达。
- **轻量化与高效重绘**：内建视口裁剪剔除、局部脏区重绘（`paint_dirty` / `render_dirty`）、帧间像素快照复用（`SurfaceCache`）与 $O(\text{visible})$ 虚拟列表（`VisibleWindow` / `virtual_list`）。

## 架构层次

```text
应用层（如 winc-app）
  ├── 组件层（widgets）：Theme + 按钮 / 勾选框 / 徽章 / 箭头 / 对话框头尾栏 / 输入框槽位
  ├── 交互层（HitMap / SurfaceCache / popover / scroll）：命中测试、快照复用、浮层定位、滚动与虚拟列表
  ├── 布局层（layout / render / render_dirty / wrap_text）：Taffy Flexbox 测量、折行、绘制与裁剪
  └── 描述层（Node）：vstack / hstack / box_ / text / divider / custom / custom_measured
        └── CPU 渲染器（winc-render）
```

## 快速上手

运行独立离屏示例（无需窗口系统）：

```bash
cargo run -p winc-cpui --example offscreen_demo
```

运行全部单元测试、外部 API 集成测试与文档示例测试：

```bash
cargo test -p winc-cpui
```

## 核心能力速查

| 能力 | 入口 |
|---|---|
| 布局原语 | `vstack`、`hstack`、`hstack_rev`、`box_`、`spacer`、`divider`、`vdivider`、`text`、`custom`、`custom_measured` |
| 布局与局部重绘 | `layout`、`render`、`render_dirty`、`LaidOut::{root_size, paint_at, paint_dirty}`、`wrap_text` |
| 命中与快照复用 | `HitMap`、`SurfaceCache` |
| 弹出层定位与避让 | `place_below_or_above`、`place_at_point`、`place_submenu`、`clamp_rect` |
| 滚动与虚拟列表 | `ScrollMetrics`、`ScrollbarSpec`、`VisibleWindow`、`virtual_list` |
| 主题无关组件 | `Theme`、`widgets::{action_button, action_button_with, checkbox, chevron, chip, dialog_header, dialog_footer, field_slot}` |

## 作为独立项目对外发布的可行性评估

- **当前依赖边界**：`winc-cpui` 仅依赖 `taffy` 与 `winc-render`，已彻底解除与 `winc-app`、`winc-core`、`winc-richtext`、`winc-i18n` 及操作系统窗口库的耦合，支持完整的离屏构建、测试与文档验证。
- **唯一前置工作**：若未来计划发布至 crates.io 供外部项目复用，只需将 `winc-render` 同步作为独立渲染 crate 发布（或在 `winc-cpui` 中抽取最小渲染 trait 覆盖 `Frame` 的矩形、圆角、阴影、描边、文本与裁剪原语），上层 `Node` / `layout` / `popover` / `scroll` / `widgets` 公共接口无需破坏性改动。

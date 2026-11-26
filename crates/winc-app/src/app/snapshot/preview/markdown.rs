//! Markdown 预览的测试：各类块的排版、目录、链接和滚动。

use super::*;

#[test]
fn markdown_can_render_across_the_editing_area() {
    let (mut app, dir) = readme("md-modes", false, PreviewMode::Split);
    let doc = app.editor.active().unwrap();
    assert_eq!(doc.preview_mode(), PreviewMode::Split);
    assert!(doc.shows_text() && !doc.readonly);
    assert_eq!(app.layout.preview.w, 0.0);
    // 右边是预览：绘制在分栏的右半边，已完成排版并记录链接。
    assert_eq!(app.preview.markdown.area, app.layout.split);
    assert!(app.preview.markdown.height > app.preview.markdown.area.h);
    assert!(app.hits.markdown_links.len() >= 2);
    // 图片已读取，并按显示大小绘制。
    assert!(matches!(
        app.preview.markdown.images.first_slot(),
        Some(crate::app::preview::markdown::ImageSlot::Ready { pixels, .. }) if !pixels.is_empty()
    ));

    // 只看预览：整个编辑区，不显示源代码，但可以直接编辑；切回源代码后仍可正常编辑。
    app.set_preview_mode(PreviewMode::Preview);
    settle(&mut app);
    let doc = app.editor.active().unwrap();
    assert!(!doc.readonly && !doc.shows_text() && doc.edits_text());
    assert!(app.markdown_live());
    assert_eq!(app.preview.markdown.area, app.layout.editor);
    app.set_preview_mode(PreviewMode::Source);
    paint(&mut app);
    assert!(!app.editor.active().unwrap().readonly);
    assert_eq!(app.preview.markdown.area, Rect::default());

    let _ = std::fs::remove_dir_all(&dir);
}

#[test]
fn tab_nests_list_items_in_the_preview() {
    use winit::keyboard::{Key, NamedKey};
    let tab = Key::Named(NamedKey::Tab);

    let (mut app, dir) = live_doc("md-tab", "- 一\n- 二\n\n段落\n");
    put_caret(&mut app, "二", true);
    assert!(app.markdown_key(&tab, false));
    assert_eq!(with_caret(&app), "- 一\n  - 二|\n\n段落\n");
    settle(&mut app);
    app.pointer.modifiers = winit::keyboard::ModifiersState::SHIFT;
    assert!(app.markdown_key(&tab, false));
    app.pointer.modifiers = winit::keyboard::ModifiersState::empty();
    assert_eq!(with_caret(&app), "- 一\n- 二|\n\n段落\n");
    settle(&mut app);
    // 第一项无法缩进，也不向文字中插入空格。
    put_caret(&mut app, "一", false);
    assert!(app.markdown_key(&tab, false));
    assert_eq!(with_caret(&app), "- |一\n- 二\n\n段落\n");
    // 不是列表项时交给编辑区处理。
    put_caret(&mut app, "段落", true);
    assert!(!app.markdown_key(&tab, false));

    let _ = std::fs::remove_dir_all(&dir);
}

#[test]
fn clicking_a_task_box_toggles_it() {
    let (mut app, dir) = live_doc("md-task", "开头\n\n- [ ] 甲\n- [x] 乙\n");
    let task = crate::app::preview::markdown::laid::TASK;
    let boxes = |app: &App| -> Vec<winc_render::Rect> {
        app.hits
            .markdown_links
            .iter()
            .filter(|(_, url)| url.starts_with(task))
            .map(|(r, _)| *r)
            .collect()
    };
    let caret = app.editor.active().unwrap().selection;
    let first = boxes(&app)[0];
    super::click(&mut app, (first.x + first.w / 2.0, first.y + first.h / 2.0));
    settle(&mut app);
    assert_eq!(doc_text(&app), "开头\n\n- [x] 甲\n- [x] 乙\n");
    // 光标不移动，也不因此进入编辑状态：列表仍按排版显示，勾选框保留。
    assert_eq!(app.editor.active().unwrap().selection, caret);
    let second = boxes(&app)[1];
    super::click(&mut app, (second.x + 2.0, second.y + 2.0));
    settle(&mut app);
    assert_eq!(doc_text(&app), "开头\n\n- [x] 甲\n- [ ] 乙\n");
    app.run(winc_editor::Command::Undo);
    assert_eq!(doc_text(&app), "开头\n\n- [x] 甲\n- [x] 乙\n");

    let _ = std::fs::remove_dir_all(&dir);
}

#[test]
fn math_and_diagrams_show_the_result_below_the_source() {
    let height = |text: &str| {
        let (mut app, dir) = live_doc("md-below", text);
        put_caret(&mut app, "A", false);
        app.run(winc_editor::Command::Insert(" ".into()));
        settle(&mut app);
        let shown = app.preview.markdown.full_text().unwrap();
        // 光标不在围栏行上时只显示围栏之间的原文，不显示围栏。
        assert!(shown.contains(" A") && !shown.contains("```"), "{shown}");
        assert!(!shown.contains("$$"), "{shown}");
        let height = app.preview.markdown.height;
        let _ = std::fs::remove_dir_all(&dir);
        (height, shown)
    };
    // 原文下方绘制渲染后的图，因此比只显示原文的代码块更高。
    let (diagram, _) = height("```mermaid\ngraph TD\nA-->B\n```\n");
    let (plain, _) = height("```text\ngraph TD\nA-->B\n```\n");
    assert!(diagram > plain + 40.0, "{diagram} {plain}");
    // 公式的原文下面是排好的公式（复制的文字是公式的原文，跟在原文后面）。
    let (_, shown) = height("$$\nA^2 + b\n$$\n");
    assert_eq!(shown.matches("A^2").count(), 2, "{shown}");
}

#[test]
fn clicking_an_anchor_link_scrolls_to_the_heading() {
    let (mut app, dir) = readme("md-anchor", false, PreviewMode::Preview);
    let index = app
        .hits
        .markdown_links
        .iter()
        .position(|(_, url)| url == "#安装")
        .expect("有转到「安装」的链接");
    let (rect, _) = app.hits.markdown_links[index].clone();
    // 可编辑的预览中单击链接只放光标。
    super::click(&mut app, (rect.x + 2.0, rect.y + 2.0));
    assert_eq!(app.preview.markdown.scroll(), 0.0);
    command_click(&mut app, (rect.x + 2.0, rect.y + 2.0));
    let top = app.preview.markdown.anchor_top("安装").unwrap();
    assert!(top > 0.0);
    // 标题滚动到顶部；其后的内容不足一屏时滚动到底为止。
    let max = app.preview.markdown.height - app.preview.markdown.area.h;
    assert_eq!(app.preview.markdown.scroll(), top.min(max));
    // 脚注的编号和末尾的 ↩ 能找到彼此。
    let note = app.preview.markdown.anchor_top("fn-1").expect("脚注有锚点");
    let back = app
        .preview
        .markdown
        .anchor_top("fnref-1")
        .expect("能跳回引用处");
    assert!(back > top && note > back);

    let _ = std::fs::remove_dir_all(&dir);
}

#[test]
fn code_blocks_offer_a_copy_button_on_hover() {
    let (mut app, dir) = readme("md-copy-code", false, PreviewMode::Preview);
    let copy = crate::app::preview::markdown::laid::COPY;
    assert!(
        !app.hits
            .markdown_links
            .iter()
            .any(|(_, url)| url.starts_with(copy))
    );
    let code = *app.hits.markdown_codes.first().expect("画出了代码块");
    app.on_mouse_move(code.x + 20.0, code.y + 20.0);
    paint(&mut app);
    let (rect, _) = app
        .hits
        .markdown_links
        .iter()
        .find(|(_, url)| url.starts_with(copy))
        .cloned()
        .expect("鼠标在代码块上时有复制按钮");
    super::click(&mut app, (rect.x + 4.0, rect.y + 4.0));
    let copied = app.clipboard.copied.clone().unwrap();
    assert!(copied.starts_with("fn main() {\n"), "{copied:?}");
    assert!(copied.trim_end().ends_with('}'));

    let _ = std::fs::remove_dir_all(&dir);
}

#[test]
fn links_to_headings_in_other_files_open_there() {
    let dir = sample_folder("md-other-file");
    let path = dir.join("README.md");
    std::fs::write(&path, "见 [用法](docs/guide.md#用法)。\n").unwrap();
    let mut guide = String::from("# 指南\n\n");
    for i in 0..80 {
        guide.push_str(&format!("第 {i} 段。\n\n"));
    }
    guide.push_str("## 用法\n\n照着做。\n");
    std::fs::create_dir_all(dir.join("docs")).unwrap();
    std::fs::write(dir.join("docs/guide.md"), guide).unwrap();
    let mut app = app_with(&path, false);
    paint(&mut app);
    app.set_preview_mode(PreviewMode::Preview);
    settle(&mut app);
    let (rect, _) = app.hits.markdown_links[0].clone();
    command_click(&mut app, (rect.x + 2.0, rect.y + 2.0));
    settle(&mut app);
    let doc = app.editor.active().unwrap();
    assert!(doc.path.as_ref().unwrap().ends_with("docs/guide.md"));
    assert_eq!(doc.preview_mode(), PreviewMode::Preview);
    let top = app.preview.markdown.anchor_top("用法").unwrap();
    assert!(top > 0.0);
    assert!(app.preview.markdown.pending_anchor.is_none());
    let max = app.preview.markdown.height - app.preview.markdown.area.h;
    assert_eq!(app.preview.markdown.scroll(), top.min(max));

    let _ = std::fs::remove_dir_all(&dir);
}

#[test]
fn headings_get_github_anchors() {
    use crate::view::preview::markdown::tests::slug;
    assert_eq!(slug("Getting Started!"), "getting-started");
    assert_eq!(slug("安装 winc"), "安装-winc");
    assert_eq!(slug("a_b-c"), "a_b-c");
}

/// 可绘制的 mermaid 图在悬停时显示全屏按钮；不支持的图按代码块显示，只有复制按钮。
#[test]
fn mermaid_blocks_draw_diagrams_and_fall_back_to_code() {
    let dir = sample_folder("md-mermaid-fallback");
    let path = dir.join("M.md");
    let drawn = "```mermaid\ngraph TD\n  A --> B\n```\n";
    let unknown = "```mermaid\njourney\n  title 步骤\n```\n";
    let expand = crate::app::preview::markdown::laid::EXPAND;
    for (source, diagram) in [(drawn, true), (unknown, false)] {
        std::fs::write(&path, source).unwrap();
        let mut app = app_with(&path, false);
        paint(&mut app);
        app.set_preview_mode(PreviewMode::Preview);
        settle(&mut app);
        let [block] = app.hits.markdown_codes[..] else {
            panic!("只有一个代码块或图：{source}");
        };
        app.on_mouse_move(block.x + block.w / 2.0, block.y + block.h / 2.0);
        paint(&mut app);
        let has_expand = app.hits.markdown_links.iter().any(|(_, url)| url == expand);
        assert_eq!(has_expand, diagram, "{source}");
    }
    let _ = std::fs::remove_dir_all(&dir);
}

/// 类图、实体关系图、思维导图、Git 提交图、甘特图都画成图，不按代码块显示。
#[test]
fn more_diagram_kinds_draw_as_diagrams() {
    for source in MORE_DIAGRAMS {
        let (mut app, dir) = live_doc("md-mermaid-more", source);
        app.set_preview_mode(PreviewMode::Preview);
        settle(&mut app);
        let [block] = app.hits.markdown_codes[..] else {
            panic!("只有一张图：{source}");
        };
        app.on_mouse_move(block.x + block.w / 2.0, block.y + block.h / 2.0);
        paint(&mut app);
        let expand = crate::app::preview::markdown::laid::EXPAND;
        assert!(
            app.hits.markdown_links.iter().any(|(_, url)| url == expand),
            "{source}"
        );
        let over = crate::view::preview::markdown::tests::overflow(&app);
        assert!(over <= 0.5, "超出 {over}：{source}");
        let _ = std::fs::remove_dir_all(&dir);
    }
}

/// 全屏查看：图覆盖编辑区并按适应查看区域的倍数显示；工具栏按钮、按键与按住 `⌘` 滚动可以
/// 缩放，拖动可以平移；`Esc`、关闭按钮或切回源代码时关闭，查看期间输入不写入文档。
#[test]
fn diagrams_open_full_screen_and_zoom() {
    use winit::keyboard::{Key, NamedKey};

    use crate::app::preview::markdown::DiagramTool;
    let (mut app, dir) = diagram_view_app("md-diagram-view", 0, false);
    let view = app
        .preview
        .markdown
        .diagram
        .as_ref()
        .expect("打开了全屏查看");
    assert_eq!(view.area, app.layout.editor);
    assert!(view.zoom.is_none());
    let fit = view.scale();
    assert!(fit > 1.0, "小图放大到适应窗口：{fit}");
    let button = |app: &App, tool| {
        let view = app.preview.markdown.diagram.as_ref().unwrap();
        let (r, _) = view.buttons.iter().find(|(_, t)| *t == tool).unwrap();
        (r.x + r.w / 2.0, r.y + r.h / 2.0)
    };
    let scale = |app: &App| app.preview.markdown.diagram.as_ref().unwrap().scale();

    let at = button(&app, DiagramTool::ZoomIn);
    click(&mut app, at);
    paint(&mut app);
    assert!((scale(&app) - fit * 1.25).abs() < 1e-3);
    assert!(app.diagram_view_key(&Key::Character("-".into()), "-", false));
    assert!((scale(&app) - fit).abs() < 1e-3);

    // 按住 `⌘` 滚动：以鼠标处为中心放大，鼠标处的内容位置不变。
    let area = app.preview.markdown.diagram.as_ref().unwrap().area;
    let at = (area.x + area.w / 4.0, area.y + area.h / 3.0);
    app.on_mouse_move(at.0, at.1);
    app.pointer.modifiers = command_modifiers();
    // 滚动量以物理像素给出；离屏窗口的缩放倍数为 2，换算为 100 逻辑像素。
    app.on_wheel(MouseScrollDelta::PixelDelta((0.0, 200.0).into()));
    app.pointer.modifiers = Default::default();
    paint(&mut app);
    let view = app.preview.markdown.diagram.as_ref().unwrap();
    let k = view.scale() / fit;
    assert!(k > 1.5, "{k}");
    let (cx, cy) = (at.0 - area.x - area.w / 2.0, at.1 - area.y - area.h / 2.0);
    assert!((view.pan.0 - (cx - cx * k)).abs() < 0.5);
    assert!((view.pan.1 - (cy - cy * k)).abs() < 0.5);

    // 拖动平移；点击倍数后恢复适应查看区域的倍数，图居中。
    let before = view.pan;
    app.on_mouse_down(MouseButton::Left);
    app.on_mouse_move(at.0 + 30.0, at.1 - 10.0);
    app.on_mouse_up(MouseButton::Left);
    let view = app.preview.markdown.diagram.as_ref().unwrap();
    assert!((view.pan.0 - (before.0 + 30.0)).abs() < 1e-3);
    assert!((view.pan.1 - (before.1 - 10.0)).abs() < 1e-3);
    assert!(view.grab.is_none());
    let at = button(&app, DiagramTool::Fit);
    click(&mut app, at);
    let view = app.preview.markdown.diagram.as_ref().unwrap();
    assert_eq!((view.zoom, view.pan), (None, (0.0, 0.0)));

    // 全屏查看期间的输入不写入被遮住的文档。
    let revision = app.editor.active().unwrap().revision;
    assert!(app.diagram_view_key(&Key::Character("x".into()), "x", false));
    assert_eq!(app.editor.active().unwrap().revision, revision);

    assert!(app.diagram_view_key(&Key::Named(NamedKey::Escape), "", false));
    assert!(app.preview.markdown.diagram.is_none());

    // 关闭按钮关闭全屏查看；切回源代码同样关闭。
    let (mut app, dir2) = diagram_view_app("md-diagram-view-close", 0, false);
    let at = button(&app, DiagramTool::Close);
    click(&mut app, at);
    assert!(app.preview.markdown.diagram.is_none());
    let (mut app, dir3) = diagram_view_app("md-diagram-view-source", 0, false);
    app.set_preview_mode(PreviewMode::Source);
    paint(&mut app);
    assert!(app.preview.markdown.diagram.is_none());
    for dir in [dir, dir2, dir3] {
        let _ = std::fs::remove_dir_all(&dir);
    }
}

#[test]
fn markdown_images_render_one_size_at_a_time() {
    use crate::app::preview::markdown::ImageSlot;

    let mut app = blank_app(false);
    let svg = r#"<svg xmlns="http://www.w3.org/2000/svg" width="10" height="10"/>"#;
    let picture = winc_preview::image::load_svg(svg, None, None).unwrap();
    let path = PathBuf::from("/tmp/winc-shot.svg");
    let slot = ImageSlot::Ready {
        width: picture.width,
        height: picture.height,
        density: 1,
        picture: Some(std::sync::Arc::new(picture)),
        pixels: Vec::new(),
        drawing: Vec::new(),
    };
    app.preview
        .markdown
        .images
        .insert_for_test(path.clone(), slot);
    // 拖动改变窗口大小：每帧需要一个新尺寸，正在绘制时不再提交，只有第一个在后台绘制。
    for width in 100..110 {
        app.request_markdown_pixels(&path, (width, 50));
    }
    let Some(ImageSlot::Ready { drawing, .. }) = app.preview.markdown.images.get(&path) else {
        panic!("图片不在");
    };
    assert_eq!(drawing, &[(100, 50)]);
}

/// 预览左侧有标题指示条，每个标题一条；鼠标停留满延时后标题列表在原处展开，点击一项滚动到该标题。
/// 分栏时正文同样为指示条留出位置。
#[test]
fn the_preview_shows_a_heading_indicator() {
    use crate::app::preview::markdown::TocTarget;

    let (mut app, dir) = readme("md-toc", false, PreviewMode::Preview);
    assert_eq!(app.hits.markdown_toc_tops.len(), 2);
    let find = |app: &App, target| {
        app.hits
            .markdown_toc
            .iter()
            .find(|(_, t)| *t == target)
            .map(|h| h.0)
    };
    let bars = find(&app, TocTarget::Bars).unwrap();
    assert!(find(&app, TocTarget::Heading(0)).is_none());

    // 鼠标刚进入指示条时列表不展开，停留满延时后展开。
    app.on_mouse_move(bars.x + 20.0, bars.y + 10.0);
    paint(&mut app);
    assert!(matches!(app.pointer.hover, Hover::MarkdownToc(_)));
    assert!(find(&app, TocTarget::Heading(0)).is_none());
    rest_on_toc(&mut app, bars.x + 20.0, bars.y + 10.0);
    paint(&mut app);
    assert!(matches!(app.pointer.hover, Hover::MarkdownToc(_)));
    // 展开时第一行的横线落在指示条第一条的位置上（尚未滚动到任何标题），横线不跳动。指示条的
    // 范围比第一条高出 8，第一条粗 2。
    let first = find(&app, TocTarget::Heading(0)).unwrap();
    assert_eq!(first.y + first.h / 2.0, bars.y + 8.0 + 1.0);
    // 列表覆盖指示条展开；鼠标停在列表里两行之间的空白处时列表不收起。
    let row = find(&app, TocTarget::Heading(1)).unwrap();
    let card = app.hits.markdown_toc.iter().rev().nth(1).unwrap().0;
    assert!(card.contains(bars.x + 20.0, bars.y + 10.0));
    app.on_mouse_move(card.x + 1.0, row.y);
    paint(&mut app);
    assert!(find(&app, TocTarget::Heading(1)).is_some());

    click(&mut app, (row.x + 20.0, row.y + row.h / 2.0));
    let max = (app.preview.markdown.height - app.preview.markdown.area.h).max(0.0);
    assert_eq!(
        app.preview.markdown.scroll(),
        app.hits.markdown_toc_tops[1].min(max)
    );
    paint(&mut app);
    assert_eq!(app.preview.markdown.toc.current_heading(), Some(1));

    // 指示条位于正文和原文行号的左侧，不与二者重叠，也不超出预览区域。
    let left = app
        .preview
        .markdown
        .layout
        .as_ref()
        .unwrap()
        .column_bounds()
        .0;
    let area = app.preview.markdown.area;
    // 列表展开着时卡片也记作 `Bars`，指示条本身总是最后一项。
    let bars = app.hits.markdown_toc.last().unwrap().0;
    assert!(
        bars.right() <= left - 8.0 && bars.x >= area.x,
        "{bars:?} {left}"
    );

    // 点击开头的标题后滚动到底：文末的标题无法滚动到顶部，当前所在的一节取屏幕里最后一个标题。
    app.press_toc(TocTarget::Heading(0));
    paint(&mut app);
    assert_eq!(app.preview.markdown.toc.current_heading(), Some(0));
    app.scroll_markdown_to(max);
    paint(&mut app);
    assert_eq!(app.preview.markdown.toc.current_heading(), Some(1));

    // 分栏时预览较窄：正文同样为指示条留出位置，二者不重叠。
    app.set_preview_mode(PreviewMode::Split);
    settle(&mut app);
    let left = app
        .preview
        .markdown
        .layout
        .as_ref()
        .unwrap()
        .column_bounds()
        .0;
    let area = app.preview.markdown.area;
    let bars = app.hits.markdown_toc.last().expect("分栏时也有指示条").0;
    assert!(bars.right() <= left && bars.x >= area.x, "{bars:?} {left}");
    let _ = std::fs::remove_dir_all(&dir);
}

/// 块的开头按删除键、块的末尾按向前删除键：隔着空行的两段正文合并为一段；上一块是代码块时
/// 不合并，光标移到代码的末尾。
#[test]
fn deleting_at_a_block_edge_joins_the_neighbouring_block() {
    use winit::keyboard::{Key, NamedKey};
    let text = "甲\n\n乙\n\n```\nc\n```\n\n丙\n";
    let (mut app, dir) = live_doc("md-join-blocks", text);
    put_caret(&mut app, "乙", false);
    assert!(app.markdown_key(&Key::Named(NamedKey::Backspace), false));
    settle(&mut app);
    assert!(
        with_caret(&app).starts_with("甲|乙\n\n```"),
        "{}",
        with_caret(&app)
    );
    app.run(winc_editor::Command::Undo);
    assert_eq!(doc_text(&app), text);
    put_caret(&mut app, "甲", true);
    assert!(app.markdown_key(&Key::Named(NamedKey::Delete), false));
    settle(&mut app);
    assert!(
        with_caret(&app).starts_with("甲|乙\n\n```"),
        "{}",
        with_caret(&app)
    );
    app.run(winc_editor::Command::Undo);
    put_caret(&mut app, "丙", false);
    assert!(app.markdown_key(&Key::Named(NamedKey::Backspace), false));
    settle(&mut app);
    assert_eq!(with_caret(&app), "甲\n\n乙\n\n```\nc|\n```\n\n丙\n");
    let _ = std::fs::remove_dir_all(&dir);
}

/// 列表记号、标题和引用的标记之后输入空格才生效：只输入 `-`、`#`、`>` 时按文字显示。
#[test]
fn block_markers_take_effect_after_a_space() {
    let (mut app, dir) = live_doc("md-bare-markers", "前文\n\n\n\n后文\n");
    let doc = app.editor.active_mut().unwrap();
    doc.selection = winc_core::Selection::point(4);
    settle(&mut app);
    let shown = |app: &App| app.preview.markdown.full_text().unwrap();
    for marker in ["-", "1.", "#", ">"] {
        app.run(winc_editor::Command::Insert(marker.into()));
        paint(&mut app);
        assert!(
            shown(&app).contains(&format!("前文\n{marker}\n")),
            "{}",
            shown(&app)
        );
        let y = caret_screen_y(&app);
        app.run(winc_editor::Command::Insert(" ".into()));
        paint(&mut app);
        // 成为列表项、引用之后记号不再按文字显示，光标仍在同一行上；标题在光标位于开头时
        // 显示 `#`，字号随之变大。
        if marker != "#" {
            assert!(!shown(&app).contains(marker), "{marker}：{}", shown(&app));
            assert!((caret_screen_y(&app) - y).abs() < 4.0, "{marker}");
        } else {
            let parsed = app.preview.markdown.parsed.clone().unwrap();
            let heading = |b: &winc_preview::markdown::Block| {
                matches!(b, winc_preview::markdown::Block::Heading(1, _))
            };
            assert!(parsed.blocks.iter().any(heading));
        }
        app.run(winc_editor::Command::Undo);
        app.run(winc_editor::Command::Undo);
        settle(&mut app);
    }
    let _ = std::fs::remove_dir_all(&dir);
}

/// Tab、⇧Tab 调整列表项的层级时，子项和续行随该项移动。
#[test]
fn tab_moves_a_list_item_with_its_children() {
    use winit::keyboard::{Key, NamedKey};
    let tab = Key::Named(NamedKey::Tab);
    let text = "- 一\n- 二\n  - 子\n\n    续行\n- 三\n";
    let (mut app, dir) = live_doc("md-tab-subtree", text);
    put_caret(&mut app, "二", true);
    assert!(app.markdown_key(&tab, false));
    settle(&mut app);
    assert_eq!(
        with_caret(&app),
        "- 一\n  - 二|\n    - 子\n\n      续行\n- 三\n"
    );
    app.pointer.modifiers = winit::keyboard::ModifiersState::SHIFT;
    assert!(app.markdown_key(&tab, false));
    app.pointer.modifiers = winit::keyboard::ModifiersState::empty();
    settle(&mut app);
    assert_eq!(with_caret(&app), text.replace("二", "二|"));
    let _ = std::fs::remove_dir_all(&dir);
}

/// 代码块的新建与删除：输入第三个反引号即补上闭围栏；回车进入代码块；空的代码块按一次删除键
/// 整块删除；有内容时在开围栏上删除只移除两行围栏；⌘↩ 在代码块之后另起一段。
#[test]
fn code_blocks_are_created_and_removed_as_a_whole() {
    use winit::keyboard::{Key, NamedKey};
    let tick = Key::Character("`".into());
    let back = Key::Named(NamedKey::Backspace);
    let (mut app, dir) = live_doc("md-code-whole", "前文\n\n\n\n后文\n");
    let doc = app.editor.active_mut().unwrap();
    doc.selection = winc_core::Selection::point(4);
    settle(&mut app);
    for _ in 0..2 {
        assert!(!app.markdown_key(&tick, false));
        app.run(winc_editor::Command::Insert("`".into()));
        paint(&mut app);
    }
    app.preview.markdown.cost = Some(std::time::Duration::MAX);
    assert!(app.markdown_key(&tick, false));
    paint(&mut app);
    assert_eq!(with_caret(&app), "前文\n\n```|\n```\n\n后文\n");
    // 后文没有成为代码。
    let shown = app.preview.markdown.full_text().unwrap();
    assert!(
        shown.ends_with("后文\n") || shown.ends_with("后文"),
        "{shown}"
    );
    assert!(!app.markdown_key(&Key::Named(NamedKey::Enter), false));
    app.run(winc_editor::Command::Newline);
    settle(&mut app);
    assert_eq!(with_caret(&app), "前文\n\n```\n|\n```\n\n后文\n");
    // 有内容：在内容开头按删除键不删除；⌘↩ 到代码块之后。
    app.run(winc_editor::Command::Insert("x".into()));
    app.run(winc_editor::Command::MoveLeft {
        word: false,
        extend: false,
    });
    settle(&mut app);
    assert!(app.markdown_key(&back, false));
    assert_eq!(with_caret(&app), "前文\n\n```\n|x\n```\n\n后文\n");
    app.pointer.modifiers = command_modifiers();
    assert!(app.markdown_key(&Key::Named(NamedKey::Enter), true));
    app.pointer.modifiers = winit::keyboard::ModifiersState::empty();
    settle(&mut app);
    assert_eq!(with_caret(&app), "前文\n\n```\nx\n```\n\n|\n\n后文\n");
    app.run(winc_editor::Command::Undo);
    // 在开围栏上删除：移除两行围栏，保留内容。
    put_caret(&mut app, "```", true);
    assert!(app.markdown_key(&back, false));
    settle(&mut app);
    assert_eq!(with_caret(&app), "前文\n\n|x\n\n后文\n");
    app.run(winc_editor::Command::Undo);
    // 空的代码块：按一次删除键整块删除。
    put_caret(&mut app, "x\n", false);
    app.run(winc_editor::Command::Delete { word: false });
    settle(&mut app);
    assert!(app.markdown_key(&back, false));
    settle(&mut app);
    assert_eq!(with_caret(&app), "前文\n\n|\n\n后文\n");
    let _ = std::fs::remove_dir_all(&dir);
}

#[test]
fn leaving_code_accepts_a_closer_at_least_as_long_as_its_fence() {
    use winc_editor::Command;
    use winit::keyboard::{Key, NamedKey};
    for (name, source, expected) in [
        (
            "long-backticks",
            "````md\n```\n内容\n````",
            "````md\n```\n内容\n````\n\n下文",
        ),
        (
            "longer-tildes-crlf",
            "~~~text\r\n内容\r\n~~~~~",
            "~~~text\r\n内容\r\n~~~~~\r\n\r\n下文",
        ),
        (
            "quoted",
            "> ````text\n> 内容\n> `````",
            "> ````text\n> 内容\n> `````\n\n下文",
        ),
    ] {
        let (mut app, dir) = live_doc(&format!("md-fence-leave-{name}"), source);
        put_caret(&mut app, "内容", true);
        assert!(
            app.markdown_key(&Key::Named(NamedKey::Enter), true),
            "{name}"
        );
        app.run(Command::Insert("下文".into()));
        settle(&mut app);
        assert_eq!(doc_text(&app), expected);
        use winc_preview::markdown::Block;
        let parsed = app.preview.markdown.parsed.as_ref().unwrap();
        assert!(matches!(parsed.blocks.last(), Some(Block::Paragraph(_))));
        std::fs::remove_dir_all(dir).unwrap();
    }
}

#[test]
fn a_false_closing_fence_cannot_be_used_to_leave_open_code() {
    use winit::keyboard::{Key, NamedKey};
    for (name, source) in [
        ("too-short", "````md\n内容\n```"),
        ("trailing-text", "```text\n内容\n```>"),
    ] {
        let (mut app, dir) = live_doc(&format!("md-open-fence-{name}"), source);
        put_caret(&mut app, "内容", true);
        assert!(
            !app.markdown_key(&Key::Named(NamedKey::Enter), true),
            "{name}"
        );
        app.run(winc_editor::Command::DocEnd { extend: false });
        settle(&mut app);
        assert!(app.markdown_key(&Key::Named(NamedKey::ArrowDown), false));
        assert_eq!(doc_text(&app), source, "{name}");
        std::fs::remove_dir_all(dir).unwrap();
    }
}

#[test]
fn backspace_from_prose_after_a_wide_fence_keeps_the_delimiter_intact() {
    use winit::keyboard::{Key, NamedKey};
    let text = "````text\r\n原文\r\n`````\r\n\r\n下文";
    let (mut app, dir) = live_doc("md-join-wide-fence", text);
    put_caret(&mut app, "下文", false);
    assert!(app.markdown_key(&Key::Named(NamedKey::Backspace), false));
    assert_eq!(with_caret(&app), "````text\r\n原文|\r\n`````\r\n\r\n下文");
    std::fs::remove_dir_all(dir).unwrap();
}

#[test]
fn a_fence_with_a_literal_quote_marker_is_not_empty() {
    use winit::keyboard::{Key, NamedKey};
    for (name, source) in [
        ("standalone", "```\n>\n```"),
        ("quoted", "> ```\n> >\n> ```"),
    ] {
        let (mut app, dir) = live_doc(&format!("md-fence-literal-greater-{name}"), source);
        app.run(winc_editor::Command::DocEnd { extend: false });
        settle(&mut app);
        assert!(app.markdown_key(&Key::Named(NamedKey::Backspace), false));
        assert_eq!(doc_text(&app), source);
        assert!(with_caret(&app).contains(">|\n"));
        std::fs::remove_dir_all(dir).unwrap();
    }
}

/// 文档以代码块或公式结束时再按 ↓：在其后另起一段，光标移到该段。
#[test]
fn down_after_a_trailing_code_block_or_formula_adds_a_paragraph() {
    use winit::keyboard::{Key, NamedKey};
    for (name, source) in [
        ("code", "前文\n\n```\nx\n```"),
        ("inline-fence", "前文\n\n$$x$$"),
    ] {
        let (mut app, dir) = live_doc(&format!("md-trailing-{name}"), source);
        put_caret(&mut app, "x", true);
        assert!(app.markdown_key(&Key::Named(NamedKey::ArrowDown), false));
        settle(&mut app);
        assert_eq!(with_caret(&app), format!("{source}\n\n|"));
        std::fs::remove_dir_all(dir).unwrap();
    }
}

/// 滚动预览时，悬停展开的标题列表和钉住的列表位置不变。
#[test]
fn the_heading_list_stays_put_while_the_preview_scrolls() {
    use crate::app::preview::markdown::TocTarget;

    let dir = sample_folder("md-toc-still");
    let path = dir.join("LONG.md");
    let text: String = (1..=8)
        .map(|i| format!("## 第 {i} 节\n\n{}\n\n", "正文。\n\n".repeat(30)))
        .collect();
    std::fs::write(&path, text).unwrap();
    let mut app = app_with(&path, false);
    paint(&mut app);
    app.set_preview_mode(PreviewMode::Preview);
    settle(&mut app);
    // 列表展开时，卡片是最宽的 `Bars` 区域。
    let card = |app: &App| {
        (app.hits.markdown_toc.iter())
            .filter(|(_, t)| *t == TocTarget::Bars)
            .map(|(rect, _)| *rect)
            .max_by(|a, b| a.w.total_cmp(&b.w))
            .unwrap()
    };
    let scroll_to = |app: &mut App, heading: usize| {
        let top = app.hits.markdown_toc_tops[heading];
        app.scroll_markdown_to(top);
        paint(app);
        assert_eq!(app.preview.markdown.toc.current_heading(), Some(heading));
    };

    let bars = card(&app);
    rest_on_toc(&mut app, bars.x + 10.0, bars.y + 10.0);
    paint(&mut app);
    let opened = card(&app);
    assert!(opened.w > bars.w);
    for heading in [3, 6, 1] {
        scroll_to(&mut app, heading);
        assert_eq!(card(&app), opened, "{heading}");
    }
    // 收起后再次展开：位置按当时所在的一节重新确定。
    let area = app.preview.markdown.area;
    app.on_mouse_move(area.x + area.w / 2.0, area.y + area.h / 2.0);
    paint(&mut app);
    assert_eq!(app.preview.markdown.toc.floating_top(), None);

    app.state.markdown_toc_pinned = true;
    settle(&mut app);
    let docked = card(&app);
    for heading in [5, 0, 7] {
        scroll_to(&mut app, heading);
        assert_eq!(card(&app), docked, "{heading}");
    }
    let _ = std::fs::remove_dir_all(&dir);
}

/// 将真实场景样本 `testdata/markdown/epidemic-modeling/` 连同本地图片只读复制到隔离临时目录。
fn copy_epidemic_modeling(name: &str) -> (PathBuf, PathBuf) {
    let src_dir = std::path::Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("../../testdata/markdown/epidemic-modeling");
    let dir = sample_folder(name);
    let img_dir = dir.join("images");
    std::fs::create_dir_all(&img_dir).unwrap();
    let index = dir.join("index.md");
    std::fs::copy(src_dir.join("index.md"), &index).unwrap();
    for file in [
        "sir-curves.svg",
        "herd-immunity-threshold.svg",
        "attack-rate-heatmap.png",
    ] {
        std::fs::copy(src_dir.join("images").join(file), img_dir.join(file)).unwrap();
    }
    (dir, index)
}

/// 打开真实场景样本并等待全部解析与本地图片解码、按屏绘制就绪。
fn open_epidemic_modeling(path: &Path, dark: bool) -> App {
    let mut app = app_with(path, dark);
    paint(&mut app);
    app.set_preview_mode(PreviewMode::Preview);
    settle(&mut app);
    app
}

/// 采样滚动位置 `y` 处的一帧离屏像素，等待当前视口中按需请求的图片解码绘制后再取帧。
fn render_viewport(app: &mut App, y: f32) -> (u32, u32, Vec<u32>) {
    app.scroll_markdown_to(y);
    settle(app);
    let image = app.frame.renderer.as_ref().unwrap().image().unwrap();
    (image.width, image.height, image.pixels.clone())
}

/// 统计当前帧中与预览背景色不同的像素数量，确认画面并非空白。
fn painted_pixels(pixels: &[u32], bg: Color) -> usize {
    let ch = |v: f32| (v.clamp(0.0, 1.0) * 255.0).round() as u32;
    let bg_argb = 0xff00_0000 | (ch(bg.r) << 16) | (ch(bg.g) << 8) | ch(bg.b);
    pixels.iter().filter(|&&px| px != bg_argb).count()
}

/// 逐像素比对两帧离屏缓冲，发现差异时仅报告差异像素数与首个差异坐标（不倾倒整帧像素）。
fn assert_frames_equal(a: &(u32, u32, Vec<u32>), b: &(u32, u32, Vec<u32>), context: &str) {
    assert_eq!((a.0, a.1), (b.0, b.1), "{context}: 帧尺寸不一致");
    if a.2 == b.2 {
        return;
    }
    let w = a.0 as usize;
    let mut diffs = 0usize;
    let mut first = None;
    for (idx, (&pa, &pb)) in a.2.iter().zip(&b.2).enumerate() {
        if pa != pb {
            diffs += 1;
            if first.is_none() {
                first = Some((idx % w, idx / w, pa, pb));
            }
        }
    }
    let (x, y, pa, pb) = first.unwrap();
    panic!(
        "{context}: 发现 {diffs} 个差异像素，首处位于物理像素 ({x}, {y}) got={pa:#010x} expected={pb:#010x}"
    );
}

/// 逐屏扫过整篇文档，触发全部延迟加载的本地图片解码并让总高度与缓存稳定。
fn warm_all_images(app: &mut App) {
    let step = (app.preview.markdown.area.h * 0.85).max(300.0);
    let mut y = 0.0_f32;
    loop {
        let max = app.preview.markdown.max_scroll();
        app.scroll_markdown_to(y.min(max));
        settle(app);
        if y >= max {
            break;
        }
        y += step;
    }
    app.scroll_markdown_to(0.0);
    settle(app);
}

/// 真实场景样本（`testdata/markdown/epidemic-modeling/index.md`）的渲染一致性校验：
/// 固定尺寸与深浅主题下图片全部就绪、重复渲染逐像素一致、滚动卸载后回滚逐像素一致、编辑后撤销无损还原。
#[test]
fn epidemic_modeling_renders_consistently_across_themes_and_cache_cycles() {
    let (dir, index) = copy_epidemic_modeling("md-epidemic-consistency");
    let mut theme_top_frames: Vec<Vec<u32>> = Vec::new();

    for dark in [false, true] {
        let label = if dark { "dark" } else { "light" };
        let mut app = open_epidemic_modeling(&index, dark);
        // 先完整扫过全篇让所有本地图片解码并稳定整篇文档高度（避免首次采集前后因图片加载导致滚动条长度变化）。
        warm_all_images(&mut app);
        let height = app.preview.markdown.height;
        let max_scroll = app.preview.markdown.max_scroll();
        assert!(height > 5000.0, "[{label}] 样稿排版高度过小: {height}");

        // 1. 确认 3 张本地图片全部解码成功（无 Failed / Loading），且尺寸有效。
        let slots = app.preview.markdown.images.as_typeset_map();
        for name in [
            "sir-curves.svg",
            "herd-immunity-threshold.svg",
            "attack-rate-heatmap.png",
        ] {
            let slot = slots
                .iter()
                .find(|(p, _)| p.file_name().and_then(|n| n.to_str()) == Some(name))
                .map(|(_, s)| s)
                .unwrap_or_else(|| panic!("[{label}] 未加载本地图片: {name}"));
            match slot {
                crate::app::preview::markdown::ImageSlot::Ready { width, height, .. } => {
                    assert!(
                        *width > 100 && *height > 100,
                        "[{label}] 图片 {name} 尺寸异常: {width}x{height}"
                    );
                }
                crate::app::preview::markdown::ImageSlot::Loading => {
                    panic!("[{label}] 图片 {name} 仍在加载中");
                }
                crate::app::preview::markdown::ImageSlot::Failed => {
                    panic!("[{label}] 图片 {name} 解码失败");
                }
            }
        }

        // 选取 4 个固定视口位置：文首（属性与目录）、上中段（公式与 SVG 曲线）、
        // 下中段（表格与 PNG 热图）、文末（Mermaid 类图/ER/Git 图与脚注）。
        let checkpoints = [
            0.0,
            (max_scroll * 0.28).round(),
            (max_scroll * 0.52).round(),
            max_scroll.round(),
        ];

        // 2. 采集 4 个检查点的像素基线，同时确认每屏都有真实绘制像素。
        let bg = app.palette.editor_bg;
        let mut baseline = Vec::with_capacity(checkpoints.len());
        for &y in &checkpoints {
            let frame = render_viewport(&mut app, y);
            assert!(
                painted_pixels(&frame.2, bg) > 10_000,
                "[{label}] 视口 y={y} 绘制像素异常偏少"
            );
            baseline.push(frame);
        }
        theme_top_frames.push(baseline[0].2.clone());

        // 3. 完整滚动走完缓存卸载周期后回到 4 个检查点，断言逐像素完全一致。
        let step = (app.preview.markdown.area.h * 0.9).max(300.0);
        let mut y = 0.0_f32;
        while y < max_scroll {
            app.scroll_markdown_to(y);
            paint(&mut app);
            y += step;
        }
        for (idx, &cp) in checkpoints.iter().enumerate() {
            let after = render_viewport(&mut app, cp);
            assert_frames_equal(
                &after,
                &baseline[idx],
                &format!("[{label}] 滚动周期后视口 #{idx} (y={cp})"),
            );
        }

        // 4. 全新独立实例在同尺寸与同主题下渲染，断言与首个实例逐像素完全一致。
        let mut fresh = open_epidemic_modeling(&index, dark);
        warm_all_images(&mut fresh);
        for (idx, &cp) in checkpoints.iter().enumerate() {
            let other = render_viewport(&mut fresh, cp);
            assert_frames_equal(
                &other,
                &baseline[idx],
                &format!("[{label}] 独立重开渲染视口 #{idx} (y={cp})"),
            );
        }

        // 5. 浅色主题下验证实时编辑 + 撤销往返后，文本、排版高度与离屏帧像素完全一致。
        if !dark {
            let original = doc_text(&fresh);
            let before_top = render_viewport(&mut fresh, 0.0);
            let before_height = fresh.preview.markdown.height;
            put_caret(&mut fresh, "仓室模型", true);
            fresh.run(winc_editor::Command::Insert("（测试修改）".into()));
            settle(&mut fresh);
            assert_ne!(doc_text(&fresh), original);
            fresh.run(winc_editor::Command::Undo);
            settle(&mut fresh);
            assert_eq!(doc_text(&fresh), original, "撤销后原文应逐字节还原");
            assert!(
                (fresh.preview.markdown.height - before_height).abs() < 0.5,
                "撤销后总高度漂移: before={before_height}, after={}",
                fresh.preview.markdown.height
            );
            // 退出实时编辑光标显示后对比文首像素，确认与编辑前一致。
            fresh.preview.markdown.live.leave_mode();
            let after_top = render_viewport(&mut fresh, 0.0);
            assert_frames_equal(&after_top, &before_top, "编辑并撤销后文首像素");
        }
    }

    assert!(
        theme_top_frames[0] != theme_top_frames[1],
        "浅色与深色主题的渲染输出不应相同"
    );
    let _ = std::fs::remove_dir_all(&dir);
}

#[test]
fn extreme_markdown_renders_and_scrolls_consistently_offscreen() {
    let mut md = String::from("# 极端样本离屏一致性\n\n");
    for i in 0..120 {
        md.push_str(&format!(
            "超长段落第{i}段含**粗体**、`code_{i}`与[链接](https://example.com/{i})；"
        ));
    }
    md.push_str("\n\n");
    for c in 0..12 {
        md.push_str(&format!("| 列{c} "));
    }
    md.push_str("|\n");
    for _ in 0..12 {
        md.push_str("| --- ");
    }
    md.push_str("|\n");
    for r in 0..40 {
        for c in 0..12 {
            md.push_str(&format!("| R{r}C{c} "));
        }
        md.push_str("|\n");
    }
    md.push('\n');
    for d in 1..=10 {
        md.push_str(&format!("{} 第{d}层引用\n", ">".repeat(d)));
    }
    md.push('\n');
    for i in 1..=16 {
        md.push_str(&format!(
            "$$\n\\sum_{{k=1}}^{{{i}}} \\frac{{1}}{{k^2}} = \\int_0^1 t^{{{i}}}\\,dt\n$$\n\n"
        ));
    }

    let (mut app, dir) = live_doc("md-extreme", &md);
    warm_all_images(&mut app);
    assert!(
        app.preview.markdown.height > app.preview.markdown.area.h * 2.0,
        "极端样本总排版高度应超过两屏"
    );

    let max_scroll = app.preview.markdown.max_scroll();
    let top_before = render_viewport(&mut app, 0.0);
    let _mid = render_viewport(&mut app, max_scroll * 0.5);
    let _bottom = render_viewport(&mut app, max_scroll);
    let top_after = render_viewport(&mut app, 0.0);
    assert_frames_equal(&top_after, &top_before, "极端样本滚回文首像素应完全一致");
    let _ = std::fs::remove_dir_all(&dir);
}

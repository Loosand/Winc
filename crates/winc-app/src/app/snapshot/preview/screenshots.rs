//! 手动生成的 Markdown 预览截图。

use super::*;

/// 截图：分栏（左侧源代码、右侧预览）和只看预览，浅色、深色各一张。
#[test]
#[ignore = "截图，手动运行"]
fn shoot_markdown_preview() {
    for dark in [false, true] {
        let theme = if dark { "dark" } else { "light" };
        for (mode, name) in [
            (PreviewMode::Split, "split"),
            (PreviewMode::Preview, "preview"),
        ] {
            let name = format!("md-{name}-{theme}");
            let (mut app, dir) = readme(&name, dark, mode);
            shoot(&mut app, &name);
            if mode == PreviewMode::Preview {
                let area = app.preview.markdown.area;
                app.pointer.mouse = (area.x + 50.0, area.y + 50.0);
                app.on_wheel(MouseScrollDelta::PixelDelta(PhysicalPosition::new(
                    0.0, -700.0,
                )));
                settle(&mut app);
                shoot(&mut app, &format!("{name}-scrolled"));
                // 选中一段、查找「预览」。
                app.markdown_select_all();
                app.preview.markdown.selection = Some((10, 40));
                app.open_search(false);
                app.bars.find.input.set_text("预览");
                app.bars.search.query = "预览".into();
                app.search_changed();
                settle(&mut app);
                shoot(&mut app, &format!("{name}-find"));
            }
            let _ = std::fs::remove_dir_all(&dir);
        }
    }
}

#[test]
#[ignore = "截图，手动运行"]
fn shoot_rich_markdown() {
    for dark in [false, true] {
        let dir = sample_folder("md-rich");
        let path = dir.join("RICH.md");
        std::fs::write(&path, RICH).unwrap();
        let mut app = app_with(&path, dark);
        paint(&mut app);
        app.set_preview_mode(PreviewMode::Preview);
        settle(&mut app);
        shoot(
            &mut app,
            &format!("md-rich-{}", if dark { "dark" } else { "light" }),
        );
        let _ = std::fs::remove_dir_all(&dir);
    }
}

#[test]
#[ignore = "截图，手动运行"]
fn shoot_more_mermaid_diagrams() {
    for dark in [false, true] {
        for (k, source) in MORE_DIAGRAMS.iter().enumerate() {
            let dir = sample_folder("md-mermaid-more");
            let path = dir.join("MERMAID.md");
            std::fs::write(&path, source).unwrap();
            let mut app = app_with(&path, dark);
            paint(&mut app);
            app.set_preview_mode(PreviewMode::Preview);
            settle(&mut app);
            let theme = if dark { "dark" } else { "light" };
            shoot(&mut app, &format!("md-mermaid-more-{k}-{theme}"));
            let _ = std::fs::remove_dir_all(&dir);
        }
    }
}

#[test]
#[ignore = "截图，手动运行"]
fn shoot_mermaid_diagrams() {
    for dark in [false, true] {
        for (k, source) in DIAGRAMS.iter().enumerate() {
            let dir = sample_folder("md-mermaid");
            let path = dir.join("MERMAID.md");
            std::fs::write(&path, source).unwrap();
            let mut app = app_with(&path, dark);
            paint(&mut app);
            app.set_preview_mode(PreviewMode::Preview);
            settle(&mut app);
            let theme = if dark { "dark" } else { "light" };
            shoot(&mut app, &format!("md-mermaid-{k}-{theme}"));
            let _ = std::fs::remove_dir_all(&dir);
        }
    }
}

#[test]
#[ignore = "截图，手动运行"]
fn shoot_diagram_view() {
    for dark in [false, true] {
        let (mut app, dir) = diagram_view_app("md-diagram-shot", 3, dark);
        let theme = if dark { "dark" } else { "light" };
        shoot(&mut app, &format!("md-diagram-view-{theme}"));
        // 关闭全屏查看后鼠标仍在图上，截取图右上角的全屏、复制按钮。
        app.close_diagram_view();
        let diagram = *app.hits.markdown_codes.first().unwrap();
        app.on_mouse_move(diagram.right() - 40.0, diagram.y + 20.0);
        shoot(&mut app, &format!("md-diagram-buttons-{theme}"));
        let _ = std::fs::remove_dir_all(&dir);
    }
}

/// 截图：可编辑的预览中光标位于粗体和链接中，只显示光标接触到的标记。
#[test]
#[ignore = "截图，手动运行"]
fn shoot_markdown_reveal() {
    let (mut app, dir) = readme("md-reveal-shot", false, PreviewMode::Preview);
    for (needle, name) in [
        ("Rust", "bold"),
        ("GitHub", "link"),
        ("winc 编辑器", "heading"),
        ("cargo xtask", "code"),
    ] {
        put_caret(&mut app, needle, false);
        app.run(winc_editor::Command::MoveRight {
            extend: false,
            word: false,
        });
        app.run(winc_editor::Command::MoveLeft {
            extend: false,
            word: false,
        });
        settle(&mut app);
        shoot(&mut app, &format!("md-reveal-{name}"));
    }
    // 标题中的输入法组字使用标题的字号和粗体。
    put_caret(&mut app, "编辑器", false);
    app.ime.preedit = "中文".into();
    shoot(&mut app, "md-reveal-preedit");
    let _ = std::fs::remove_dir_all(&dir);
}

/// 截图：可编辑的预览中光标分别位于代码、mermaid、公式、HTML、图片、表格中。
#[test]
#[ignore = "截图，手动运行"]
fn shoot_markdown_blocks() {
    let text = "# 块\n\n```rust\nfn main() {}\n```\n\n```mermaid\ngraph TD\n  A --> B\n```\n\n\
                $$\nx^2 + y^2\n$$\n\n<div align=\"center\">\n<b>粗</b> 和 <i>斜</i>\n</div>\n\n\
                ![标志](logo.svg)\n\n| 名 | 值 |\n|---|---|\n| **甲** | `1` |\n| 乙 | 2 |\n\n尾\n";
    let (mut app, dir) = live_doc("md-blocks-shot", text);
    std::fs::write(dir.join("logo.svg"), LOGO).unwrap();
    settle(&mut app);
    for (needle, name) in [
        ("fn main", "code"),
        ("```rust", "code-fence"),
        ("A -->", "mermaid"),
        ("x^2", "math"),
        ("<b>", "html"),
        ("标志", "image"),
        ("甲", "table"),
    ] {
        put_caret(&mut app, needle, false);
        shoot(&mut app, &format!("md-block-{name}"));
    }
    // 删除表格的一行，过渡进行到一半：后文正在向上滑动。
    app.preview.markdown.frozen = Some(crate::app::animation::MARKDOWN_MOTION / 3);
    put_caret(&mut app, "乙", false);
    app.live_table_tool("delete-row");
    settle(&mut app);
    shoot(&mut app, "md-block-table-motion");
    let _ = std::fs::remove_dir_all(&dir);
}

/// 截图：本仓库的 README 只看预览，滚动到第三个标题，鼠标停在标题指示条上；点击最后一个
/// 标题后正文中的底色。深浅两套配色。
#[test]
#[ignore]
fn shoot_preview_heading_indicator() {
    use crate::app::preview::markdown::TocTarget;

    for dark in [false, true] {
        let dir = sample_folder(if dark { "md-toc-dark" } else { "md-toc-light" });
        let path = dir.join("README.md");
        std::fs::write(&path, include_str!("../../../../../../README.md")).unwrap();
        let mut app = app_with(&path, dark);
        paint(&mut app);
        app.set_preview_mode(PreviewMode::Preview);
        settle(&mut app);
        let top = app.hits.markdown_toc_tops[2];
        app.scroll_markdown_to(top);
        paint(&mut app);
        shoot(&mut app, if dark { "md-toc-dark" } else { "md-toc-light" });
        let bars = app
            .hits
            .markdown_toc
            .iter()
            .find(|(_, t)| *t == TocTarget::Bars)
            .unwrap()
            .0;
        rest_on_toc(&mut app, bars.x + 20.0, bars.y + 10.0);
        paint(&mut app);
        let open = if dark {
            "md-toc-open-dark"
        } else {
            "md-toc-open-light"
        };
        shoot(&mut app, open);
        // 展开到一半：横线正从指示条滑向各行，卡片和文字淡入到一半。
        let at = crate::app::animation::TOC_FADE;
        app.preview.markdown.toc_fade = Some(crate::app::animation::Clock::frozen(at / 2, at));
        let half = if dark {
            "md-toc-opening-dark"
        } else {
            "md-toc-opening-light"
        };
        shoot(&mut app, half);
        app.preview.markdown.toc_fade = None;
        // 点击最后一个标题：滚动到底，它成为当前所在的一节，正文里的底色淡出到三成。
        let last = app.hits.markdown_toc_tops.len() - 1;
        app.press_toc(TocTarget::Heading(last));
        let at = crate::app::animation::TOC_FLASH;
        let clock = crate::app::animation::Clock::frozen(at * 3 / 10, at);
        app.preview.markdown.toc_flash = Some((last, clock));
        app.on_mouse_move(900.0, 400.0);
        paint(&mut app);
        let picked = if dark {
            "md-toc-picked-dark"
        } else {
            "md-toc-picked-light"
        };
        shoot(&mut app, picked);
        let _ = std::fs::remove_dir_all(&dir);
    }
}

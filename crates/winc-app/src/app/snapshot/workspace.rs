//! 打开的目录与文件的测试：快速打开、全局搜索、侧栏与文件树、拖放、标签栏、现场的保存与
//! 恢复、目录监视。

use super::*;

pub(super) mod folders;
mod library;
pub(super) mod loading;
mod note_search;
mod notebook;
pub(super) mod outline;
mod panes;
pub(super) mod picker;
pub(super) mod recent;
pub(super) mod search;
pub(super) mod selection;
pub(super) mod session;
pub(super) mod tab_selection;
pub(super) mod tabs;
pub(super) mod tree;
mod tree_drag;
pub(super) mod watch;

fn note_app(name: &str) -> (App, PathBuf) {
    let dir = sample_folder(name);
    let root = dir.join("Notebook");
    std::fs::create_dir_all(root.join("Recipes")).unwrap();
    std::fs::write(
        root.join("Recipes/茶.md"),
        "# 茶\n\n## 冲泡\n\n一片薄荷。\n\n### 时间\n\n约两分钟。\n",
    )
    .unwrap();
    std::fs::write(root.join("薄荷.md"), "# 薄荷\n\n盆栽。\n").unwrap();
    std::fs::write(root.join(".hidden.md"), "# 薄荷\n").unwrap();
    std::fs::write(root.join("config.txt"), "薄荷").unwrap();
    let mut app = blank_app(false);
    app.panels.sidebar.legacy_chrome = false;
    App::apply_preview_state(&mut app.editor, &app.state);
    app.open_path(&root);
    let root = app.library_root().unwrap().to_path_buf();
    (app, root)
}

fn press(app: &mut App, r: Rect) {
    assert!(r.h > 0.0 && r.w > 0.0);
    click(app, (r.x + r.w / 2.0, r.y + r.h / 2.0));
    paint(app);
}

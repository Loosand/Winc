//! Actions for the backlinks footer; locations are revalidated against the opened buffer.

use super::*;
use winc_i18n::t;

pub(crate) const CONTROL: &str = "\u{1}note-link:";

#[derive(Clone)]
pub(crate) enum Action {
    Open(PathBuf),
    Cite(PathBuf, Citation),
    Expand(PathBuf),
    Graph,
}

impl App {
    pub(in crate::app) fn press_note_footer(&mut self, url: &str) -> bool {
        let Some(index) = url.strip_prefix(CONTROL) else {
            return false;
        };
        let Some(action) = index
            .parse::<usize>()
            .ok()
            .and_then(|i| self.note_links.footer_hits.get(i).cloned())
        else {
            return true;
        };
        let Some(target) = self.editor.active().and_then(|doc| doc.path.clone()) else {
            return true;
        };
        match action {
            Action::Open(path) => self.open_path(&path),
            Action::Cite(path, citation) => self.open_path_then(&path, move |app| {
                if let Some((ticket, range)) = app.editor.active().and_then(|doc| {
                    let rope = doc.text().rope()?;
                    let range = locate(&rope.to_string(), &citation)?;
                    let chars = rope.byte_to_char(range.start)..rope.byte_to_char(range.end);
                    Some(((doc.id, doc.revision, range.start), chars))
                }) {
                    if app.markdown_live() {
                        app.start_live_editing();
                        app.note_links.citation_jump = Some(ticket);
                    }
                    app.select(range.start, range.end);
                } else {
                    app.flash(t!("workspace-note-citation-changed"));
                }
            }),
            Action::Expand(path) => {
                let sources = self.note_links.expanded.entry(target).or_default();
                if !sources.remove(&path) {
                    sources.insert(path);
                }
                self.request_redraw();
            }
            Action::Graph => self.open_note_graph(Some(target)),
        }
        true
    }
}

fn locate(text: &str, citation: &Citation) -> Option<Range<usize>> {
    let mut matches = citations(text).filter(|link| {
        link.syntax == citation.syntax
            && link.target == citation.target
            && link.label == citation.label
            && link.before == citation.before
            && link.after == citation.after
    });
    let first = matches.next()?;
    if first.range == citation.range {
        return Some(first.range);
    }
    let Some(second) = matches.next() else {
        return Some(first.range);
    };
    // If several identical excerpts moved, only an exact range match is safe.
    std::iter::once(second)
        .chain(matches)
        .find_map(|link| (link.range == citation.range).then_some(link.range))
}

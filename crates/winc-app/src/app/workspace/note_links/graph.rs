//! The relationship graph projects the same resolved note-to-note edges as backlinks.

use std::collections::VecDeque;

use winc_editor::Reading;
use winc_i18n::t;
use winit::event::MouseScrollDelta;
use winit::keyboard::{Key, NamedKey};

use super::*;

#[derive(Default)]
pub(crate) struct Graph {
    pub(crate) paths: Vec<PathBuf>,
    pub(crate) points: Vec<(f32, f32)>,
    pub(crate) edges: Vec<(usize, usize)>,
    pub(crate) neighbors: Vec<Vec<usize>>,
}

impl Graph {
    pub(super) fn from_notes(index: &Index, previous: Option<&Arc<Self>>) -> Arc<Self> {
        let paths: Vec<_> = index.paths().cloned().collect();
        let ids: BTreeMap<_, _> = paths
            .iter()
            .enumerate()
            .map(|(i, path)| (path, i))
            .collect();
        let edges = index
            .incoming
            .iter()
            .flat_map(|(target, sources)| sources.keys().map(|source| (ids[source], ids[target])))
            .collect();
        if let Some(previous) =
            previous.filter(|graph| graph.paths == paths && graph.edges == edges)
        {
            return Arc::clone(previous);
        }
        let mut graph = Self::new(paths, edges);
        graph.place_components();
        Arc::new(graph)
    }

    fn new(paths: Vec<PathBuf>, edges: Vec<(usize, usize)>) -> Self {
        let mut neighbors = vec![Vec::new(); paths.len()];
        for &(a, b) in &edges {
            neighbors[a].push(b);
            neighbors[b].push(a);
        }
        for adjacent in &mut neighbors {
            adjacent.sort_unstable();
            adjacent.dedup();
        }
        Self {
            points: vec![(0.0, 0.0); paths.len()],
            paths,
            edges,
            neighbors,
        }
    }

    fn local(&self, path: &Path) -> Self {
        let Ok(center) = self.paths.binary_search_by(|p| p.as_path().cmp(path)) else {
            return Self::default();
        };
        let ids: BTreeSet<_> = self.neighbors[center]
            .iter()
            .copied()
            .chain([center])
            .collect();
        let new: BTreeMap<_, _> = ids.iter().enumerate().map(|(i, &old)| (old, i)).collect();
        let paths = ids.iter().map(|&old| self.paths[old].clone()).collect();
        let edges = self
            .edges
            .iter()
            .filter_map(|(a, b)| Some((*new.get(a)?, *new.get(b)?)))
            .collect();
        let mut graph = Self::new(paths, edges);
        graph.place_ring(
            new[&center],
            &self.neighbors[center]
                .iter()
                .map(|i| new[i])
                .collect::<Vec<_>>(),
            (0.0, 0.0),
        );
        graph
    }

    fn place_ring(&mut self, center: usize, ring: &[usize], (x, y): (f32, f32)) {
        self.points[center] = (x, y);
        let mut start = 0;
        for row in 1.. {
            if start == ring.len() {
                break;
            }
            let end = (start + 8 * row).min(ring.len());
            let radius = row as f32 * 92.0;
            for (i, &node) in ring[start..end].iter().enumerate() {
                let theta = std::f32::consts::TAU * i as f32 / (end - start) as f32
                    - std::f32::consts::FRAC_PI_2;
                self.points[node] = (x + radius * theta.cos(), y + radius * theta.sin());
            }
            start = end;
        }
    }

    fn place_components(&mut self) {
        let mut seen = vec![false; self.paths.len()];
        let mut components = Vec::new();
        for start in 0..self.paths.len() {
            if seen[start] || self.neighbors[start].is_empty() {
                continue;
            }
            seen[start] = true;
            let mut queue = VecDeque::from([start]);
            let mut component = Vec::new();
            while let Some(at) = queue.pop_front() {
                component.push(at);
                for &next in &self.neighbors[at] {
                    if !seen[next] {
                        seen[next] = true;
                        queue.push_back(next);
                    }
                }
            }
            components.push(component);
        }
        components.sort_by_key(|c| std::cmp::Reverse(c.len()));
        let scale = (self.paths.len() as f32).sqrt().max(3.0) * 190.0;
        let (mut x, mut y, mut row): (f32, f32, f32) = (0.0, 0.0, 0.0);
        for component in components {
            let n = component.len() as f32;
            let size = ((n / 4.0).sqrt().ceil() * 184.0 + 136.0).min(scale);
            if x > 0.0 && x + size > scale {
                x = 0.0;
                y += row;
                row = 0.0;
            }
            let center = component
                .iter()
                .copied()
                .max_by_key(|&i| self.neighbors[i].len())
                .unwrap_or(component[0]);
            let ring: Vec<_> = component.into_iter().filter(|&i| i != center).collect();
            self.place_ring(center, &ring, (x + size / 2.0, y + size / 2.0));
            x += size;
            row = row.max(size);
        }
        y += row;
        x = 0.0;
        for (i, neighbors) in self.neighbors.iter().enumerate() {
            if neighbors.is_empty() {
                self.points[i] = (x + 44.0, y + 44.0);
                x += 88.0;
                if x + 88.0 > scale {
                    x = 0.0;
                    y += 88.0;
                }
            }
        }
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Target {
    Search,
    Fit,
    Isolated,
    Node(usize),
    Canvas,
}

pub(crate) struct View {
    pub(crate) center: Option<PathBuf>,
    pub(crate) selected: Option<PathBuf>,
    pub(crate) isolated: bool,
    pub(crate) fit: bool,
    pub(crate) area: Rect,
    pub(crate) hits: Vec<(Rect, Target)>,
    pub(crate) zoom: f32,
    pub(crate) pan: (f32, f32),
    pub(crate) local: Option<Graph>,
}

impl Default for View {
    fn default() -> Self {
        Self {
            center: None,
            selected: None,
            isolated: false,
            fit: true,
            area: Rect::default(),
            hits: Vec::new(),
            zoom: 1.0,
            pan: (0.0, 0.0),
            local: None,
        }
    }
}

impl View {
    pub(crate) fn graph<'a>(&'a self, index: &'a Index) -> &'a Graph {
        self.local.as_ref().unwrap_or(&index.graph)
    }

    pub(crate) fn fit(&mut self, index: &Index) {
        let graph = self.graph(index);
        let mut xs = (f32::INFINITY, f32::NEG_INFINITY);
        let mut ys = xs;
        for (i, &(x, y)) in graph.points.iter().enumerate() {
            if self.isolated || self.center.is_some() || !graph.neighbors[i].is_empty() {
                xs = (xs.0.min(x), xs.1.max(x));
                ys = (ys.0.min(y), ys.1.max(y));
            }
        }
        if xs.0.is_finite() && self.area.w > 0.0 && self.area.h > 0.0 {
            self.zoom = (self.area.w / (xs.1 - xs.0 + 120.0))
                .min(self.area.h / (ys.1 - ys.0 + 120.0))
                .clamp(0.005, 1.5);
            self.pan = (
                -self.zoom * (xs.0 + xs.1) / 2.0,
                -self.zoom * (ys.0 + ys.1) / 2.0,
            );
        }
        self.fit = false;
    }

    pub(crate) fn position(&self, (x, y): (f32, f32)) -> (f32, f32) {
        (
            self.area.x + self.area.w / 2.0 + self.pan.0 + x * self.zoom,
            self.area.y + self.area.h / 2.0 + self.pan.1 + y * self.zoom,
        )
    }
}

impl App {
    pub(crate) fn note_graph_shown(&self) -> bool {
        self.editor
            .active()
            .is_some_and(|d| d.reading == Some(Reading::NoteGraph))
    }

    pub(in crate::app) fn open_note_graph(&mut self, center: Option<PathBuf>) {
        if !self.note_graph_shown() && !self.prepare_writing_switch(None) {
            return;
        }
        self.ensure_note_links();
        let local = center
            .as_deref()
            .zip(self.note_links.index.as_ref())
            .map(|(path, index)| index.graph.local(path));
        self.note_links.graph = View {
            selected: center.clone(),
            center,
            local,
            ..View::default()
        };
        if !self.note_graph_shown() {
            let existing = self
                .editor
                .documents
                .iter()
                .position(|doc| doc.reading == Some(Reading::NoteGraph));
            if let Some(existing) = existing {
                self.activate_tab(existing);
            } else {
                self.auto_save_active();
                self.editor
                    .show_reading(t!("workspace-note-graph"), Reading::NoteGraph);
            }
        }
        self.focus = Focus::Editor;
        self.relayout();
        self.request_redraw();
    }

    pub(in crate::app) fn graph_index_updated(&mut self) {
        if let Some(index) = &self.note_links.index {
            let view = &mut self.note_links.graph;
            view.hits.clear();
            view.local = view.center.as_deref().map(|path| index.graph.local(path));
            if view
                .selected
                .as_ref()
                .is_some_and(|path| view.graph(index).paths.binary_search(path).is_err())
            {
                view.selected = None;
            }
        }
    }

    pub(in crate::app) fn graph_at(&self, x: f32, y: f32) -> Target {
        self.note_links
            .graph
            .hits
            .iter()
            .rev()
            .find(|(rect, _)| rect.contains(x, y))
            .map_or(Target::Canvas, |(_, target)| *target)
    }

    pub(in crate::app) fn press_note_graph(&mut self, target: Target, x: f32, y: f32) {
        self.focus = Focus::Editor;
        match target {
            Target::Canvas => self.pointer.drag = Drag::NoteGraph { at: (x, y) },
            Target::Search => self.open_graph_search(),
            Target::Fit => self.note_links.graph.fit = true,
            Target::Isolated => {
                self.note_links.graph.isolated ^= true;
                self.note_links.graph.fit = true;
            }
            Target::Node(i) => {
                let open = self.click_count(x, y) == 2;
                let Some(index) = self.note_links.index.as_ref() else {
                    return;
                };
                let Some(path) = self.note_links.graph.graph(index).paths.get(i).cloned() else {
                    return;
                };
                self.note_links.graph.selected = Some(path.clone());
                if open {
                    self.open_path(&path);
                }
            }
        }
        self.request_redraw();
    }

    pub(in crate::app) fn drag_note_graph(
        &mut self,
        (x, y): (f32, f32),
        (old_x, old_y): (f32, f32),
    ) {
        let pan = &mut self.note_links.graph.pan;
        pan.0 += x - old_x;
        pan.1 += y - old_y;
        self.pointer.drag = Drag::NoteGraph { at: (x, y) };
        self.request_redraw();
    }

    pub(in crate::app) fn wheel_note_graph(&mut self, delta: MouseScrollDelta) -> bool {
        let (x, y) = self.mouse();
        if !self.note_graph_shown()
            || self.overlays.picker.open
            || self.settings.open
            || self.overlays.form_dialog.is_some()
            || self.overlays.alert.is_some()
            || self.overlays.context_menu.is_some()
            || !self.note_links.graph.area.contains(x, y)
        {
            return false;
        }
        let view = &mut self.note_links.graph;
        let power = match delta {
            MouseScrollDelta::LineDelta(_, y) => y * 0.18,
            MouseScrollDelta::PixelDelta(delta) => delta.y as f32 * 0.003,
        };
        let zoom = (view.zoom * power.clamp(-2.0, 2.0).exp()).clamp(0.005, 4.0);
        let ratio = zoom / view.zoom;
        let at = (
            x - view.area.x - view.area.w / 2.0,
            y - view.area.y - view.area.h / 2.0,
        );
        view.pan = (
            at.0 - (at.0 - view.pan.0) * ratio,
            at.1 - (at.1 - view.pan.1) * ratio,
        );
        view.zoom = zoom;
        self.request_redraw();
        true
    }

    pub(in crate::app) fn focus_graph_note(&mut self, path: &Path) {
        let Some(index) = &self.note_links.index else {
            return;
        };
        let view = &mut self.note_links.graph;
        let graph = view.graph(index);
        let Ok(i) = graph.paths.binary_search_by(|p| p.as_path().cmp(path)) else {
            return;
        };
        let (x, y) = graph.points[i];
        if view.center.is_none() && graph.neighbors[i].is_empty() {
            view.isolated = true;
        }
        view.selected = Some(path.to_path_buf());
        view.zoom = view.zoom.max(1.0);
        view.pan = (-x * view.zoom, -y * view.zoom);
        view.fit = false;
        self.request_redraw();
    }

    pub(in crate::app) fn note_graph_key(&mut self, key: &Key, command: bool) {
        if command {
            return;
        }
        let view = &self.note_links.graph;
        match key {
            Key::Named(NamedKey::Enter) => {
                if let Some(path) = view.selected.clone() {
                    self.open_path(&path);
                }
            }
            Key::Named(NamedKey::Home) => {
                self.note_links.graph.fit = true;
                self.request_redraw();
            }
            Key::Named(
                NamedKey::Tab
                | NamedKey::ArrowLeft
                | NamedKey::ArrowRight
                | NamedKey::ArrowUp
                | NamedKey::ArrowDown,
            ) => {
                let Some(index) = &self.note_links.index else {
                    return;
                };
                let graph = view.graph(index);
                let reverse = self.pointer.modifiers.shift_key()
                    || matches!(key, Key::Named(NamedKey::ArrowLeft | NamedKey::ArrowUp));
                let current = view
                    .selected
                    .as_ref()
                    .and_then(|p| graph.paths.binary_search(p).ok());
                let len = graph.paths.len();
                let next = (0..len)
                    .map(|step| {
                        if reverse {
                            current.unwrap_or(0).wrapping_add(len) - step - 1
                        } else {
                            current.map_or(0, |i| i + 1) + step
                        }
                    })
                    .map(|step| step % len)
                    .find(|&i| {
                        view.isolated || view.center.is_some() || !graph.neighbors[i].is_empty()
                    });
                if let Some(path) = next.map(|i| graph.paths[i].clone()) {
                    self.focus_graph_note(&path);
                }
            }
            _ => {}
        }
    }
}

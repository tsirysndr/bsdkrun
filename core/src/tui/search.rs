//! The `/` search modal — an fzf-style jump-anywhere palette over all four
//! panels at once. Scoring and match-position highlighting come from
//! nucleo-matcher (the fzf-v2 algorithm extracted from Helix's nucleo), so
//! ranking behaves the way fingers trained on fzf expect: word-boundary and
//! camel-case bonuses, gap penalties, and per-character indices to paint.

use nucleo_matcher::pattern::{CaseMatching, Normalization, Pattern};
use nucleo_matcher::{Config, Matcher, Utf32Str};

use super::{App, Panel};

/// One searchable row, rebuilt from the snapshot when the modal opens.
pub struct Entry {
    pub panel: Panel,
    pub index: usize,
    /// What the matcher sees and the list shows, e.g.
    /// `machine  tidy_turing  running  nginx:alpine`.
    pub display: String,
}

/// A scored hit, re-derived on every keystroke.
pub struct Hit {
    pub entry: usize,
    pub score: u32,
    /// Character positions to highlight in `display`.
    pub indices: Vec<u32>,
}

pub struct SearchModal {
    pub query: String,
    pub entries: Vec<Entry>,
    pub hits: Vec<Hit>,
    pub sel: usize,
    matcher: Matcher,
    /// True until first scored against a snapshot (entries fill lazily so the
    /// modal opens even before the first snapshot lands).
    primed: bool,
}

impl Default for SearchModal {
    fn default() -> Self {
        Self::new()
    }
}

impl SearchModal {
    pub fn new() -> Self {
        SearchModal {
            query: String::new(),
            entries: Vec::new(),
            hits: Vec::new(),
            sel: 0,
            matcher: Matcher::new(Config::DEFAULT),
            primed: false,
        }
    }

    /// A modal primed against the current snapshot, ready to render.
    pub fn open(app: &App) -> Self {
        let mut m = Self::new();
        m.prime(app);
        m.rescore();
        m
    }

    /// Flatten the current snapshot into searchable rows.
    fn prime(&mut self, app: &App) {
        self.entries.clear();
        for (i, m) in app.snap.machines.iter().enumerate() {
            self.entries.push(Entry {
                panel: Panel::Machines,
                index: i,
                display: format!(
                    "machine  {}  {}  {}",
                    super::display_name(m),
                    m.status,
                    m.image
                ),
            });
        }
        for (i, im) in app.snap.images.iter().enumerate() {
            self.entries.push(Entry {
                panel: Panel::Images,
                index: i,
                display: format!("image  {}  {}", im.reference, im.id),
            });
        }
        for (i, v) in app.snap.volumes.iter().enumerate() {
            self.entries.push(Entry {
                panel: Panel::Volumes,
                index: i,
                display: format!("volume  {}  {}", v.name, v.guest.as_deref().unwrap_or("-")),
            });
        }
        for (i, n) in app.snap.networks.iter().enumerate() {
            self.entries.push(Entry {
                panel: Panel::Networks,
                index: i,
                display: format!("network  {}  {}", n.name, n.subnet),
            });
        }
        for (i, sn) in app.snap.disk_snapshots.iter().enumerate() {
            self.entries.push(Entry {
                panel: Panel::Snapshots,
                index: i,
                display: format!("snapshot  {}  {}  {}", sn.name, sn.machine_name, sn.kind),
            });
        }
        for (i, a) in app.snap.ai.iter().enumerate() {
            self.entries.push(Entry {
                panel: Panel::Ai,
                index: i,
                display: format!(
                    "ai  {}  {}  {}",
                    a.label.as_deref().unwrap_or(&a.name),
                    a.agent,
                    a.project.as_deref().unwrap_or("")
                ),
            });
        }
        self.primed = true;
    }

    /// Re-score every entry against the query. An empty query lists everything
    /// in panel order, so `/` doubles as a jump list.
    pub fn rescore(&mut self) {
        self.hits.clear();
        if self.query.is_empty() {
            self.hits.extend((0..self.entries.len()).map(|entry| Hit {
                entry,
                score: 0,
                indices: Vec::new(),
            }));
        } else {
            let pattern = Pattern::parse(&self.query, CaseMatching::Ignore, Normalization::Smart);
            let mut buf = Vec::new();
            for (i, e) in self.entries.iter().enumerate() {
                let mut indices = Vec::new();
                let haystack = Utf32Str::new(&e.display, &mut buf);
                if let Some(score) = pattern.indices(haystack, &mut self.matcher, &mut indices) {
                    self.hits.push(Hit {
                        entry: i,
                        score,
                        indices,
                    });
                }
            }
            self.hits.sort_by_key(|h| std::cmp::Reverse(h.score));
        }
        self.sel = self.sel.min(self.hits.len().saturating_sub(1));
    }
}

/// Keys inside the search modal.
pub fn handle_key(app: &mut App, key: crossterm::event::KeyEvent) {
    use crossterm::event::{KeyCode, KeyModifiers};
    // Prime against the snapshot on first interaction — taken out of `app`
    // first because `prime` borrows the snapshot.
    if app.search.as_ref().map(|m| !m.primed).unwrap_or(false) {
        let mut m = app.search.take().unwrap();
        m.prime(app);
        m.rescore();
        app.search = Some(m);
    }
    let Some(modal) = &mut app.search else { return };
    let ctrl = key.modifiers.contains(KeyModifiers::CONTROL);
    match key.code {
        KeyCode::Esc => app.search = None,
        KeyCode::Enter => {
            if let Some(hit) = modal.hits.get(modal.sel) {
                let e = &modal.entries[hit.entry];
                let (panel, index) = (e.panel, e.index);
                app.focus = panel;
                app.set_selection(panel, index);
                app.search = None;
            }
        }
        KeyCode::Down => modal.sel = (modal.sel + 1).min(modal.hits.len().saturating_sub(1)),
        KeyCode::Up => modal.sel = modal.sel.saturating_sub(1),
        KeyCode::Char('n') if ctrl => {
            modal.sel = (modal.sel + 1).min(modal.hits.len().saturating_sub(1))
        }
        KeyCode::Char('p') if ctrl => modal.sel = modal.sel.saturating_sub(1),
        KeyCode::Backspace => {
            modal.query.pop();
            modal.rescore();
        }
        // fzf's line-editing defaults: ^U clears, ^W deletes the last word.
        KeyCode::Char('u') if ctrl => {
            modal.query.clear();
            modal.rescore();
        }
        KeyCode::Char('w') if ctrl => {
            let trimmed = modal.query.trim_end();
            let cut = trimmed.rfind(' ').map(|i| i + 1).unwrap_or(0);
            modal.query.truncate(cut);
            modal.rescore();
        }
        KeyCode::Char(c) if !ctrl => {
            modal.query.push(c);
            modal.rescore();
        }
        _ => {}
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn modal_with(entries: Vec<(&str, Panel, usize)>) -> SearchModal {
        let mut m = SearchModal::new();
        m.entries = entries
            .into_iter()
            .map(|(display, panel, index)| Entry {
                panel,
                index,
                display: display.to_string(),
            })
            .collect();
        m.primed = true;
        m
    }

    #[test]
    fn fuzzy_query_ranks_the_tight_match_first() {
        let mut m = modal_with(vec![
            (
                "machine  brave_darwin  running  postgres:16",
                Panel::Machines,
                0,
            ),
            (
                "machine  tidy_turing  running  nginx:alpine",
                Panel::Machines,
                1,
            ),
            ("image  nginx:alpine  abc123", Panel::Images, 0),
        ]);
        m.query = "ttur".into();
        m.rescore();
        assert!(!m.hits.is_empty());
        let top = &m.entries[m.hits[0].entry];
        assert_eq!((top.panel, top.index), (Panel::Machines, 1));
        assert!(
            !m.hits[0].indices.is_empty(),
            "match positions for highlighting"
        );
    }

    #[test]
    fn empty_query_lists_everything() {
        let mut m = modal_with(vec![
            ("machine  a", Panel::Machines, 0),
            ("volume  b", Panel::Volumes, 0),
        ]);
        m.rescore();
        assert_eq!(m.hits.len(), 2);
    }

    #[test]
    fn non_matching_query_yields_nothing() {
        let mut m = modal_with(vec![("machine  a", Panel::Machines, 0)]);
        m.query = "zzz".into();
        m.rescore();
        assert!(m.hits.is_empty());
    }
}

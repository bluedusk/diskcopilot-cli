// ---------------------------------------------------------------------------
// SearchState — fuzzy (substring) search for TUI
// ---------------------------------------------------------------------------

#[derive(Debug, Clone)]
pub struct SearchResult {
    /// Index into visible_items (or list_items)
    pub index: usize,
    /// Match score (higher = better); currently: 1 for any match
    pub score: u32,
}

#[derive(Debug, Clone)]
pub struct SearchState {
    pub active: bool,
    pub query: String,
    pub results: Vec<SearchResult>,
}

impl SearchState {
    pub fn new() -> Self {
        Self {
            active: false,
            query: String::new(),
            results: Vec::new(),
        }
    }

    pub fn activate(&mut self) {
        self.active = true;
        self.query.clear();
        self.results.clear();
    }

    pub fn deactivate(&mut self) {
        self.active = false;
        self.query.clear();
        self.results.clear();
    }

    pub fn push_char(&mut self, c: char) {
        self.query.push(c);
    }

    pub fn pop_char(&mut self) {
        self.query.pop();
    }

    /// Populate `results` from `items`, which is a slice of (original_index, display_name).
    /// Uses case-insensitive substring matching. Empty query yields no results.
    pub fn filter(&mut self, items: &[(usize, String)]) {
        self.results.clear();
        if self.query.is_empty() {
            return;
        }
        let lower_query = self.query.to_lowercase();
        for (idx, name) in items {
            if name.to_lowercase().contains(&lower_query) {
                self.results.push(SearchResult { index: *idx, score: 1 });
            }
        }
    }
}

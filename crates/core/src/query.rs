/// Sortable directory-listing fields.
#[derive(Clone, Copy, Debug, Eq, Hash, PartialEq)]
pub enum SortKey {
    Name,
    Size,
    Modified,
    Kind,
}

/// Sort direction.
#[derive(Clone, Copy, Debug, Eq, Hash, PartialEq)]
pub enum SortDirection {
    Ascending,
    Descending,
}

/// Complete pane-local sort configuration.
#[derive(Clone, Copy, Debug, Eq, Hash, PartialEq)]
pub struct SortSpec {
    pub key: SortKey,
    pub direction: SortDirection,
    pub directories_first: bool,
}

impl Default for SortSpec {
    fn default() -> Self {
        Self {
            key: SortKey::Name,
            direction: SortDirection::Ascending,
            directories_first: true,
        }
    }
}

/// Incremental fuzzy filter configuration for one pane.
#[derive(Clone, Debug, Default, Eq, PartialEq)]
pub struct Filter {
    pub query: String,
    pub include_hidden: bool,
}

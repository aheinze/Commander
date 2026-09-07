//! Command-palette matching.

use super::*;

pub(super) fn matching_commands(query: &str) -> Vec<&'static CommandDefinition> {
    COMMANDS
        .iter()
        .filter(|definition| {
            definition.available
                && palette_query_matches(query, &[definition.label, definition.id.as_str()])
        })
        .collect()
}

pub(super) fn palette_query_matches(query: &str, candidates: &[&str]) -> bool {
    let candidates = candidates
        .iter()
        .map(|candidate| candidate.to_lowercase())
        .collect::<Vec<_>>();
    query
        .trim()
        .to_lowercase()
        .split_whitespace()
        .all(|term| candidates.iter().any(|candidate| candidate.contains(term)))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn command_palette_matches_multiple_keyboard_friendly_terms() {
        let commands = matching_commands("sort modified");
        assert_eq!(commands.len(), 1);
        assert_eq!(commands[0].id, CommandId::SortByModified);

        let commands = matching_commands("focus pane");
        assert!(
            commands
                .iter()
                .any(|command| command.id == CommandId::FocusFiles)
        );
    }
}

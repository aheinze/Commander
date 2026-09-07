//! Bounded syntax parsing on the preview worker, independent of GTK and its theme.

use std::path::Path;
use std::sync::LazyLock;
use std::time::{Duration, Instant};

use syntect::parsing::{ParseState, Scope, ScopeStack, SyntaxReference, SyntaxSet};

use crate::{CancelToken, Cancelled};

const MAX_HIGHLIGHT_BYTES: usize = 128 * 1_024;
const MAX_LINE_BYTES: usize = 4 * 1_024;
const MAX_LINES: usize = 2_000;
const MAX_SPANS: usize = 4_096;
const HIGHLIGHT_BUDGET: Duration = Duration::from_millis(500);

static SYNTAXES: LazyLock<SyntaxSet> = LazyLock::new(two_face::syntax::extra_newlines);
static RULES: LazyLock<Vec<(Scope, SyntaxKind)>> = LazyLock::new(|| {
    [
        ("comment", SyntaxKind::Comment),
        ("string", SyntaxKind::String),
        ("constant.character.escape", SyntaxKind::Escape),
        ("keyword.operator", SyntaxKind::Operator),
        ("keyword", SyntaxKind::Keyword),
        ("storage", SyntaxKind::Keyword),
        ("entity.name.type", SyntaxKind::Type),
        ("entity.name.class", SyntaxKind::Type),
        ("entity.name.namespace", SyntaxKind::Type),
        ("entity.name.tag", SyntaxKind::Type),
        ("support.type", SyntaxKind::Type),
        ("support.class", SyntaxKind::Type),
        ("entity.name.function", SyntaxKind::Function),
        ("support.function", SyntaxKind::Function),
        ("constant", SyntaxKind::Constant),
        ("entity.other.attribute-name", SyntaxKind::Attribute),
        ("meta.annotation", SyntaxKind::Attribute),
    ]
    .into_iter()
    .map(|(scope, kind)| (Scope::new(scope).expect("valid built-in scope"), kind))
    .collect()
});

/// Semantic colours; the UI supplies a palette suited to the current appearance.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum SyntaxKind {
    Comment,
    String,
    Escape,
    Keyword,
    Operator,
    Type,
    Function,
    Constant,
    Attribute,
}

/// A half-open range of Unicode character offsets, as expected by GTK TextBuffer.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct SyntaxSpan {
    pub start: usize,
    pub end: usize,
    pub kind: SyntaxKind,
}

#[derive(Clone, Copy)]
pub(super) struct TextSyntax {
    pub language: &'static str,
    syntax: Option<&'static SyntaxReference>,
}

/// Use the same registry for accepting files, labeling previews, and parsing them.
pub(super) fn for_path(path: &Path) -> Option<TextSyntax> {
    let filename = path
        .file_name()
        .and_then(|name| name.to_str())
        .unwrap_or_default();
    let extension = path
        .extension()
        .and_then(|extension| extension.to_str())
        .unwrap_or_default()
        .to_ascii_lowercase();
    // Avoid loading grammars for plain prose and logs. CMakeLists.txt is code.
    if (matches!(extension.as_str(), "txt" | "log" | "csv" | "conf")
        && !filename.eq_ignore_ascii_case("CMakeLists.txt"))
        || matches!(
            filename.to_ascii_lowercase().as_str(),
            "readme" | "license" | "licence" | "copying" | "authors" | "notice" | "changelog"
        )
    {
        return Some(TextSyntax {
            language: "Text",
            syntax: None,
        });
    }
    let syntaxes = &*SYNTAXES;
    let syntax = if filename.to_ascii_lowercase().starts_with("dockerfile.") {
        syntaxes.find_syntax_by_name("Dockerfile")
    } else if filename.starts_with(".env.") {
        syntaxes.find_syntax_by_name("DotENV")
    } else {
        syntaxes.find_syntax_by_extension(filename).or_else(|| {
            syntaxes.find_syntax_by_extension(match extension.as_str() {
                // The React grammar accepts JavaScript JSX as well as TSX.
                "jsx" => "tsx",
                "mjs" | "cjs" => "js",
                // The default registry otherwise resolves .h to Objective-C.
                "h" => "c",
                extension => extension,
            })
        })
    }?;
    let language = match (extension.as_str(), syntax.name.as_str()) {
        ("jsx", _) => "JavaScript (JSX)",
        (_, "TypeScriptReact") => "TypeScript (TSX)",
        (_, "Bourne Again Shell (bash)") => "Shell",
        (_, name) => name,
    };
    Some(TextSyntax {
        language,
        syntax: Some(syntax),
    })
}

pub(super) fn highlight(
    content: &str,
    text_syntax: TextSyntax,
    cancel: &CancelToken,
) -> Result<Vec<SyntaxSpan>, Cancelled> {
    cancel.check()?;
    let Some(syntax) = text_syntax.syntax else {
        return Ok(Vec::new());
    };
    let syntaxes = &*SYNTAXES;
    cancel.check()?;
    let mut parser = ParseState::new(syntax);
    let mut stack = ScopeStack::new();
    let mut spans = Vec::new();
    let mut character_offset = 0;
    let mut byte_offset = 0;
    let started = Instant::now();
    'lines: for line in content.split_inclusive('\n').take(MAX_LINES) {
        cancel.check()?;
        // Stop at a whole line to retain valid parser state for multiline tokens.
        // The rest of the original text stays visible without colours.
        if line.len() > MAX_LINE_BYTES
            || byte_offset + line.len() > MAX_HIGHLIGHT_BYTES
            || started.elapsed() >= HIGHLIGHT_BUDGET
        {
            break;
        }
        let Ok(changes) = parser.parse_line(line, syntaxes) else {
            break;
        };
        let mut previous = 0;
        for (offset, operation) in changes {
            cancel.check()?;
            if !append_span(
                &line[previous..offset],
                &stack,
                &mut character_offset,
                &mut spans,
            ) {
                break 'lines;
            }
            if stack.apply(&operation).is_err() {
                break 'lines;
            }
            previous = offset;
        }
        if !append_span(&line[previous..], &stack, &mut character_offset, &mut spans) {
            break;
        }
        byte_offset += line.len();
    }
    cancel.check()?;
    Ok(spans)
}

fn append_span(
    text: &str,
    stack: &ScopeStack,
    offset: &mut usize,
    spans: &mut Vec<SyntaxSpan>,
) -> bool {
    let start = *offset;
    *offset += text.chars().count();
    if start == *offset {
        return true;
    }
    let Some(kind) = stack.scopes.iter().rev().find_map(|scope| {
        RULES
            .iter()
            .find_map(|(prefix, kind)| prefix.is_prefix_of(*scope).then_some(*kind))
    }) else {
        return true;
    };
    if let Some(last) = spans.last_mut()
        && last.end == start
        && last.kind == kind
    {
        last.end = *offset;
    } else {
        if spans.len() >= MAX_SPANS {
            return false;
        }
        spans.push(SyntaxSpan {
            start,
            end: *offset,
            kind,
        });
    }
    true
}

#[cfg(test)]
mod tests {
    use super::*;

    fn spans_for(source: &str, filename: &str) -> Vec<SyntaxSpan> {
        let syntax = for_path(Path::new(filename)).expect("recognized text file");
        highlight(source, syntax, &CancelToken::new()).expect("highlight source")
    }

    fn rust_spans(source: &str) -> Vec<SyntaxSpan> {
        spans_for(source, "sample.rs")
    }

    fn kind_at(source: &str, spans: &[SyntaxSpan], needle: &str) -> Option<SyntaxKind> {
        let byte_offset = source.find(needle).expect("token in fixture");
        let offset = source[..byte_offset].chars().count();
        spans
            .iter()
            .find(|span| span.start <= offset && offset < span.end)
            .map(|span| span.kind)
    }

    #[test]
    fn rust_tokens_and_unicode_use_character_offsets() {
        let source = "// café 🦀\nfn main() { let answer = 42; let s = \"héllo\"; }\n";
        let spans = rust_spans(source);
        assert_eq!(kind_at(source, &spans, "café"), Some(SyntaxKind::Comment));
        assert_eq!(kind_at(source, &spans, "fn"), Some(SyntaxKind::Keyword));
        assert_eq!(kind_at(source, &spans, "main"), Some(SyntaxKind::Function));
        assert_eq!(kind_at(source, &spans, "42"), Some(SyntaxKind::Constant));
        assert_eq!(kind_at(source, &spans, "héllo"), Some(SyntaxKind::String));
        assert!(spans.windows(2).all(|pair| pair[0].end <= pair[1].start));
        assert!(
            spans
                .iter()
                .all(|span| span.start < span.end && span.end <= source.chars().count())
        );
    }

    #[test]
    fn multiline_comments_and_raw_strings_keep_their_state() {
        let source = "/* start\ncomment body */\nlet s = r#\"first\nraw string\"#;\n";
        let spans = rust_spans(source);
        assert_eq!(
            kind_at(source, &spans, "comment body"),
            Some(SyntaxKind::Comment)
        );
        assert_eq!(
            kind_at(source, &spans, "raw string"),
            Some(SyntaxKind::String)
        );
        assert_eq!(kind_at(source, &spans, "let"), Some(SyntaxKind::Keyword));
    }

    #[test]
    fn known_extensions_are_case_insensitive_and_plain_text_stays_plain() {
        assert!(!spans_for("fn main() {}", "sample.RS").is_empty());
        assert!(!spans_for("{\"answer\":42}", "sample.JSON").is_empty());
        for filename in [
            "sample.txt",
            "sample.log",
            "sample.csv",
            "sample.conf",
            "README",
            "LICENSE",
        ] {
            assert!(spans_for("fn main() {}", filename).is_empty(), "{filename}");
        }
        for filename in ["sample.unknown", "sample.bin", "unrecognized"] {
            assert!(for_path(Path::new(filename)).is_none(), "{filename}");
        }
    }

    #[test]
    fn web_and_configuration_languages_color_real_tokens() {
        let fixtures = [
            (
                "index.PHP",
                "<html><?php function greet() { return \"héllo\"; } ?></html>\n",
                "function",
                SyntaxKind::Keyword,
            ),
            (
                "types.ts",
                "export const answer: number = 42;\n",
                "const",
                SyntaxKind::Keyword,
            ),
            (
                "view.tsx",
                "const view = <button title=\"héllo\">Hello</button>;\n",
                "button",
                SyntaxKind::Type,
            ),
            (
                "view.jsx",
                "const view = <button title=\"héllo\">Hello</button>;\n",
                "title",
                SyntaxKind::Attribute,
            ),
            (
                "Cargo.toml",
                "[package]\nname = \"commander\"\n",
                "commander",
                SyntaxKind::String,
            ),
            (
                "settings.ini",
                "[settings]\n; configuration comment\n",
                "configuration comment",
                SyntaxKind::Comment,
            ),
            (
                "widget.vue",
                "<template><button title=\"héllo\">Hello</button></template>\n",
                "button",
                SyntaxKind::Type,
            ),
            (
                "widget.svelte",
                "<button title=\"héllo\">Hello</button>\n",
                "button",
                SyntaxKind::Type,
            ),
        ];
        for (filename, source, token, expected) in fixtures {
            let spans = spans_for(source, filename);
            assert_eq!(
                kind_at(source, &spans, token),
                Some(expected),
                "{filename}: {token}"
            );
        }
    }

    #[test]
    fn modern_module_extensions_and_build_filenames_are_recognized() {
        for (filename, language) in [
            ("index.phtml", "PHP"),
            ("module.mts", "TypeScript"),
            ("module.cts", "TypeScript"),
            ("module.mjs", "JavaScript"),
            ("module.cjs", "JavaScript"),
            ("View.JSX", "JavaScript (JSX)"),
            ("View.TSX", "TypeScript (TSX)"),
            ("header.h", "C"),
            ("Dockerfile", "Dockerfile"),
            ("Dockerfile.dev", "Dockerfile"),
            ("Makefile", "Makefile"),
            ("CMakeLists.txt", "CMake"),
            (".env", "DotENV"),
            (".env.preview", "DotENV"),
            (".gitignore", "Git Ignore"),
            (".editorconfig", "INI"),
            (".bashrc", "Shell"),
            ("PKGBUILD", "Shell"),
            ("Cargo.lock", "TOML"),
        ] {
            assert_eq!(
                for_path(Path::new(filename)).expect(filename).language,
                language,
                "{filename}"
            );
        }
    }

    #[test]
    fn oversized_lines_and_long_files_have_bounded_highlighting() {
        assert!(rust_spans(&"x".repeat(MAX_LINE_BYTES + 1)).is_empty());
        let source = "let a = 1; let b = 2;\n".repeat(MAX_LINES + 1);
        let spans = rust_spans(&source);
        assert!(spans.len() <= MAX_SPANS);
        assert!(spans.iter().all(|span| span.end <= MAX_HIGHLIGHT_BYTES));
    }

    #[test]
    fn cancelled_requests_do_not_return_highlights() {
        let cancel = CancelToken::new();
        cancel.cancel();
        let syntax = for_path(Path::new("sample.rs")).unwrap();
        assert!(highlight("fn main() {}", syntax, &cancel).is_err());
    }
}

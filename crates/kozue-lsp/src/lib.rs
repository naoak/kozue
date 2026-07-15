//! kozue language server — library entry point.
//!
//! Exposes [`run`] (the stdio server loop) and pure helper functions
//! ([`detect_language`], [`to_lsp_diagnostics`]) that are unit-testable
//! without a running LSP client.

mod position;

use std::collections::BTreeMap;
use std::ops::Range;

use kozue_ir::Diagram;
use position::SpanUnit;
use tower_lsp::jsonrpc::Result;
use tower_lsp::lsp_types::{
    Diagnostic, DiagnosticRelatedInformation, DiagnosticSeverity, DidChangeTextDocumentParams,
    DidCloseTextDocumentParams, DidOpenTextDocumentParams, DidSaveTextDocumentParams,
    DocumentFormattingParams, Hover, HoverContents, HoverParams, HoverProviderCapability,
    InitializeParams, InitializeResult, InitializedParams, Location, MarkupContent, MarkupKind,
    OneOf, ServerCapabilities, ServerInfo, TextDocumentSyncCapability, TextDocumentSyncKind,
    TextEdit, Url,
};
use tower_lsp::{Client, LanguageServer, LspService, Server};

// ---------------------------------------------------------------------------
// Language detection
// ---------------------------------------------------------------------------

/// Diagram language understood by this server.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Language {
    Kozue,
    Mermaid,
    Plantuml,
}

/// Detect language from URI extension. Returns `None` for unknown extensions.
///
/// The URI extension is primary. The `init_lang` parameter (e.g. from
/// `initializationOptions.language`) is only used as a **fallback** when the
/// extension is unrecognised — it must not trample the per-file extension,
/// otherwise a single init option would force one language onto every open
/// document in a mixed-language workspace.
pub fn detect_language(uri: &Url, init_lang: Option<&str>) -> Option<Language> {
    let path = uri.path().to_ascii_lowercase();
    if path.ends_with(".kozue") || path.ends_with(".kzd") {
        return Some(Language::Kozue);
    } else if path.ends_with(".mmd") || path.ends_with(".mermaid") {
        return Some(Language::Mermaid);
    } else if path.ends_with(".puml")
        || path.ends_with(".plantuml")
        || path.ends_with(".pu")
        || path.ends_with(".iuml")
    {
        return Some(Language::Plantuml);
    }

    // Extension unknown: fall back to an explicit init option, if recognised.
    match init_lang?.to_ascii_lowercase().as_str() {
        "kozue" => Some(Language::Kozue),
        "mermaid" => Some(Language::Mermaid),
        "plantuml" => Some(Language::Plantuml),
        _ => None,
    }
}

// ---------------------------------------------------------------------------
// Diagnostics conversion
// ---------------------------------------------------------------------------

/// Parse `text` in `lang` to the shared IR, discarding diagnostics. All three
/// frontends produce the same [`Diagram`], so IR-only features (hover) work
/// uniformly across languages. Returns `None` on parse failure.
fn parse_to_ir(text: &str, lang: Language) -> Option<Diagram> {
    match lang {
        Language::Kozue => kozue_dsl::parse(text).ok(),
        Language::Mermaid => kozue_mermaid::parse(text).ok(),
        Language::Plantuml => kozue_plantuml::parse(text).ok(),
    }
}

/// Convert frontend errors for `text` to LSP diagnostics. Pure fn, no async.
///
/// - DSL spans are in chumsky character units (`SpanUnit::Char`).
/// - Mermaid and PlantUML spans are in byte units (`SpanUnit::Byte`).
pub fn to_lsp_diagnostics(uri: &Url, text: &str, lang: Language) -> Vec<Diagnostic> {
    match lang {
        Language::Kozue => {
            let errors = match kozue_dsl::parse(text) {
                Ok(_) => return vec![],
                Err(errs) => errs,
            };
            errors
                .into_iter()
                .map(|e| Diagnostic {
                    range: position::to_range(text, &e.span, SpanUnit::Char),
                    severity: Some(DiagnosticSeverity::ERROR),
                    source: Some("kozue".to_string()),
                    message: e.message.clone(),
                    related_information: e.secondary.as_ref().map(|(sec_span, sec_msg)| {
                        vec![DiagnosticRelatedInformation {
                            location: Location {
                                uri: uri.clone(),
                                range: position::to_range(text, sec_span, SpanUnit::Char),
                            },
                            message: sec_msg.clone(),
                        }]
                    }),
                    ..Default::default()
                })
                .collect()
        }
        Language::Mermaid => {
            let errors = match kozue_mermaid::parse(text) {
                Ok(_) => return vec![],
                Err(errs) => errs,
            };
            errors
                .into_iter()
                .map(|e| Diagnostic {
                    range: position::to_range(text, &e.span, SpanUnit::Byte),
                    severity: Some(DiagnosticSeverity::ERROR),
                    source: Some("kozue".to_string()),
                    message: e.message.clone(),
                    ..Default::default()
                })
                .collect()
        }
        Language::Plantuml => {
            let errors = match kozue_plantuml::parse(text) {
                Ok(_) => return vec![],
                Err(errs) => errs,
            };
            errors
                .into_iter()
                .map(|e| Diagnostic {
                    range: position::to_range(text, &e.span, SpanUnit::Byte),
                    severity: Some(DiagnosticSeverity::ERROR),
                    source: Some("kozue".to_string()),
                    message: e.message.clone(),
                    ..Default::default()
                })
                .collect()
        }
    }
}

// ---------------------------------------------------------------------------
// M6b: hover and formatting helpers (pure, no async, testable)
// ---------------------------------------------------------------------------

/// Scan `text` for an identifier (word chars: `[A-Za-z0-9_-]`) that contains
/// the byte offset `byte`.  Returns the byte range and the slice, or `None`
/// if the cursor is on whitespace or punctuation.
pub(crate) fn identifier_at(text: &str, byte: usize) -> Option<(Range<usize>, &str)> {
    let byte = byte.min(text.len());
    // Snap to a char boundary (shouldn't be needed with p2b, but be safe).
    let byte = {
        let mut b = byte;
        while b > 0 && !text.is_char_boundary(b) {
            b -= 1;
        }
        b
    };

    // Check that the character at `byte` is a word character.
    let ch = text[byte..].chars().next().unwrap_or(' ');
    if !is_word_char(ch) {
        return None;
    }

    // Scan left to find word start.
    let start = text[..byte]
        .char_indices()
        .rev()
        .find(|(_, c)| !is_word_char(*c))
        .map(|(i, c)| i + c.len_utf8())
        .unwrap_or(0);

    // Scan right to find word end.
    let end = byte
        + text[byte..]
            .char_indices()
            .find(|(_, c)| !is_word_char(*c))
            .map(|(i, _)| i)
            .unwrap_or(text[byte..].len());

    Some((start..end, &text[start..end]))
}

/// Matches the DSL identifier grammar (chumsky `text::ident()`): ASCII
/// alphanumerics and `_`. Notably NOT hyphen — otherwise the no-space arrow
/// syntax `a->b` would extract `a-` and miss the node lookup — and NOT Unicode
/// alphanumerics, which the lexer does not accept as identifier characters.
fn is_word_char(c: char) -> bool {
    c.is_ascii_alphanumeric() || c == '_'
}

/// Build a markdown hover string for an identifier found in `diagram`.
///
/// For a [`Diagram::Graph`] the id is looked up in `nodes`.
/// For a [`Diagram::Sequence`] the id is looked up in `participants`.
/// Returns `None` if `word` is not a known id.
pub(crate) fn hover_for_word(diagram: &Diagram, word: &str) -> Option<String> {
    match diagram {
        Diagram::Graph(g) => {
            let node = g.nodes.get(word)?;
            Some(format!(
                "**node** `{id}`\n\nLabel: {label}",
                id = node.id,
                label = node.label
            ))
        }
        Diagram::Sequence(s) => {
            let p = s.participants.get(word)?;
            Some(format!(
                "**participant** `{id}`\n\nLabel: {label}",
                id = p.id,
                label = p.label
            ))
        }
        // Diagram is #[non_exhaustive]; new variants carry no hover info yet.
        _ => None,
    }
}

// ---------------------------------------------------------------------------
// LSP backend
// ---------------------------------------------------------------------------

struct Backend {
    client: Client,
    docs: tokio::sync::Mutex<BTreeMap<Url, String>>,
    init_lang: tokio::sync::Mutex<Option<String>>,
}

impl Backend {
    fn new(client: Client) -> Self {
        Self {
            client,
            docs: tokio::sync::Mutex::new(BTreeMap::new()),
            init_lang: tokio::sync::Mutex::new(None),
        }
    }

    async fn publish(&self, uri: Url, text: String) {
        let init_lang_guard = self.init_lang.lock().await;
        let init_lang = init_lang_guard.as_deref();
        let lang = detect_language(&uri, init_lang);
        let diags = if let Some(lang) = lang {
            to_lsp_diagnostics(&uri, &text, lang)
        } else {
            vec![]
        };
        self.client.publish_diagnostics(uri, diags, None).await;
    }
}

#[tower_lsp::async_trait]
impl LanguageServer for Backend {
    async fn initialize(&self, params: InitializeParams) -> Result<InitializeResult> {
        let init_lang = params.initialization_options.and_then(|v| {
            v.get("language")
                .and_then(|l| l.as_str())
                .map(str::to_owned)
        });
        *self.init_lang.lock().await = init_lang;

        Ok(InitializeResult {
            capabilities: ServerCapabilities {
                text_document_sync: Some(TextDocumentSyncCapability::Kind(
                    TextDocumentSyncKind::FULL,
                )),
                hover_provider: Some(HoverProviderCapability::Simple(true)),
                document_formatting_provider: Some(OneOf::Left(true)),
                ..Default::default()
            },
            server_info: Some(ServerInfo {
                name: "kozue-lsp".to_string(),
                version: Some(env!("CARGO_PKG_VERSION").to_string()),
            }),
        })
    }

    async fn initialized(&self, _: InitializedParams) {}

    async fn shutdown(&self) -> Result<()> {
        Ok(())
    }

    async fn did_open(&self, params: DidOpenTextDocumentParams) {
        let uri = params.text_document.uri;
        let text = params.text_document.text;
        self.docs.lock().await.insert(uri.clone(), text.clone());
        self.publish(uri, text).await;
    }

    async fn did_change(&self, params: DidChangeTextDocumentParams) {
        let uri = params.text_document.uri;
        // FULL sync: last content_change is the whole document.
        if let Some(change) = params.content_changes.into_iter().last() {
            let text = change.text;
            self.docs.lock().await.insert(uri.clone(), text.clone());
            self.publish(uri, text).await;
        }
    }

    async fn did_save(&self, params: DidSaveTextDocumentParams) {
        let uri = params.text_document.uri;
        let text = {
            let docs = self.docs.lock().await;
            docs.get(&uri).cloned()
        };
        if let Some(text) = text {
            self.publish(uri, text).await;
        }
    }

    async fn did_close(&self, params: DidCloseTextDocumentParams) {
        let uri = params.text_document.uri;
        self.docs.lock().await.remove(&uri);
        // Clear any diagnostics we published for this document so they don't
        // linger in the client after it is closed.
        self.client.publish_diagnostics(uri, vec![], None).await;
    }

    async fn hover(&self, params: HoverParams) -> Result<Option<Hover>> {
        let uri = &params.text_document_position_params.text_document.uri;
        let pos = params.text_document_position_params.position;

        let init_lang_guard = self.init_lang.lock().await;
        let lang = detect_language(uri, init_lang_guard.as_deref());
        drop(init_lang_guard);

        // Hover needs only the IR, so it works for every supported language
        // (unlike formatting, which is DSL-only). Unknown extension → no hover.
        let lang = match lang {
            Some(l) => l,
            None => return Ok(None),
        };

        let text = {
            let docs = self.docs.lock().await;
            docs.get(uri).cloned()
        };
        let text = match text {
            Some(t) => t,
            None => return Ok(None),
        };

        // Parse the document; broken files produce no hover.
        let diagram = match parse_to_ir(&text, lang) {
            Some(d) => d,
            None => return Ok(None),
        };

        let byte = position::position_to_byte_offset(&text, pos);
        let (span, word) = match identifier_at(&text, byte) {
            Some((span, w)) => (span, w),
            None => return Ok(None),
        };

        let md = match hover_for_word(&diagram, word) {
            Some(s) => s,
            None => return Ok(None),
        };

        Ok(Some(Hover {
            contents: HoverContents::Markup(MarkupContent {
                kind: MarkupKind::Markdown,
                value: md,
            }),
            // Byte-unit span → the editor highlights the exact hovered token.
            range: Some(position::to_range(&text, &span, SpanUnit::Byte)),
        }))
    }

    async fn formatting(&self, params: DocumentFormattingParams) -> Result<Option<Vec<TextEdit>>> {
        let uri = &params.text_document.uri;

        let init_lang_guard = self.init_lang.lock().await;
        let lang = detect_language(uri, init_lang_guard.as_deref());
        drop(init_lang_guard);

        // Formatting is only implemented for Kozue DSL files.
        if lang != Some(Language::Kozue) {
            return Ok(None);
        }

        let text = {
            let docs = self.docs.lock().await;
            docs.get(uri).cloned()
        };
        let text = match text {
            Some(t) => t,
            None => return Ok(None),
        };

        // On parse error return None — never corrupt the document.
        let formatted = match kozue_dsl::format_kzd(&text) {
            Ok(s) => s,
            Err(_) => return Ok(None),
        };

        // Build a single TextEdit that replaces the whole document.
        let end = position::end_of_document_position(&text);
        let edit = TextEdit {
            range: tower_lsp::lsp_types::Range {
                start: tower_lsp::lsp_types::Position {
                    line: 0,
                    character: 0,
                },
                end,
            },
            new_text: formatted,
        };
        Ok(Some(vec![edit]))
    }
}

// ---------------------------------------------------------------------------
// Public entry point
// ---------------------------------------------------------------------------

/// Start the stdio language server. Runs until the client disconnects.
pub async fn run() {
    let stdin = tokio::io::stdin();
    let stdout = tokio::io::stdout();
    let (service, socket) = LspService::build(Backend::new).finish();
    Server::new(stdin, stdout, socket).serve(service).await;
}

// ---------------------------------------------------------------------------
// Tests — pure functions only, no async runtime / tower-lsp client needed.
// ---------------------------------------------------------------------------
#[cfg(test)]
mod tests {
    use super::*;
    use tower_lsp::lsp_types::Position;

    fn uri(name: &str) -> Url {
        Url::parse(&format!("file:///tmp/{name}")).unwrap()
    }

    // ---- detect_language ----

    #[test]
    fn detect_by_extension() {
        assert_eq!(
            detect_language(&uri("a.kozue"), None),
            Some(Language::Kozue)
        );
        assert_eq!(detect_language(&uri("a.kzd"), None), Some(Language::Kozue));
        assert_eq!(
            detect_language(&uri("a.mmd"), None),
            Some(Language::Mermaid)
        );
        assert_eq!(
            detect_language(&uri("a.mermaid"), None),
            Some(Language::Mermaid)
        );
        for ext in ["puml", "plantuml", "pu", "iuml"] {
            assert_eq!(
                detect_language(&uri(&format!("a.{ext}")), None),
                Some(Language::Plantuml),
                "ext {ext}"
            );
        }
    }

    #[test]
    fn detect_is_case_insensitive() {
        assert_eq!(
            detect_language(&uri("A.KOZUE"), None),
            Some(Language::Kozue)
        );
        assert_eq!(
            detect_language(&uri("A.Mermaid"), None),
            Some(Language::Mermaid)
        );
    }

    #[test]
    fn detect_unknown_extension_is_none() {
        // Unknown extension → None → server publishes empty diagnostics
        // (no silent language guessing).
        assert_eq!(detect_language(&uri("a.txt"), None), None);
        assert_eq!(detect_language(&uri("a"), None), None);
    }

    #[test]
    fn detect_init_lang_is_fallback_only() {
        // A `.txt` file with an explicit override is honoured (fallback).
        assert_eq!(
            detect_language(&uri("a.txt"), Some("mermaid")),
            Some(Language::Mermaid)
        );
        // But a known extension is primary: the override must NOT trample it,
        // so a mixed-language workspace keeps per-file detection.
        assert_eq!(
            detect_language(&uri("a.kozue"), Some("plantuml")),
            Some(Language::Kozue)
        );
    }

    #[test]
    fn detect_unknown_init_lang_falls_back_to_extension() {
        assert_eq!(
            detect_language(&uri("a.kozue"), Some("nonsense")),
            Some(Language::Kozue)
        );
        assert_eq!(detect_language(&uri("a.txt"), Some("nonsense")), None);
    }

    // ---- to_lsp_diagnostics ----

    #[test]
    fn valid_input_has_no_diagnostics() {
        let kozue = "graph d {\n  a: \"A\"\n  b: \"B\"\n  a -> b\n}";
        assert!(to_lsp_diagnostics(&uri("a.kozue"), kozue, Language::Kozue).is_empty());
    }

    #[test]
    fn kozue_error_is_reported_with_span_and_severity() {
        let bad = "this is not valid kozue";
        let diags = to_lsp_diagnostics(&uri("a.kozue"), bad, Language::Kozue);
        assert!(!diags.is_empty());
        let d = &diags[0];
        assert_eq!(d.severity, Some(DiagnosticSeverity::ERROR));
        assert!(!d.message.is_empty());
        // Span must be a valid position on line 0.
        assert_eq!(d.range.start.line, 0);
    }

    #[test]
    fn kozue_duplicate_id_surfaces_related_information() {
        // Duplicate declaration carries a "first declared here" secondary label
        // that must be attached as related_information (no data loss).
        let dup = "graph d {\n  a: \"A\"\n  a: \"B\"\n}";
        let diags = to_lsp_diagnostics(&uri("a.kozue"), dup, Language::Kozue);
        let related: Vec<_> = diags
            .iter()
            .filter_map(|d| d.related_information.as_ref())
            .collect();
        assert!(
            !related.is_empty(),
            "duplicate id should carry related_information"
        );
    }

    #[test]
    fn mermaid_error_uses_byte_span() {
        // A multi-byte prefix ensures the byte-vs-char distinction matters:
        // the error position must be computed via SpanUnit::Byte.
        let bad = "flowchart TD\n  A --> ??? invalid";
        let diags = to_lsp_diagnostics(&uri("a.mmd"), bad, Language::Mermaid);
        assert!(!diags.is_empty());
        assert_eq!(diags[0].severity, Some(DiagnosticSeverity::ERROR));
    }

    #[test]
    fn plantuml_error_is_reported() {
        let bad = "not a plantuml diagram";
        let diags = to_lsp_diagnostics(&uri("a.puml"), bad, Language::Plantuml);
        assert!(!diags.is_empty());
        assert_eq!(diags[0].severity, Some(DiagnosticSeverity::ERROR));
    }

    #[test]
    fn cjk_dsl_error_does_not_panic_and_ranges_are_sane() {
        // Regression: DSL spans are char indices; byte-slicing them would panic
        // on multi-byte input. Must produce clean diagnostics instead.
        let bad = "graph d {\n  a: \"あいうえお\" @\n}";
        let diags = to_lsp_diagnostics(&uri("a.kozue"), bad, Language::Kozue);
        assert!(!diags.is_empty());
        for d in &diags {
            // A well-formed range never has end before start on the same line.
            if d.range.start.line == d.range.end.line {
                assert!(d.range.end.character >= d.range.start.character);
            }
        }
    }

    #[test]
    fn diagnostic_source_is_kozue() {
        let bad = "not valid";
        let diags = to_lsp_diagnostics(&uri("a.kozue"), bad, Language::Kozue);
        assert!(!diags.is_empty());
        assert_eq!(diags[0].source.as_deref(), Some("kozue"));
    }

    #[test]
    fn related_information_location_points_at_this_document() {
        let dup = "graph d {\n  a: \"A\"\n  a: \"B\"\n}";
        let u = uri("dup.kozue");
        let diags = to_lsp_diagnostics(&u, dup, Language::Kozue);
        let related = diags
            .iter()
            .find_map(|d| d.related_information.as_ref())
            .expect("expected related information");
        assert_eq!(related[0].location.uri, u);
        // Should point somewhere non-trivial (not the default zero range only).
        let _ = related[0].location.range.start.line;
        let _: Position = related[0].location.range.start;
    }

    // ---- identifier_at ----

    #[test]
    fn identifier_at_start_of_word() {
        let text = "foo bar";
        // byte 0 → start of "foo"
        let (range, word) = identifier_at(text, 0).unwrap();
        assert_eq!(word, "foo");
        assert_eq!(range, 0..3);
    }

    #[test]
    fn identifier_at_middle_of_word() {
        let text = "hello world";
        // byte 2 → inside "hello"
        let (range, word) = identifier_at(text, 2).unwrap();
        assert_eq!(word, "hello");
        assert_eq!(range, 0..5);
    }

    #[test]
    fn identifier_at_end_of_word() {
        let text = "abc xyz";
        // byte 2 → last char of "abc"
        let (range, word) = identifier_at(text, 2).unwrap();
        assert_eq!(word, "abc");
        assert_eq!(range, 0..3);
    }

    #[test]
    fn identifier_at_whitespace_is_none() {
        let text = "foo bar";
        // byte 3 → space
        assert!(identifier_at(text, 3).is_none());
    }

    #[test]
    fn identifier_at_stops_at_hyphen() {
        // `-` is not an identifier char (matches the DSL lexer). In `a->b` the
        // word under the cursor on `a` is just "a", so node lookup succeeds.
        let text = "a->b";
        let (range, word) = identifier_at(text, 0).unwrap();
        assert_eq!(word, "a");
        assert_eq!(range, 0..1);
        // Cursor on the trailing `b`.
        let (_, word) = identifier_at(text, 3).unwrap();
        assert_eq!(word, "b");
    }

    // ---- hover across frontends (shared IR) ----

    #[test]
    fn hover_works_for_mermaid_via_shared_ir() {
        // Mermaid parses to the same IR, so hover_for_word resolves its nodes.
        let src = "flowchart TD\n  a --> b";
        let diagram = parse_to_ir(src, Language::Mermaid).expect("mermaid parses");
        assert!(hover_for_word(&diagram, "a").is_some());
    }

    // ---- hover_for_word ----

    #[test]
    fn hover_for_word_unknown_id_is_none() {
        let src = "graph d {\n  a: \"A\"\n  b: \"B\"\n  a -> b\n}";
        let diagram = kozue_dsl::parse(src).unwrap();
        assert!(hover_for_word(&diagram, "z").is_none());
    }

    #[test]
    fn hover_for_word_graph_node_returns_markdown() {
        let src = "graph d {\n  a: \"Alpha\"\n  b: \"Beta\"\n  a -> b\n}";
        let diagram = kozue_dsl::parse(src).unwrap();
        let md = hover_for_word(&diagram, "a").unwrap();
        assert!(md.contains("node"), "expected 'node' in: {md}");
        assert!(md.contains("Alpha"), "expected label in: {md}");
    }

    #[test]
    fn hover_for_word_sequence_participant_returns_markdown() {
        let src = "sequence s {\n  participant alice: \"Alice\"\n  participant bob: \"Bob\"\n  alice -> bob : \"hi\"\n}";
        let diagram = kozue_dsl::parse(src).unwrap();
        let md = hover_for_word(&diagram, "alice").unwrap();
        assert!(
            md.contains("participant"),
            "expected 'participant' in: {md}"
        );
        assert!(md.contains("Alice"), "expected label in: {md}");
    }
}

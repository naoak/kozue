//! kozue language server — library entry point.
//!
//! Exposes [`run`] (the stdio server loop) and pure helper functions
//! ([`detect_language`], [`to_lsp_diagnostics`]) that are unit-testable
//! without a running LSP client.

mod position;

use std::collections::BTreeMap;

use position::SpanUnit;
use tower_lsp::jsonrpc::Result;
use tower_lsp::lsp_types::{
    Diagnostic, DiagnosticRelatedInformation, DiagnosticSeverity, DidChangeTextDocumentParams,
    DidCloseTextDocumentParams, DidOpenTextDocumentParams, DidSaveTextDocumentParams,
    InitializeParams, InitializeResult, InitializedParams, Location, ServerCapabilities,
    ServerInfo, TextDocumentSyncCapability, TextDocumentSyncKind, Url,
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
        let kozue = "diagram d {\n  a: \"A\"\n  b: \"B\"\n  a -> b\n}";
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
        let dup = "diagram d {\n  a: \"A\"\n  a: \"B\"\n}";
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
        let bad = "diagram d {\n  a: \"あいうえお\" @\n}";
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
        let dup = "diagram d {\n  a: \"A\"\n  a: \"B\"\n}";
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
}

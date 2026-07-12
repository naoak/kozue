//! kozue language server — stdio entry point.
//!
//! All real logic lives in [`kozue_lsp`] so it is fully testable without
//! the async runtime.

#[tokio::main]
async fn main() {
    kozue_lsp::run().await;
}

use std::sync::Arc;

use arc_lsp::{ArcLanguageServer, NullMcpTooling};
use tower_lsp::{LspService, Server};

#[tokio::main]
async fn main() {
    let stdin = tokio::io::stdin();
    let stdout = tokio::io::stdout();

    let (service, socket) =
        LspService::new(|client| ArcLanguageServer::new(client, Arc::new(NullMcpTooling)));

    Server::new(stdin, stdout, socket).serve(service).await;
}

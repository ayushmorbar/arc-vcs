#[tokio::main]
async fn main() -> anyhow::Result<()> {
    arc_diagnostics::init_tracing("arc_daemon");
    arc_daemon::server::run().await
}

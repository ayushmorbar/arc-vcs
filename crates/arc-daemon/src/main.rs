#[tokio::main]
async fn main() -> anyhow::Result<()> {
    arc_daemon::server::run().await
}

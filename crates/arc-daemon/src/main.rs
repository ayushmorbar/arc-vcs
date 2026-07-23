#[cfg(feature = "rpc-server")]
#[tokio::main]
async fn main() -> anyhow::Result<()> {
    arc_diagnostics::init_tracing("arc_daemon");
    arc_daemon::server::run().await
}

#[cfg(not(feature = "rpc-server"))]
fn main() -> anyhow::Result<()> {
    arc_diagnostics::init_tracing("arc_daemon");
    eprintln!(
        "arc-daemon built without rpc-server feature; use arc watch via arc-cli for autosnapshot \
         daemon mode"
    );
    Ok(())
}

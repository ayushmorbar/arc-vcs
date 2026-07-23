use arc_tui::{app::App, bridge::BackendBridge, provider::MockProvider};
use arc_ux::OutputEvent;

fn main() -> anyhow::Result<()> {
    let bridge = BackendBridge::channel(64);

    let sender = bridge.inbound.clone();
    std::thread::spawn(move || {
        let _ = sender.blocking_send(OutputEvent::Started("arc log --tui".to_string()));
        let _ =
            sender.blocking_send(OutputEvent::Progress(3, 5, "hydrating bento view".to_string()));
        let _ = sender.blocking_send(OutputEvent::Success(
            "ready".to_string(),
            vec!["5 mock changes loaded".to_string()],
        ));
    });

    let mut app = App::new(MockProvider, bridge.inbound_rx);
    app.run()
}

use arc_ux::OutputEvent;
use tokio::sync::mpsc;

pub struct BackendBridge {
    pub inbound: mpsc::Sender<OutputEvent>,
    pub inbound_rx: mpsc::Receiver<OutputEvent>,
}

impl BackendBridge {
    pub fn channel(capacity: usize) -> Self {
        let (backend_tx, inbound_rx) = mpsc::channel::<OutputEvent>(capacity);

        Self {
            inbound: backend_tx,
            inbound_rx,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::BackendBridge;
    use arc_ux::OutputEvent;

    #[tokio::test]
    async fn receives_backend_events_from_channel() {
        let mut bridge = BackendBridge::channel(8);
        bridge
            .inbound
            .send(OutputEvent::Started("load".to_string()))
            .await
            .expect("send event");

        let next = bridge
            .inbound_rx
            .recv()
            .await
            .expect("receive backend event");
        match next {
            OutputEvent::Started(op) => assert_eq!(op, "load"),
            _ => panic!("unexpected message variant"),
        }
    }
}

use arc_error::{ErrorExt, Exn, Message, message};

#[test]
fn tree_error_keeps_top_frame_display() {
    let exn: Exn<Message> = message("inner").raise().raise(message("outer"));
    let rendered = exn.to_string();
    assert!(rendered.contains("outer"));
}

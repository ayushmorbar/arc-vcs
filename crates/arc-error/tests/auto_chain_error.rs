use arc_error::{ErrorExt, Exn, Message, message};

#[test]
fn auto_chain_error_displays_context_chain() {
    let exn: Exn<Message> = message("inner").raise().raise(message("outer"));
    let rendered = exn.to_string();
    assert!(rendered.contains("outer"));
    assert!(rendered.contains("inner"));
}

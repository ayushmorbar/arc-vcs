use arc_error::*;
use std::error::Error as StdError;
use std::fmt;

// -- helper error types --------------------------------------------------

#[derive(Debug)]
struct Inner;
impl fmt::Display for Inner {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str("inner err")
    }
}
impl StdError for Inner {}

#[derive(Debug)]
struct Outer;
impl fmt::Display for Outer {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str("outer err")
    }
}
impl StdError for Outer {}

#[derive(Debug)]
struct Deep;
impl fmt::Display for Deep {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str("deep err")
    }
}
impl StdError for Deep {}

// -- Message -------------------------------------------------------------

#[test]
fn message_new_display() {
    let m = Message::new("oops");
    assert_eq!(m.to_string(), "oops");
}

#[test]
fn message_is_std_error() {
    let m = Message::new("fail");
    let e: &dyn StdError = &m;
    assert_eq!(e.to_string(), "fail");
    assert!(e.source().is_none());
}

#[test]
fn message_fn_shortcut() {
    let m = message("shortcut");
    assert_eq!(m.to_string(), "shortcut");
}

// -- message! macro ------------------------------------------------------

#[test]
fn message_macro_literal() {
    let m = message!("plain msg");
    assert_eq!(m.to_string(), "plain msg");
}

#[test]
fn message_macro_format() {
    let m = message!("code={}", 42);
    assert_eq!(m.to_string(), "code=42");
}

// -- bail! macro ---------------------------------------------------------

#[test]
fn bail_macro_returns_exn() {
    fn inner() -> Result<(), Exn<Message>> {
        bail!(message("gone"));
    }
    let err = inner().unwrap_err();
    assert!(err.to_string().contains("gone"));
}

// -- ensure! macro -------------------------------------------------------

#[test]
fn ensure_passes_on_true() {
    fn inner() -> Result<(), Exn<Message>> {
        ensure!(true, message("should not fire"));
        Ok(())
    }
    inner().unwrap();
}

#[test]
fn ensure_fails_on_false() {
    fn inner() -> Result<(), Exn<Message>> {
        ensure!(1 + 1 == 3, message!("math is broken"));
        Ok(())
    }
    let err = inner().unwrap_err();
    assert!(err.to_string().contains("math is broken"));
}

#[test]
fn ensure_with_message_error() {
    fn inner() -> Result<(), Exn<Message>> {
        ensure!(false, message!("literal string"));
        Ok(())
    }
    let err = inner().unwrap_err();
    assert!(err.to_string().contains("literal string"));
}

// -- Exn -----------------------------------------------------------------

#[test]
fn exn_new_captures_caller_location() {
    let exn: Exn<Message> = Exn::new(message("located"));
    let debug = format!("{:?}", exn);
    assert!(debug.contains("located"));
    // Debug format: "at <file>:<line>: <error>"
    assert!(debug.starts_with("at "));
}

#[test]
fn exn_display_shows_error_and_location() {
    let exn: Exn<Message> = Exn::new(message("displayed"));
    let s = exn.to_string();
    assert!(s.contains("displayed"));
    // Display format: "<error> (at <file>:<line>)"
    assert!(s.contains("(at "));
}

#[test]
fn exn_raise_chains_two_levels() {
    let exn: Exn<Outer> = Exn::new(Inner).raise(Outer);
    let s = exn.to_string();
    assert!(s.contains("outer err"));
}

#[test]
fn exn_probable_cause_returns_deepest() {
    // Each raise() wraps the previous frame as source.
    // probable_cause() walks source chain to the innermost frame.
    let exn: Exn<Deep> = message("root").raise().raise(Inner).raise(Outer).raise(Deep);
    // Deepest source is Message("root"), so probable_cause returns that.
    assert_eq!(exn.probable_cause().to_string(), "root");
}

#[test]
fn exn_probable_cause_single_level() {
    let exn: Exn<Message> = Exn::new(message("only"));
    assert_eq!(exn.probable_cause().to_string(), "only");
}

#[test]
fn exn_erased_converts_to_untyped() {
    let exn: Exn<Inner> = Exn::new(Inner);
    let erased: Exn = exn.erased();
    let s = erased.to_string();
    assert!(s.contains("inner err"));
}

#[test]
fn exn_std_error_source_returns_chained() {
    let exn: Exn<Outer> = Exn::new(Inner).raise(Outer);
    let source = <Exn<Outer> as StdError>::source(&exn);
    assert!(source.is_some());
    // Source is a Frame whose Display includes location info
    let source_str = source.unwrap().to_string();
    assert!(source_str.contains("inner err"));
    assert!(source_str.contains("(at "));
}

#[test]
fn exn_into_arc_error() {
    let exn: Exn<Message> = Exn::new(message("converted"));
    let err: Error = exn.into();
    assert!(err.to_string().contains("converted"));
}

// -- Frame ---------------------------------------------------------------

#[test]
fn frame_debug_shows_location() {
    let exn: Exn<Message> = Exn::new(message("frame test"));
    let debug = format!("{:?}", exn.frame);
    assert!(debug.contains("at"));
    assert!(debug.contains("frame test"));
}

#[test]
fn frame_display_shows_location_and_error() {
    let exn: Exn<Message> = Exn::new(message("frame disp"));
    let s = exn.frame.to_string();
    assert!(s.contains("frame disp"));
    // Frame Display: "<error> (at <file>:<line>)"
    assert!(s.contains("(at "));
}

#[test]
fn frame_source_is_none_for_single() {
    let exn: Exn<Message> = Exn::new(message("solo"));
    assert!(exn.frame.source.is_none());
}

#[test]
fn frame_source_present_after_raise() {
    let exn: Exn<Outer> = Exn::new(Inner).raise(Outer);
    assert!(exn.frame.source.is_some());
}

// -- Error wrapper -------------------------------------------------------

#[test]
fn error_from_std_error() {
    let err = Error::from_error(Inner);
    assert_eq!(err.to_string(), "inner err");
    assert!(err.source().is_none());
}

#[test]
fn error_debug_format() {
    let err = Error::from_error(Inner);
    let debug = format!("{:?}", err);
    // Debug delegates to the inner error's Debug derive → "Inner"
    assert_eq!(debug, "Inner");
}

#[test]
fn error_display_format() {
    let err = Error::from_error(Outer);
    let s = err.to_string();
    assert_eq!(s, "outer err");
}

// -- OptionExt -----------------------------------------------------------

#[test]
fn option_ext_some_returns_ok() {
    let result: Result<String, Exn<Message>> =
        Some("yes".to_string()).ok_or_raise(|| message("should not happen"));
    assert_eq!(result.unwrap(), "yes");
}

#[test]
fn option_ext_none_returns_exn() {
    let result: Result<String, Exn<Message>> = None.ok_or_raise(|| message("was none"));
    assert!(result.unwrap_err().to_string().contains("was none"));
}

// -- ResultExt -----------------------------------------------------------

#[test]
fn result_ext_ok_passthrough() {
    let result: Result<i32, Exn<Outer>> = Ok::<i32, Inner>(42).or_raise(|| Outer);
    assert_eq!(result.unwrap(), 42);
}

#[test]
fn result_ext_err_raises_context() {
    let result: Result<(), Exn<Outer>> = Result::<(), Inner>::Err(Inner).or_raise(|| Outer);
    let err = result.unwrap_err();
    assert!(err.to_string().contains("outer err"));
    assert_eq!(err.probable_cause().to_string(), "inner err");
}

#[test]
fn result_ext_deep_chain() {
    let result: Result<(), Exn<Deep>> =
        Result::<(), Inner>::Err(Inner).or_raise(|| Outer).or_raise(|| Deep);
    let err = result.unwrap_err();
    assert!(err.to_string().contains("deep err"));
    // or_raise boxes the previous Exn as error; probable_cause returns
    // the boxed Exn<Outer> which renders as "outer err (at ...)"
    assert!(err.probable_cause().to_string().contains("outer err"));
}

// -- Untyped -------------------------------------------------------------

#[test]
fn untyped_display() {
    let u = Untyped;
    assert_eq!(u.to_string(), "untyped error");
}

#[test]
fn untyped_debug() {
    let u = Untyped;
    let debug = format!("{:?}", u);
    assert_eq!(debug, "Untyped");
}

// -- Exn<Untyped> (default) ----------------------------------------------

#[test]
fn exn_untyped_default_parameter() {
    let exn: Exn = Exn::new(Untyped);
    assert!(exn.to_string().contains("untyped error"));
}

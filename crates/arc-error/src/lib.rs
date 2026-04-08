//! Error wrappers and result helpers shared across arc crates.
//!
//! This crate provides a lightweight [`Exn`] wrapper that captures caller
//! location and keeps ergonomic context propagation available via [`ResultExt`].

use std::{error::Error as StdError, fmt, panic::Location};

/// A lightweight message-only error.
#[derive(Debug, Clone)]
pub struct Message(String);

impl Message {
    /// Create a new message error.
    pub fn new(message: impl Into<String>) -> Self {
        Self(message.into())
    }
}

impl fmt::Display for Message {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(&self.0)
    }
}

impl StdError for Message {}

/// Build a message-only error.
pub fn message(message: impl Into<String>) -> Message {
    Message::new(message)
}

impl<E: StdError + Send + Sync + 'static> Exn<E> {
    #[track_caller]
    pub fn new(error: E) -> Self {
        Self {
            frame: Frame {
                location: Location::caller(),
                error: Box::new(error),
                source: None,
            },
            _marker: std::marker::PhantomData,
        }
    }

    #[track_caller]
    pub fn raise<Ctx: StdError + Send + Sync + 'static>(self, ctx: Ctx) -> Exn<Ctx> {
        Exn {
            frame: Frame {
                location: Location::caller(),
                error: Box::new(ctx),
                source: Some(Box::new(self.frame)),
            },
            _marker: std::marker::PhantomData,
        }
    }

    /// Convert a typed exception into a type-erased one.
    pub fn erased(self) -> Exn {
        Exn {
            frame: self.frame,
            _marker: std::marker::PhantomData,
        }
    }

    /// Return the most probable cause (deepest linked source frame).
    pub fn probable_cause(&self) -> &(dyn StdError + Send + Sync + 'static) {
        let mut current = &self.frame;
        while let Some(source) = current.source.as_ref() {
            current = source;
        }
        current.error.as_ref()
    }
}

/// A frame holding error context and caller location.
pub struct Frame {
    /// Source location at which this frame was created.
    pub location: &'static Location<'static>,
    /// Error value for this frame.
    pub error: Box<dyn StdError + Send + Sync + 'static>,
    /// Optional nested source frame.
    pub source: Option<Box<Frame>>,
}

impl fmt::Debug for Frame {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(
            f,
            "at {}:{}: {}",
            self.location.file(),
            self.location.line(),
            self.error
        )
    }
}

impl fmt::Display for Frame {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        #[cfg(feature = "auto-chain-error")]
        {
            write!(
                f,
                "{} (at {}:{})",
                self.error,
                self.location.file(),
                self.location.line()
            )?;

            let mut current = self.source.as_deref();
            while let Some(source) = current {
                write!(
                    f,
                    ": {} (at {}:{})",
                    source.error,
                    source.location.file(),
                    source.location.line()
                )?;
                current = source.source.as_deref();
            }
            Ok(())
        }

        #[cfg(not(feature = "auto-chain-error"))]
        {
            write!(
                f,
                "{} (at {}:{})",
                self.error,
                self.location.file(),
                self.location.line()
            )
        }
    }
}

impl StdError for Frame {
    fn source(&self) -> Option<&(dyn StdError + 'static)> {
        self.source
            .as_ref()
            .map(|frame| frame as &(dyn StdError + 'static))
    }
}

/// Exn error wrapper, similar to gitoxide's Exn<E>.
pub struct Exn<E: StdError + Send + Sync + 'static = Untyped> {
    pub frame: Frame,
    pub _marker: std::marker::PhantomData<E>,
}

/// Type-erased marker for [`Exn`] default parameter.
#[derive(Debug)]
pub struct Untyped;

impl fmt::Display for Untyped {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str("untyped error")
    }
}

impl StdError for Untyped {}

impl<E: StdError + Send + Sync + 'static> fmt::Debug for Exn<E> {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        self.frame.fmt(f)
    }
}

impl<E: StdError + Send + Sync + 'static> fmt::Display for Exn<E> {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        self.frame.fmt(f)
    }
}

impl<E: StdError + Send + Sync + 'static> StdError for Exn<E> {
    fn source(&self) -> Option<&(dyn StdError + 'static)> {
        self.frame.source()
    }
}

/// A boxed error wrapper that provides stable public error topology.
pub struct Error {
    inner: Box<dyn StdError + Send + Sync + 'static>,
}

impl Error {
    /// Wrap a concrete error as top-level arc error.
    pub fn from_error<E>(error: E) -> Self
    where
        E: StdError + Send + Sync + 'static,
    {
        Self {
            inner: Box::new(error),
        }
    }
}

impl fmt::Debug for Error {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        fmt::Debug::fmt(&self.inner, f)
    }
}

impl fmt::Display for Error {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        fmt::Display::fmt(&self.inner, f)
    }
}

impl StdError for Error {
    fn source(&self) -> Option<&(dyn StdError + 'static)> {
        self.inner.source()
    }
}

impl<E> From<Exn<E>> for Error
where
    E: StdError + Send + Sync + 'static,
{
    fn from(value: Exn<E>) -> Self {
        Self {
            inner: Box::new(value.frame),
        }
    }
}

/// Extension trait for error values supported by [`Exn`].
pub trait ErrorExt: StdError + Send + Sync + 'static {
    /// Raise this error into a new [`Exn`].
    #[track_caller]
    fn raise(self) -> Exn<Self>
    where
        Self: Sized,
    {
        Exn::new(self)
    }

    /// Raise this error and immediately wrap it in a higher-level context.
    #[track_caller]
    fn and_raise<Ctx>(self, context: Ctx) -> Exn<Ctx>
    where
        Self: Sized,
        Ctx: StdError + Send + Sync + 'static,
    {
        Exn::new(self).raise(context)
    }
}

impl<T> ErrorExt for T where T: StdError + Send + Sync + 'static {}

/// Extension trait for [`Option`] with context-aware failure conversion.
pub trait OptionExt<T> {
    /// Convert `None` into a raised contextual error.
    fn ok_or_raise<Ctx, F>(self, make_ctx: F) -> Result<T, Exn<Ctx>>
    where
        F: FnOnce() -> Ctx,
        Ctx: StdError + Send + Sync + 'static;
}

impl<T> OptionExt<T> for Option<T> {
    #[track_caller]
    fn ok_or_raise<Ctx, F>(self, make_ctx: F) -> Result<T, Exn<Ctx>>
    where
        F: FnOnce() -> Ctx,
        Ctx: StdError + Send + Sync + 'static,
    {
        match self {
            Some(value) => Ok(value),
            None => Err(Exn::new(make_ctx())),
        }
    }
}

pub trait ResultExt<T, E> {
    /// Convert `Err(E)` into contextual [`Exn<Ctx>`].
    fn or_raise<Ctx, F>(self, make_ctx: F) -> Result<T, Exn<Ctx>>
    where
        F: FnOnce() -> Ctx,
        Ctx: StdError + Send + Sync + 'static;
}

impl<T, E: StdError + Send + Sync + 'static> ResultExt<T, E> for Result<T, E> {
    #[track_caller]
    fn or_raise<Ctx, F>(self, make_ctx: F) -> Result<T, Exn<Ctx>>
    where
        F: FnOnce() -> Ctx,
        Ctx: StdError + Send + Sync + 'static,
    {
        self.map_err(|err| Exn::new(err).raise(make_ctx()))
    }
}

/// Return early with a raised error.
#[macro_export]
macro_rules! bail {
    ($err:expr) => {{
        return ::std::result::Result::Err($crate::Exn::new($err));
    }};
}

/// Ensure condition holds, otherwise return a raised error.
#[macro_export]
macro_rules! ensure {
    ($cond:expr, $err:expr $(,)?) => {{
        if !bool::from($cond) {
            $crate::bail!($err)
        }
    }};
}

/// Build a formatted [`Message`] error.
#[macro_export]
macro_rules! message {
    ($message_with_format_args:literal $(,)?) => {
        $crate::Message::new(format!($message_with_format_args))
    };
    ($fmt:expr, $($arg:tt)*) => {
        $crate::Message::new(format!($fmt, $($arg)*))
    };
}

#[cfg(test)]
mod tests {
    use super::*;

    #[derive(Debug)]
    struct Inner;

    impl fmt::Display for Inner {
        fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
            f.write_str("inner")
        }
    }

    impl StdError for Inner {}

    #[derive(Debug)]
    struct Outer;

    impl fmt::Display for Outer {
        fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
            f.write_str("outer")
        }
    }

    impl StdError for Outer {}

    #[test]
    fn result_ext_preserves_cause_chain() {
        let err = Result::<(), Inner>::Err(Inner)
            .or_raise(|| Outer)
            .expect_err("must fail");
        let rendered = err.to_string();
        assert!(rendered.contains("outer"));
        assert_eq!(err.probable_cause().to_string(), "inner");
    }
}

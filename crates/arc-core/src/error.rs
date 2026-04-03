impl<E: Error + Send + Sync + 'static> Exn<E> {
    #[track_caller]
    pub fn new(error: E) -> Self {
        Self {
            frame: Frame {
                location: Location::caller(),
                error: Box::new(error),
            },
            _marker: std::marker::PhantomData,
        }
    }
    #[track_caller]
    pub fn raise<Ctx: Error + Send + Sync + 'static>(self, _ctx: Ctx) -> Self {
        self
    }
}
use std::{error::Error, fmt, panic::Location};

/// A frame holding error context and caller location.
pub struct Frame {
    pub location: &'static Location<'static>,
    pub error: Box<dyn Error + Send + Sync + 'static>,
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
        write!(
            f,
            "{} (at {}:{})",
            self.error,
            self.location.file(),
            self.location.line()
        )
    }
}

impl Error for Frame {}

/// Exn error wrapper, similar to gitoxide's Exn<E>.
pub struct Exn<E: Error + Send + Sync + 'static> {
    pub frame: Frame,
    pub _marker: std::marker::PhantomData<E>,
}

impl<E: Error + Send + Sync + 'static> fmt::Debug for Exn<E> {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        self.frame.fmt(f)
    }
}

impl<E: Error + Send + Sync + 'static> fmt::Display for Exn<E> {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        self.frame.fmt(f)
    }
}

impl<E: Error + Send + Sync + 'static> Error for Exn<E> {}

pub trait ResultExt<T, E> {
    fn or_raise<Ctx, F>(self, make_ctx: F) -> Result<T, Exn<Ctx>>
    where
        F: FnOnce() -> Ctx,
        Ctx: std::error::Error + Send + Sync + 'static;
}

impl<T, E: std::error::Error + Send + Sync + 'static> ResultExt<T, E> for Result<T, E> {
    #[track_caller]
    fn or_raise<Ctx, F>(self, make_ctx: F) -> Result<T, Exn<Ctx>>
    where
        F: FnOnce() -> Ctx,
        Ctx: std::error::Error + Send + Sync + 'static,
    {
        self.map_err(|_e| Exn::new(make_ctx()))
    }
}

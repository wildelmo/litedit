//! Clipboard access behind a trait: the system clipboard in the window,
//! an in-memory one for scripts and tests.

pub trait Clipboard {
    fn get(&mut self) -> Option<String>;
    fn set(&mut self, text: String);
}

/// In-memory clipboard (scripts, tests, non-Windows dev builds).
#[derive(Default)]
pub struct MemClipboard(pub Option<String>);

impl Clipboard for MemClipboard {
    fn get(&mut self) -> Option<String> {
        self.0.clone()
    }

    fn set(&mut self, text: String) {
        self.0 = Some(text);
    }
}

/// The Windows clipboard (via arboard). Falls back to memory if the
/// clipboard cannot be opened.
#[cfg(windows)]
pub struct SystemClipboard {
    inner: Option<arboard::Clipboard>,
    fallback: MemClipboard,
}

#[cfg(windows)]
impl SystemClipboard {
    pub fn new() -> Self {
        Self {
            inner: arboard::Clipboard::new().ok(),
            fallback: MemClipboard::default(),
        }
    }
}

#[cfg(windows)]
impl Clipboard for SystemClipboard {
    fn get(&mut self) -> Option<String> {
        match self.inner.as_mut() {
            Some(c) => c.get_text().ok(),
            None => self.fallback.get(),
        }
    }

    fn set(&mut self, text: String) {
        match self.inner.as_mut() {
            Some(c) => {
                let _ = c.set_text(text);
            }
            None => self.fallback.set(text),
        }
    }
}

/// The clipboard used by the real window on this platform.
pub fn system() -> Box<dyn Clipboard> {
    #[cfg(windows)]
    {
        Box::new(SystemClipboard::new())
    }
    #[cfg(not(windows))]
    {
        Box::new(MemClipboard::default())
    }
}

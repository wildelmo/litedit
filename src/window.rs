//! The real window. winit events are translated into [`Input`]; the app's
//! [`Request`]s are carried out here. Nothing else in the app touches winit.

use std::num::NonZeroU32;
use std::path::PathBuf;
use std::rc::Rc;
use std::time::{Duration, Instant};

use winit::application::ApplicationHandler;
use winit::dpi::LogicalSize;
use winit::event::{ElementState, Ime, MouseScrollDelta, WindowEvent};
use winit::event_loop::{ActiveEventLoop, ControlFlow, EventLoop};
use winit::keyboard::{Key as WKey, ModifiersState, NamedKey};
use winit::window::{CursorIcon, Window, WindowId};

use crate::app::App;
use crate::canvas::Canvas;
use crate::input::{Cursor, Input, Key, Mods, MouseButton, Request, WheelUnit, translate_key};
use crate::watch::FsWatcher;

/// Events sent to the UI thread from other threads (file watcher, etc.).
#[derive(Debug)]
pub enum UserEvent {
    FsChanged(Vec<PathBuf>),
}

const DOUBLE_CLICK: Duration = Duration::from_millis(450);
/// Per-iteration budget for deferred work, so input stays responsive.
const TICK_BUDGET: Duration = Duration::from_millis(8);
/// Pause between deferred-work slices.
const PENDING_WAKE: Duration = Duration::from_millis(4);

struct Gfx {
    window: Rc<Window>,
    surface: softbuffer::Surface<Rc<Window>, Rc<Window>>,
}

struct Runner {
    app: App,
    gfx: Option<Gfx>,
    watcher: FsWatcher,
    mods: ModifiersState,
    mouse: (f32, f32),
    last_click: Option<(Instant, (f32, f32), u32)>,
    title: String,
    cursor: Cursor,
}

impl Runner {
    fn after_input(&mut self, event_loop: &ActiveEventLoop, mut redraw: bool) {
        let Some(window) = self.gfx.as_ref().map(|g| g.window.clone()) else {
            return;
        };
        for req in self.app.take_requests() {
            match req {
                Request::Quit => event_loop.exit(),
                // Handled inside the app; never reaches the window.
                Request::OpenPath(_) => {}
                Request::OpenUrl(url) => {
                    let _ = crate::platform::open_url(&url);
                }
                Request::RevealInExplorer(p) => {
                    let _ = crate::platform::reveal_in_explorer(&p);
                }
                Request::OpenFileDialog => {
                    if let Some(p) = pick(&window, false) {
                        self.app.open(&p);
                        redraw = true;
                    }
                }
                Request::OpenFolderDialog => {
                    if let Some(p) = pick(&window, true) {
                        self.app.open(&p);
                        redraw = true;
                    }
                }
                Request::SaveAsDialog(suggested) => {
                    if let Some(p) = pick_save(&window, &suggested) {
                        self.app.save_as(&p);
                        redraw = true;
                    }
                }
            }
        }
        self.watcher.set_targets(self.app.watch_targets());
        let title = self.app.title();
        if title != self.title {
            window.set_title(&title);
            self.title = title;
        }
        let cursor = self.app.cursor();
        if cursor != self.cursor {
            window.set_cursor(match cursor {
                Cursor::Default => CursorIcon::Default,
                Cursor::Pointer => CursorIcon::Pointer,
                Cursor::Text => CursorIcon::Text,
                Cursor::ResizeHorizontal => CursorIcon::EwResize,
            });
            self.cursor = cursor;
        }
        if redraw {
            window.request_redraw();
        }
    }

    fn mods(&self) -> Mods {
        Mods {
            ctrl: self.mods.control_key(),
            shift: self.mods.shift_key(),
            alt: self.mods.alt_key(),
        }
    }
}

fn map_key(k: &WKey) -> Option<Key> {
    Some(match k {
        WKey::Named(n) => match n {
            NamedKey::ArrowUp => Key::Up,
            NamedKey::ArrowDown => Key::Down,
            NamedKey::ArrowLeft => Key::Left,
            NamedKey::ArrowRight => Key::Right,
            NamedKey::PageUp => Key::PageUp,
            NamedKey::PageDown => Key::PageDown,
            NamedKey::Home => Key::Home,
            NamedKey::End => Key::End,
            NamedKey::Enter => Key::Enter,
            NamedKey::Escape => Key::Escape,
            NamedKey::Tab => Key::Tab,
            NamedKey::Backspace => Key::Backspace,
            NamedKey::Delete => Key::Delete,
            NamedKey::Insert => Key::Insert,
            NamedKey::Space => Key::Char(' '),
            NamedKey::F1 => Key::F(1),
            NamedKey::F2 => Key::F(2),
            NamedKey::F3 => Key::F(3),
            NamedKey::F4 => Key::F(4),
            NamedKey::F5 => Key::F(5),
            NamedKey::F6 => Key::F(6),
            NamedKey::F7 => Key::F(7),
            NamedKey::F8 => Key::F(8),
            NamedKey::F9 => Key::F(9),
            NamedKey::F10 => Key::F(10),
            NamedKey::F11 => Key::F(11),
            NamedKey::F12 => Key::F(12),
            _ => return None,
        },
        WKey::Character(s) => Key::Char(s.chars().next()?.to_lowercase().next()?),
        _ => return None,
    })
}

impl ApplicationHandler<UserEvent> for Runner {
    fn resumed(&mut self, event_loop: &ActiveEventLoop) {
        if self.gfx.is_some() {
            return;
        }
        let attrs = Window::default_attributes()
            .with_title(self.app.title())
            .with_inner_size(LogicalSize::new(1100.0, 750.0))
            .with_min_inner_size(LogicalSize::new(320.0, 200.0));
        let window = match event_loop.create_window(attrs) {
            Ok(w) => Rc::new(w),
            Err(e) => panic!("could not create window: {e}"),
        };
        let context = softbuffer::Context::new(window.clone()).expect("softbuffer context");
        let surface =
            softbuffer::Surface::new(&context, window.clone()).expect("softbuffer surface");
        self.app.handle(Input::Scale(window.scale_factor() as f32));
        let size = window.inner_size();
        self.app.handle(Input::Resize(size.width, size.height));
        // Let input methods (Chinese, Japanese, Korean, …) compose text.
        window.set_ime_allowed(true);
        window.request_redraw();
        self.gfx = Some(Gfx { window, surface });
        self.watcher.set_targets(self.app.watch_targets());
    }

    fn user_event(&mut self, event_loop: &ActiveEventLoop, event: UserEvent) {
        let redraw = match event {
            UserEvent::FsChanged(paths) => self.app.handle(Input::FsChanged(paths)),
        };
        self.after_input(event_loop, redraw);
    }

    fn window_event(&mut self, event_loop: &ActiveEventLoop, _id: WindowId, event: WindowEvent) {
        let input = match event {
            // Goes through the app so unsaved edits can be saved first.
            WindowEvent::CloseRequested => Input::CloseRequested,
            WindowEvent::RedrawRequested => {
                self.redraw();
                return;
            }
            WindowEvent::Resized(s) => Input::Resize(s.width, s.height),
            WindowEvent::ScaleFactorChanged { scale_factor, .. } => {
                Input::Scale(scale_factor as f32)
            }
            WindowEvent::ModifiersChanged(m) => {
                self.mods = m.state();
                return;
            }
            WindowEvent::DroppedFile(p) => Input::Drop(p),
            // Text composed by an input method arrives here (not as key text).
            WindowEvent::Ime(Ime::Commit(text)) => Input::Text(text),
            WindowEvent::CursorMoved { position, .. } => {
                self.mouse = (position.x as f32, position.y as f32);
                Input::MouseMove {
                    x: self.mouse.0,
                    y: self.mouse.1,
                }
            }
            WindowEvent::CursorLeft { .. } => Input::MouseLeave,
            WindowEvent::MouseInput { state, button, .. } => {
                let button = match button {
                    winit::event::MouseButton::Left => MouseButton::Left,
                    winit::event::MouseButton::Right => MouseButton::Right,
                    winit::event::MouseButton::Middle => MouseButton::Middle,
                    _ => return,
                };
                let (x, y) = self.mouse;
                if state == ElementState::Pressed {
                    let now = Instant::now();
                    let clicks = match self.last_click {
                        Some((t, (lx, ly), n))
                            if now - t < DOUBLE_CLICK
                                && (lx - x).abs() < 5.0
                                && (ly - y).abs() < 5.0 =>
                        {
                            n % 3 + 1
                        }
                        _ => 1,
                    };
                    self.last_click = Some((now, (x, y), clicks));
                    Input::MouseDown {
                        x,
                        y,
                        button,
                        clicks,
                    }
                } else {
                    Input::MouseUp { x, y, button }
                }
            }
            WindowEvent::MouseWheel { delta, .. } => {
                let (mut dx, mut dy, unit) = match delta {
                    MouseScrollDelta::LineDelta(x, y) => (-x, -y, WheelUnit::Lines),
                    MouseScrollDelta::PixelDelta(p) => {
                        (-p.x as f32, -p.y as f32, WheelUnit::Pixels)
                    }
                };
                if self.mods.shift_key() && dx == 0.0 {
                    std::mem::swap(&mut dx, &mut dy);
                }
                Input::Wheel { dx, dy, unit }
            }
            WindowEvent::KeyboardInput { event, .. } => {
                if event.state != ElementState::Pressed {
                    return;
                }
                let inputs = translate_key(
                    map_key(&event.logical_key),
                    event.text.as_deref(),
                    self.mods(),
                );
                let mut redraw = false;
                for input in inputs {
                    redraw |= self.app.handle(input);
                }
                self.after_input(event_loop, redraw);
                return;
            }
            _ => return,
        };
        let redraw = self.app.handle(input);
        self.after_input(event_loop, redraw);
    }

    fn about_to_wait(&mut self, event_loop: &ActiveEventLoop) {
        if self.app.has_pending_work() && self.app.tick(TICK_BUDGET).redraw {
            if let Some(g) = &self.gfx {
                g.window.request_redraw();
            }
        }
        // While background work is pending, wake up again shortly (but leave
        // the CPU idle in between); otherwise sleep until the next event.
        event_loop.set_control_flow(if self.app.has_pending_work() {
            ControlFlow::WaitUntil(Instant::now() + PENDING_WAKE)
        } else {
            ControlFlow::Wait
        });
    }
}

impl Runner {
    fn redraw(&mut self) {
        let Some(gfx) = self.gfx.as_mut() else { return };
        let size = gfx.window.inner_size();
        let (Some(w), Some(h)) = (NonZeroU32::new(size.width), NonZeroU32::new(size.height)) else {
            return; // minimised
        };
        if gfx.surface.resize(w, h).is_err() {
            return;
        }
        let Ok(mut buffer) = gfx.surface.buffer_mut() else {
            return;
        };
        {
            let mut canvas = Canvas::new(&mut buffer, size.width, size.height);
            self.app.render(&mut canvas);
        }
        let _ = buffer.present();
        if let Some(r) = self.app.ime_area() {
            gfx.window.set_ime_cursor_area(
                winit::dpi::PhysicalPosition::new(r.x, r.y),
                winit::dpi::PhysicalSize::new(r.w.max(1) as u32, r.h.max(1) as u32),
            );
        }
    }
}

pub fn run(app: App) {
    let event_loop = EventLoop::<UserEvent>::with_user_event()
        .build()
        .expect("event loop");
    event_loop.set_control_flow(ControlFlow::Wait);
    let proxy = event_loop.create_proxy();
    let watcher =
        FsWatcher::new(move |paths| proxy.send_event(UserEvent::FsChanged(paths)).is_ok());
    let mut app = app;
    app.set_clipboard(crate::clipboard::system());
    let mut runner = Runner {
        app,
        gfx: None,
        watcher,
        mods: ModifiersState::empty(),
        mouse: (0.0, 0.0),
        last_click: None,
        title: String::new(),
        cursor: Cursor::Default,
    };
    event_loop.run_app(&mut runner).expect("event loop run");
}

/// Native open dialog (Windows). Blocks while the modal dialog is shown.
#[cfg(windows)]
fn pick(window: &Window, folder: bool) -> Option<PathBuf> {
    let mut d = rfd::FileDialog::new().set_parent(window);
    if let Ok(dir) = std::env::current_dir() {
        d = d.set_directory(dir);
    }
    if folder {
        d.set_title("Open folder").pick_folder()
    } else {
        d.set_title("Open file")
            .add_filter("Markdown", &["md", "markdown", "mdown", "mkd"])
            .add_filter(
                "Text and code",
                &[
                    "txt", "rs", "py", "js", "ts", "json", "toml", "yaml", "yml", "c", "h", "cpp",
                    "cs", "go", "java", "html", "css", "xml", "ps1", "bat", "sh", "ini", "log",
                    "csv",
                ],
            )
            .add_filter("All files", &["*"])
            .pick_file()
    }
}

#[cfg(not(windows))]
fn pick(_window: &Window, _folder: bool) -> Option<PathBuf> {
    None
}

/// Native "Save as" dialog (Windows).
#[cfg(windows)]
fn pick_save(window: &Window, suggested: &std::path::Path) -> Option<PathBuf> {
    let mut d = rfd::FileDialog::new()
        .set_parent(window)
        .set_title("Save as");
    if let Some(dir) = suggested.parent() {
        d = d.set_directory(dir);
    }
    if let Some(name) = suggested.file_name() {
        d = d.set_file_name(name.to_string_lossy());
    }
    d.save_file()
}

#[cfg(not(windows))]
fn pick_save(_window: &Window, _suggested: &std::path::Path) -> Option<PathBuf> {
    None
}

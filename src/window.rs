//! The real window: winit events → App, App::render → softbuffer.

use std::num::NonZeroU32;
use std::rc::Rc;

use winit::application::ApplicationHandler;
use winit::dpi::LogicalSize;
use winit::event::{ElementState, MouseScrollDelta, WindowEvent};
use winit::event_loop::{ActiveEventLoop, ControlFlow, EventLoop};
use winit::keyboard::{Key, NamedKey};
use winit::window::{Window, WindowId};

use crate::app::App;
use crate::canvas::Canvas;

struct Gfx {
    window: Rc<Window>,
    surface: softbuffer::Surface<Rc<Window>, Rc<Window>>,
}

struct Runner {
    app: App,
    gfx: Option<Gfx>,
}

impl ApplicationHandler for Runner {
    fn resumed(&mut self, event_loop: &ActiveEventLoop) {
        if self.gfx.is_some() {
            return;
        }
        let attrs = Window::default_attributes()
            .with_title(self.app.title())
            .with_inner_size(LogicalSize::new(1100.0, 750.0));
        let window = match event_loop.create_window(attrs) {
            Ok(w) => Rc::new(w),
            Err(e) => panic!("could not create window: {e}"),
        };
        let context = softbuffer::Context::new(window.clone()).expect("softbuffer context");
        let surface = softbuffer::Surface::new(&context, window.clone()).expect("softbuffer surface");
        self.app.set_scale(window.scale_factor() as f32);
        window.request_redraw();
        self.gfx = Some(Gfx { window, surface });
    }

    fn window_event(&mut self, event_loop: &ActiveEventLoop, _id: WindowId, event: WindowEvent) {
        let Some(gfx) = self.gfx.as_mut() else { return };
        match event {
            WindowEvent::CloseRequested => event_loop.exit(),
            WindowEvent::Resized(_) => gfx.window.request_redraw(),
            WindowEvent::ScaleFactorChanged { scale_factor, .. } => {
                self.app.set_scale(scale_factor as f32);
                gfx.window.request_redraw();
            }
            WindowEvent::DroppedFile(path) => {
                self.app.open(&path);
                gfx.window.set_title(&self.app.title());
                gfx.window.request_redraw();
            }
            WindowEvent::MouseWheel { delta, .. } => {
                match delta {
                    MouseScrollDelta::LineDelta(_, y) => self.app.scroll_lines(-y * 3.0),
                    MouseScrollDelta::PixelDelta(p) => self.app.scroll_pixels(-p.y as f32),
                }
                gfx.window.request_redraw();
            }
            WindowEvent::KeyboardInput { event, .. } if event.state == ElementState::Pressed => {
                let handled = match event.logical_key {
                    Key::Named(NamedKey::PageDown) => {
                        self.app.scroll_pages(1.0);
                        true
                    }
                    Key::Named(NamedKey::PageUp) => {
                        self.app.scroll_pages(-1.0);
                        true
                    }
                    Key::Named(NamedKey::ArrowDown) => {
                        self.app.scroll_lines(1.0);
                        true
                    }
                    Key::Named(NamedKey::ArrowUp) => {
                        self.app.scroll_lines(-1.0);
                        true
                    }
                    Key::Named(NamedKey::Home) => {
                        self.app.scroll_to_start();
                        true
                    }
                    Key::Named(NamedKey::End) => {
                        self.app.scroll_to_end();
                        true
                    }
                    _ => false,
                };
                if handled {
                    gfx.window.request_redraw();
                }
            }
            WindowEvent::RedrawRequested => {
                let size = gfx.window.inner_size();
                let (Some(w), Some(h)) = (NonZeroU32::new(size.width), NonZeroU32::new(size.height)) else {
                    return; // minimised
                };
                if gfx.surface.resize(w, h).is_err() {
                    return;
                }
                let Ok(mut buffer) = gfx.surface.buffer_mut() else { return };
                {
                    let mut canvas = Canvas::new(&mut buffer, size.width, size.height);
                    self.app.render(&mut canvas);
                }
                let _ = buffer.present();
            }
            _ => {}
        }
    }
}

pub fn run(app: App) {
    let event_loop = EventLoop::new().expect("event loop");
    // Redraw only on events — no busy loop, near-zero idle CPU.
    event_loop.set_control_flow(ControlFlow::Wait);
    let mut runner = Runner { app, gfx: None };
    event_loop.run_app(&mut runner).expect("event loop run");
}

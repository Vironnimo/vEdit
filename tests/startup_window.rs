#![cfg(windows)]

use std::{ffi::c_void, sync::Arc};

use eframe::{egui, glow, glow::HasContext};
use egui_winit::winit::{
    platform::windows::EventLoopBuilderExtWindows,
    raw_window_handle::{HasWindowHandle, RawWindowHandle},
};

type SubclassProc =
    unsafe extern "system" fn(*mut c_void, u32, usize, isize, usize, usize) -> isize;

#[link(name = "comctl32")]
unsafe extern "system" {
    fn SetWindowSubclass(
        window: *mut c_void,
        callback: SubclassProc,
        id: usize,
        data: usize,
    ) -> i32;
    fn DefSubclassProc(window: *mut c_void, message: u32, wparam: usize, lparam: isize) -> isize;
    fn RemoveWindowSubclass(window: *mut c_void, callback: SubclassProc, id: usize) -> i32;
}

struct FirstShow {
    gl: Arc<glow::Context>,
    pixel: Option<[u8; 4]>,
    error: u32,
}

unsafe extern "system" fn observe_first_show(
    window: *mut c_void,
    message: u32,
    wparam: usize,
    lparam: isize,
    _id: usize,
    data: usize,
) -> isize {
    const WM_SHOWWINDOW: u32 = 0x0018;
    // This callback runs synchronously on the rendering thread, before Windows
    // exposes the window. An asynchronous screen capture can miss the flash.
    if message == WM_SHOWWINDOW && wparam != 0 {
        let probe = unsafe { &mut *(data as *mut FirstShow) };
        if probe.pixel.is_none() {
            let mut pixel = [0; 4];
            unsafe {
                probe.gl.read_buffer(glow::FRONT);
                probe.gl.read_pixels(
                    32,
                    32,
                    1,
                    1,
                    glow::RGBA,
                    glow::UNSIGNED_BYTE,
                    glow::PixelPackData::Slice(Some(&mut pixel)),
                );
                probe.error = probe.gl.get_error();
                probe.gl.read_buffer(glow::BACK);
            }
            probe.pixel = Some(pixel);
        }
    }
    unsafe { DefSubclassProc(window, message, wparam, lparam) }
}

struct StartupApp {
    frames: usize,
    window: *mut c_void,
}

impl eframe::App for StartupApp {
    fn ui(&mut self, ui: &mut egui::Ui, _frame: &mut eframe::Frame) {
        ui.painter()
            .rect_filled(ui.max_rect(), 0.0, egui::Color32::from_rgb(21, 19, 15));
        self.frames += 1;
        if self.frames >= 2 {
            ui.ctx().send_viewport_cmd(egui::ViewportCommand::Close);
        }
        ui.ctx().request_repaint();
    }
}

impl Drop for StartupApp {
    fn drop(&mut self) {
        // The probe outlives run_native, including the destruction of this app.
        unsafe { RemoveWindowSubclass(self.window, observe_first_show, 1) };
    }
}

#[test]
#[ignore = "requires native Windows desktop and OpenGL"]
fn first_visible_window_has_presented_pixels() {
    let fixture = std::path::PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .join(".tmp/startup-flash")
        .join(std::process::id().to_string());
    std::fs::create_dir_all(&fixture).unwrap();
    let options = eframe::NativeOptions {
        viewport: egui::ViewportBuilder::default()
            .with_inner_size([160.0, 120.0])
            .with_decorations(false),
        renderer: eframe::Renderer::Glow,
        persistence_path: Some(fixture.join("app.ron")),
        persist_window: false,
        event_loop_builder: Some(Box::new(|builder| {
            builder.with_any_thread(true);
        })),
        ..Default::default()
    };
    let mut probe = None;
    eframe::run_native(
        "vEdit startup regression",
        options,
        Box::new(|cc| {
            let RawWindowHandle::Win32(handle) = cc.window_handle()?.as_raw() else {
                panic!("expected a native Windows window");
            };
            assert_eq!(cc.winit_window().unwrap().is_visible(), Some(false));
            let state = probe.insert(Box::new(FirstShow {
                gl: cc.gl.clone().expect("the fixture uses OpenGL"),
                pixel: None,
                error: glow::NO_ERROR,
            }));
            let window = handle.hwnd.get() as *mut c_void;
            assert_ne!(
                unsafe {
                    SetWindowSubclass(
                        window,
                        observe_first_show,
                        1,
                        &mut **state as *mut _ as usize,
                    )
                },
                0
            );
            Ok(Box::new(StartupApp { frames: 0, window }))
        }),
    )
    .unwrap();
    let probe = probe.unwrap();
    assert_eq!(probe.error, glow::NO_ERROR, "front-buffer read failed");
    let pixel = probe.pixel.expect("the fixture window was never shown");
    for (actual, expected) in pixel[..3].iter().zip([21_u8, 19, 15]) {
        assert!(
            actual.abs_diff(expected) <= 1,
            "the first visible front buffer is {pixel:?}, expected the painted dark surface"
        );
    }
}

#![cfg_attr(not(debug_assertions), windows_subsystem = "windows")]

mod agent_marks;
mod app;
mod bookmarks;
#[allow(dead_code)]
mod cli;
mod design;
mod editor;
mod file_identity;
mod file_kind;
mod fs_watch;
mod git;
mod git_diff_view;
mod git_graph;
mod line_diff;
mod lsp;
mod markdown;
mod multicursor;
mod output_links;
mod palette;
mod paths;
mod process_liveness;
mod profile;
mod recovery;
mod render;
mod screen_jump;
mod search;
mod selection_scroll;
mod single_instance;
mod startup;
mod tasks;
mod text_statistics;
mod typography;
mod vim;
mod window_chrome;
mod workspace;

// Exercise the exact dependency fix in the normal vEdit regression suite.
#[cfg(test)]
#[path = "../vendor/winit/src/platform_impl/windows/dpi_change.rs"]
mod window_dpi_tests;

#[cfg(test)]
#[path = "../vendor/winit/src/platform_impl/windows/chrome_hit_test.rs"]
mod window_chrome_native_tests;

fn main() -> eframe::Result {
    let arguments = match startup::parse(std::env::args_os().skip(1)) {
        Ok(arguments) => arguments,
        Err(error) => {
            show_startup_error(error);
            return Ok(());
        }
    };
    if arguments.help {
        print!("{}", startup::HELP);
        return Ok(());
    }
    if arguments.version {
        println!("vEdit {}", env!("CARGO_PKG_VERSION"));
        return Ok(());
    }
    if let Err(error) =
        cli::validate_environment().and_then(|()| app::profile_migration::initialize())
    {
        show_startup_error(error);
        return Ok(());
    }
    let (paths, _single_instance) = match startup::prepare(arguments) {
        Ok(Some(launch)) => launch,
        Ok(None) => return Ok(()),
        Err(error) => {
            show_startup_error(error);
            return Ok(());
        }
    };
    let open_window = match startup::OpenWindow::mark() {
        Ok(window) => window,
        Err(error) => {
            show_startup_error(error);
            return Ok(());
        }
    };
    startup::recover_interrupted_windows();

    let options = eframe::NativeOptions {
        persistence_path: profile::persistence_path(),
        viewport: eframe::egui::ViewportBuilder::default()
            .with_title("vEdit")
            .with_icon(app_icon())
            .with_decorations(!cfg!(windows))
            .with_inner_size([1440.0, 920.0])
            .with_min_inner_size([500.0, 360.0])
            .with_app_id("vedit-editor"),
        ..Default::default()
    };

    let result = eframe::run_native(
        "vEdit",
        options,
        Box::new(move |cc| Ok(Box::new(app::VEditApp::new(cc, paths)))),
    );
    if result.is_ok() {
        open_window.closed();
    }
    result
}

fn show_startup_error(error: String) {
    let _ = rfd::MessageDialog::new()
        .set_title("vEdit")
        .set_description(error)
        .show();
}

fn app_icon() -> eframe::egui::IconData {
    const SIZE: u32 = 128;
    let mut rgba = Vec::with_capacity((SIZE * SIZE * 4) as usize);
    for y in 0..SIZE {
        for x in 0..SIZE {
            let mut color = [0.0_f32; 4];
            for (sx, sy) in [(0.25, 0.25), (0.75, 0.25), (0.25, 0.75), (0.75, 0.75)] {
                let dx = (x as f32 + sx - 64.0).abs();
                let dy = (y as f32 + sy - 64.0).abs();
                let rounded = (dx - 38.0).max(0.0).hypot((dy - 38.0).max(0.0));
                let diamond = dx + dy;
                let sample = if rounded > 24.0 {
                    [0.0, 0.0, 0.0, 0.0]
                } else if (diamond - 44.0).abs() < 2.5 || diamond < 12.0 {
                    palette::ACCENT.to_array().map(f32::from)
                } else {
                    palette::BG.to_array().map(f32::from)
                };
                for i in 0..4 {
                    color[i] += sample[i] * 0.25;
                }
            }
            rgba.extend(color.map(|c| c.round() as u8));
        }
    }
    eframe::egui::IconData {
        rgba,
        width: SIZE,
        height: SIZE,
    }
}

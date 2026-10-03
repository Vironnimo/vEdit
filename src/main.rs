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
mod git;
mod git_diff_view;
mod git_graph;
mod lsp;
mod markdown;
mod paths;
mod recovery;
mod render;
mod screen_jump;
mod search;
mod selection_scroll;
mod single_instance;
mod tasks;
mod typography;
mod vim;
mod workspace;

// Exercise the exact dependency fix in the normal Morrow regression suite.
#[cfg(test)]
#[path = "../vendor/winit/src/platform_impl/windows/dpi_change.rs"]
mod window_dpi_tests;

fn main() -> eframe::Result {
    let paths: Vec<_> = std::env::args_os()
        .skip(1)
        .map(std::path::PathBuf::from)
        .collect();
    match cli::forward_paths(paths) {
        Ok(Some(_)) => return Ok(()),
        Ok(None) => {}
        Err(error) => {
            let _ = rfd::MessageDialog::new()
                .set_title("Morrow")
                .set_description(error)
                .show();
            return Ok(());
        }
    }
    let _single_instance = match single_instance::acquire() {
        Ok(Some(guard)) => Some(guard),
        Ok(None) => return Ok(()),
        Err(error) => {
            let _ = rfd::MessageDialog::new()
                .set_title("Morrow")
                .set_description(format!(
                    "Morrow could not verify that this is the only open window.\n\n{error}"
                ))
                .show();
            return Ok(());
        }
    };

    let options = eframe::NativeOptions {
        viewport: eframe::egui::ViewportBuilder::default()
            .with_title("Morrow")
            .with_icon(app_icon())
            .with_inner_size([1440.0, 920.0])
            .with_min_inner_size([960.0, 640.0])
            .with_app_id("morrow-editor"),
        ..Default::default()
    };

    eframe::run_native(
        "Morrow",
        options,
        Box::new(|cc| Ok(Box::new(app::MorrowApp::new(cc)))),
    )
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
                    [119.0, 218.0, 192.0, 255.0]
                } else {
                    [21.0, 26.0, 34.0, 255.0]
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

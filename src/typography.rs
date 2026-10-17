//! Upright code faces, independent of the UI's proportional font.
use crate::cli::EditorFont;
use eframe::egui::epaint::text::{FontTweak, HintingTarget, SmoothHinting};
use eframe::egui::{FontData, FontDefinitions, FontFamily};
use std::{
    collections::HashMap,
    fs,
    path::{Path, PathBuf},
    sync::{Mutex, OnceLock, PoisonError},
};

/// Segoe UI and Segoe UI Symbol have no CJK ideographs or kana. The first
/// installed face from this list keeps such text readable instead of boxes.
const CJK_FALLBACKS: [&str; 2] = ["msyh.ttc", "YuGothR.ttc"];

impl EditorFont {
    pub(crate) fn label(self) -> &'static str {
        match self {
            Self::JetBrainsMono => "JetBrains Mono",
            Self::Consolas => "Consolas",
            Self::CascadiaMono => "Cascadia Mono",
        }
    }
}

pub(crate) fn fonts(choice: EditorFont) -> FontDefinitions {
    let windows = std::env::var_os("WINDIR").unwrap_or_else(|| "C:\\Windows".into());
    fonts_from(Path::new(&windows).join("Fonts").as_path(), choice)
}

pub(crate) fn tab_family(width: usize) -> FontFamily {
    FontFamily::Name(format!("vedit-tabs-{}", width.clamp(1, 8)).into())
}

fn fonts_from(directory: &Path, choice: EditorFont) -> FontDefinitions {
    let mut fonts = FontDefinitions::default();
    // The bundled Light face matches the visual weight of native editors:
    // egui's dark-mode coverage boost makes the Regular face look bold.
    let bundled =
        FontData::from_static(include_bytes!("../assets/fonts/JetBrainsMonoNL-Light.ttf"));
    let selected = match choice {
        EditorFont::JetBrainsMono => None,
        EditorFont::Consolas => Some("consola.ttf"),
        EditorFont::CascadiaMono => Some("CascadiaMono.ttf"),
    }
    .and_then(|name| system_font(directory, name))
    .map(FontData::from_static)
    .unwrap_or(bundled)
    .tweak(FontTweak {
        // Light grayscale hinting keeps small regular glyphs crisp without
        // the horizontal stem widening used by the default rendering target.
        hinting_target: HintingTarget::Smooth(SmoothHinting {
            light: true,
            symmetric_rendering: false,
            ..Default::default()
        }),
        ..Default::default()
    });
    fonts.font_data.insert("vedit-code".into(), selected.into());
    fonts
        .families
        .entry(FontFamily::Monospace)
        .or_default()
        .insert(0, "vedit-code".into());
    // These families are only used for tab glyphs, so each document can have
    // its own tab width without replacing text or rebuilding the font atlas.
    for width in 1..=8 {
        let name = format!("vedit-tabs-{width}");
        let mut data = (*fonts.font_data["vedit-code"]).clone();
        data.tweak.tab_size = width as f32;
        fonts.font_data.insert(name.clone(), data.into());
        fonts.families.insert(tab_family(width), vec![name]);
    }

    for (name, file) in [
        ("vedit-ui", "segoeui.ttf"),
        ("vedit-symbols", "seguisym.ttf"),
    ] {
        if let Some(bytes) = system_font(directory, file) {
            fonts
                .font_data
                .insert(name.into(), FontData::from_static(bytes).into());
            if name == "vedit-symbols" {
                for family in [FontFamily::Proportional, FontFamily::Monospace] {
                    fonts.families.entry(family).or_default().push(name.into());
                }
            } else {
                fonts
                    .families
                    .entry(FontFamily::Proportional)
                    .or_default()
                    .insert(0, name.into());
            }
        }
    }
    // After every other fallback, so its larger glyph metrics only apply to
    // characters that no earlier face can draw. Collections use their first face.
    if let Some(bytes) = CJK_FALLBACKS
        .into_iter()
        .find_map(|file| system_font(directory, file))
    {
        fonts
            .font_data
            .insert("vedit-cjk".into(), FontData::from_static(bytes).into());
        for family in [FontFamily::Proportional, FontFamily::Monospace] {
            fonts
                .families
                .entry(family)
                .or_default()
                .push("vedit-cjk".into());
        }
    }
    if let Some(bytes) = system_font(directory, "seguisb.ttf") {
        fonts
            .font_data
            .insert("vedit-semibold".into(), FontData::from_static(bytes).into());
        let mut family = vec!["vedit-semibold".into()];
        family.extend(
            fonts
                .families
                .get(&FontFamily::Proportional)
                .cloned()
                .unwrap_or_default(),
        );
        fonts
            .families
            .insert(FontFamily::Name("vedit-semibold".into()), family);
    }
    fonts
}

/// Read each system font once per process. The CJK collection alone is about
/// 20 MB, and egui copies owned font bytes whenever it rebuilds its font atlas
/// (font choice changes, atlas growth); borrowed bytes are shared instead.
fn system_font(directory: &Path, file: &str) -> Option<&'static [u8]> {
    type Cache = Mutex<HashMap<PathBuf, Option<&'static [u8]>>>;
    static FILES: OnceLock<Cache> = OnceLock::new();
    let mut files = FILES
        .get_or_init(Default::default)
        .lock()
        .unwrap_or_else(PoisonError::into_inner);
    *files
        .entry(directory.join(file))
        .or_insert_with_key(|path| {
            fs::read(path)
                .ok()
                .map(|bytes| &*Box::leak(bytes.into_boxed_slice()))
        })
}

#[cfg(test)]
mod tests {
    use super::*;
    use eframe::egui;

    fn windows_fonts() -> PathBuf {
        let windows = std::env::var_os("WINDIR").unwrap_or_else(|| "C:\\Windows".into());
        Path::new(&windows).join("Fonts")
    }

    #[test]
    fn installed_cjk_face_follows_symbols_in_both_families_and_draws_ideographs() {
        let directory = windows_fonts();
        if !CJK_FALLBACKS
            .iter()
            .any(|file| directory.join(file).is_file())
        {
            // Nothing to verify on a system without these optional faces.
            return;
        }
        let fonts = fonts_from(&directory, EditorFont::Consolas);
        for family in [FontFamily::Proportional, FontFamily::Monospace] {
            let names = &fonts.families[&family];
            assert_eq!(names.last().map(String::as_str), Some("vedit-cjk"));
            if let Some(symbols) = names.iter().position(|name| name == "vedit-symbols") {
                assert!(symbols < names.len() - 1);
            }
        }
        let semibold = &fonts.families[&FontFamily::Name("vedit-semibold".into())];
        assert!(semibold.iter().any(|name| name == "vedit-cjk"));

        let ctx = egui::Context::default();
        ctx.set_fonts(fonts);
        let mut drawable = Vec::new();
        ctx.run_ui(egui::RawInput::default(), |ui| {
            drawable = [
                egui::FontId::proportional(14.0),
                egui::FontId::monospace(14.0),
            ]
            .into_iter()
            .map(|font| ui.fonts_mut(|fonts| fonts.has_glyphs(&font, "中文 日本語 かな")))
            .collect();
        })
        .drop_without_applying_deltas();
        assert_eq!(drawable, [true, true]);
    }

    #[test]
    fn system_fonts_are_read_once_and_shared_by_rebuilt_definitions() {
        let directory = windows_fonts();
        let first = fonts_from(&directory, EditorFont::Consolas);
        let second = fonts_from(&directory, EditorFont::Consolas);
        for name in ["vedit-code", "vedit-ui", "vedit-cjk"] {
            let (Some(first), Some(second)) =
                (first.font_data.get(name), second.font_data.get(name))
            else {
                continue;
            };
            assert!(matches!(first.font, std::borrow::Cow::Borrowed(_)));
            assert!(std::ptr::eq(first.font.as_ptr(), second.font.as_ptr()));
        }
    }

    #[test]
    fn missing_system_fonts_retain_bundled_code_face_and_unicode_fallbacks() {
        let missing = std::env::temp_dir().join("vedit-no-system-fonts-typography");
        for choice in [
            EditorFont::JetBrainsMono,
            EditorFont::Consolas,
            EditorFont::CascadiaMono,
        ] {
            let fonts = fonts_from(&missing, choice);
            let code = &fonts.font_data["vedit-code"];
            assert_eq!(
                code.font.as_ref(),
                include_bytes!("../assets/fonts/JetBrainsMonoNL-Light.ttf")
            );
            let family = &fonts.families[&FontFamily::Monospace];
            assert_eq!(family[0], "vedit-code");
            assert!(family.len() > 1);
        }
    }
}

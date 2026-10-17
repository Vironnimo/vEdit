//! Upright code faces, independent of the UI's proportional font.
use crate::cli::EditorFont;
use eframe::egui::epaint::text::{FontTweak, HintingTarget, SmoothHinting};
use eframe::egui::{FontData, FontDefinitions, FontFamily};
use std::{fs, path::Path};

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
    .and_then(|name| fs::read(directory.join(name)).ok())
    .map(FontData::from_owned)
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
        if let Ok(bytes) = fs::read(directory.join(file)) {
            fonts
                .font_data
                .insert(name.into(), FontData::from_owned(bytes).into());
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
    if let Ok(bytes) = fs::read(directory.join("seguisb.ttf")) {
        fonts
            .font_data
            .insert("vedit-semibold".into(), FontData::from_owned(bytes).into());
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

#[cfg(test)]
mod tests {
    use super::*;

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

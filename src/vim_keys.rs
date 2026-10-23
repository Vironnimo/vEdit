//! Per-chord ownership between source-editor commands and supported Vim controls.
//! The shortcut registry remains the owner of application bindings and conflicts.
use super::{Action, Bindings, Scope, Stroke, strokes};
use crate::{cli::VimKeyTarget, vim::VimMode};
use eframe::egui::{self, Key, RichText};
use serde::{Deserialize, Serialize};
use std::collections::BTreeMap;

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize)]
#[serde(rename_all = "snake_case")]
pub(crate) enum Target {
    #[default]
    Default,
    Editor,
    Vim,
}

impl From<VimKeyTarget> for Target {
    fn from(value: VimKeyTarget) -> Self {
        match value {
            VimKeyTarget::Editor => Self::Editor,
            VimKeyTarget::Vim => Self::Vim,
        }
    }
}

impl Target {
    fn label(self) -> &'static str {
        match self {
            Self::Default => "Default",
            Self::Editor => "Editor",
            Self::Vim => "Vim",
        }
    }
}

/// Only explicit ownership choices are persisted. Missing keys retain the
/// Windows-oriented baseline and yield to custom application shortcuts.
#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(transparent)]
pub(crate) struct Preferences(BTreeMap<String, VimKeyTarget>);

struct Control {
    key: Key,
    chord: &'static str,
    default: Target,
    modes: &'static [&'static str],
    effect: &'static str,
}

const MODAL: &[&str] = &["normal", "visual", "visualline", "visualblock"];
const NORMAL: &[&str] = &["normal"];
const INSERT: &[&str] = &["insert"];
const ALL_MODES: &[&str] = &["normal", "insert", "visual", "visualline", "visualblock"];

const CONTROLS: &[Control] = &[
    Control {
        key: Key::C,
        chord: "Ctrl+C",
        default: Target::Editor,
        modes: ALL_MODES,
        effect: "Cancel the current Vim command or selection; leave Insert",
    },
    Control {
        key: Key::X,
        chord: "Ctrl+X",
        default: Target::Editor,
        modes: NORMAL,
        effect: "Subtract from the number at or after the cursor",
    },
    Control {
        key: Key::V,
        chord: "Ctrl+V",
        default: Target::Editor,
        modes: MODAL,
        effect: "Start or leave a Visual Block selection",
    },
    Control {
        key: Key::Q,
        chord: "Ctrl+Q",
        default: Target::Vim,
        modes: MODAL,
        effect: "Start or leave a Visual Block selection",
    },
    Control {
        key: Key::A,
        chord: "Ctrl+A",
        default: Target::Editor,
        modes: NORMAL,
        effect: "Add to the number at or after the cursor",
    },
    Control {
        key: Key::F,
        chord: "Ctrl+F",
        default: Target::Vim,
        modes: MODAL,
        effect: "Move one page down",
    },
    Control {
        key: Key::B,
        chord: "Ctrl+B",
        default: Target::Vim,
        modes: MODAL,
        effect: "Move one page up",
    },
    Control {
        key: Key::D,
        chord: "Ctrl+D",
        default: Target::Editor,
        modes: MODAL,
        effect: "Move half a page down",
    },
    Control {
        key: Key::U,
        chord: "Ctrl+U",
        default: Target::Vim,
        modes: MODAL,
        effect: "Move half a page up",
    },
    Control {
        key: Key::O,
        chord: "Ctrl+O",
        default: Target::Vim,
        modes: NORMAL,
        effect: "Go back in this Pane's jump history",
    },
    Control {
        key: Key::I,
        chord: "Ctrl+I",
        default: Target::Vim,
        modes: NORMAL,
        effect: "Go forward in this Pane's jump history",
    },
    Control {
        key: Key::R,
        chord: "Ctrl+R",
        default: Target::Vim,
        modes: NORMAL,
        effect: "Redo the last undone change",
    },
    Control {
        key: Key::N,
        chord: "Ctrl+N",
        default: Target::Vim,
        modes: INSERT,
        effect: "Show completions or select the next completion",
    },
    Control {
        key: Key::P,
        chord: "Ctrl+P",
        default: Target::Editor,
        modes: INSERT,
        effect: "Show completions or select the previous completion",
    },
];

fn control(key: Key) -> Option<&'static Control> {
    CONTROLS.iter().find(|control| control.key == key)
}

impl Control {
    fn supports(&self, mode: VimMode) -> bool {
        match self.modes {
            NORMAL => mode == VimMode::Normal,
            INSERT => mode == VimMode::Insert,
            MODAL => mode != VimMode::Insert,
            _ => true,
        }
    }

    fn modes_label(&self) -> &'static str {
        match self.modes {
            NORMAL => "Normal",
            INSERT => "Insert",
            MODAL => "Normal & Visual modes",
            _ => "All Vim modes",
        }
    }

    fn custom_editor_binding(&self, bindings: &Bindings) -> bool {
        Action::ALL.iter().any(|&action| {
            bindings.contains_key(action.id()) && self.editor_binding(bindings, action)
        })
    }

    fn editor_binding(&self, bindings: &Bindings, action: Action) -> bool {
        matches!(
            action.scope(),
            Scope::Global | Scope::Source | Scope::SourceEditing
        ) && strokes(bindings, action).iter().any(|stroke| {
            *stroke
                == Stroke {
                    key: self.key,
                    ctrl: true,
                    alt: false,
                    shift: false,
                }
        })
    }

    fn editor_effect(&self, bindings: &Bindings) -> String {
        match self.key {
            Key::C => "Copy selection or current line".into(),
            Key::X => "Cut selection or current line".into(),
            Key::V => "Paste from the system clipboard".into(),
            Key::A => "Select all".into(),
            _ => {
                let labels: Vec<_> = Action::ALL
                    .iter()
                    .copied()
                    .filter(|&action| self.editor_binding(bindings, action))
                    .map(Action::label)
                    .collect();
                if labels.is_empty() {
                    "No editor command assigned".into()
                } else {
                    labels.join(" / ")
                }
            }
        }
    }
}

impl Preferences {
    pub(crate) fn preference(&self, key: Key) -> Target {
        control(key)
            .and_then(|control| self.0.get(control.chord))
            .copied()
            .map(Target::from)
            .unwrap_or_default()
    }

    pub(crate) fn set(&mut self, key: Key, target: Target) -> bool {
        let Some(control) = control(key) else {
            return false;
        };
        let before = self.preference(key);
        match target {
            Target::Default => {
                self.0.remove(control.chord);
            }
            Target::Editor => {
                self.0.insert(control.chord.into(), VimKeyTarget::Editor);
            }
            Target::Vim => {
                self.0.insert(control.chord.into(), VimKeyTarget::Vim);
            }
        }
        before != target
    }

    /// None means that this key has no implemented Vim operation in this mode.
    /// Callers retain normal editor routing outside this capability's modes.
    pub(crate) fn target(
        &self,
        key: Key,
        mode: VimMode,
        custom_editor_binding: bool,
    ) -> Option<Target> {
        let control = control(key).filter(|control| control.supports(mode))?;
        Some(self.resolved(control, custom_editor_binding))
    }

    fn resolved(&self, control: &Control, custom_editor_binding: bool) -> Target {
        match self.preference(control.key) {
            Target::Default if custom_editor_binding => Target::Editor,
            Target::Default => control.default,
            explicit => explicit,
        }
    }

    pub(crate) fn overrides(&self) -> &BTreeMap<String, VimKeyTarget> {
        &self.0
    }

    /// Settings patches change only supplied keys; null restores their defaults.
    pub(crate) fn apply_overrides(
        &mut self,
        patch: &BTreeMap<String, Option<VimKeyTarget>>,
    ) -> bool {
        let before = self.0.clone();
        for (key, target) in patch {
            if let Some(control) = CONTROLS.iter().find(|control| control.chord == key) {
                self.set(control.key, target.map(Target::from).unwrap_or_default());
            }
        }
        before != self.0
    }

    pub(crate) fn catalog(&self, bindings: &Bindings) -> serde_json::Value {
        serde_json::Value::Array(CONTROLS.iter().map(|control| serde_json::json!({
            "key": control.chord,
            "target": self.preference(control.key),
            "default": control.default,
            "effective": self.resolved(control, control.custom_editor_binding(bindings)),
            "modes": control.modes,
            "outside_modes": "editor",
            "scope": "source_editor",
            "vim_effect": control.effect,
            "editor_effect": control.editor_effect(bindings),
        })).collect())
    }
}

pub(crate) fn settings_ui(
    ui: &mut egui::Ui,
    preferences: &mut Preferences,
    bindings: &Bindings,
) -> bool {
    use crate::app::{MUTED, editor_preferences::preference_heading};
    let mut changed = false;
    preference_heading(ui, "CONTROL KEYS");
    ui.label("Choose whether each Ctrl shortcut goes to Vim or the editor.");
    ui.label(RichText::new("Applies in the source editor, with Vim on, in the modes shown below. Other modes, input fields and MULTI keep their editor shortcuts.").small().color(MUTED));
    ui.label(RichText::new("Default keeps Windows clipboard shortcuts and uses Ctrl+Q for Visual Block. Custom Keyboard bindings take priority over defaults; choosing Vim here takes priority over those bindings.").small().color(MUTED));
    ui.horizontal(|ui| {
        if ui
            .add_enabled(
                !preferences.overrides().is_empty(),
                egui::Button::new("Reset all Vim keys"),
            )
            .clicked()
        {
            preferences.0.clear();
            changed = true;
        }
        ui.label(
            RichText::new("Changes apply immediately")
                .small()
                .color(MUTED),
        );
    });
    let query_id = ui.id().with("vim_control_key_filter");
    let mut query = ui
        .ctx()
        .data_mut(|data| data.get_temp::<String>(query_id).unwrap_or_default());
    ui.add(
        egui::TextEdit::singleline(&mut query)
            .hint_text("Filter Vim keys or actions…")
            .desired_width(f32::INFINITY),
    );
    ui.ctx()
        .data_mut(|data| data.insert_temp(query_id, query.clone()));
    let query = query.trim().to_lowercase();
    let mut shown = 0;
    for control in CONTROLS {
        let editor_effect = control.editor_effect(bindings);
        let searchable = format!(
            "{} {} {} {}",
            control.chord,
            control.effect,
            editor_effect,
            control.modes_label()
        )
        .to_lowercase();
        if !query.is_empty() && !searchable.contains(&query) {
            continue;
        }
        shown += 1;
        ui.push_id(control.chord, |ui| {
            ui.separator();
            let mut target = preferences.preference(control.key);
            let default = if control.custom_editor_binding(bindings) {
                Target::Editor
            } else {
                control.default
            };
            let default_label = format!("Default ({})", default.label());
            ui.horizontal_wrapped(|ui| {
                let key_label = ui.label(RichText::new(control.chord).monospace().strong());
                egui::ComboBox::from_id_salt("target")
                    .selected_text(if target == Target::Default {
                        default_label.as_str()
                    } else {
                        target.label()
                    })
                    .width(140.0)
                    .show_ui(ui, |ui| {
                        ui.selectable_value(&mut target, Target::Default, &default_label);
                        ui.selectable_value(&mut target, Target::Editor, "Editor");
                        ui.selectable_value(&mut target, Target::Vim, "Vim");
                    })
                    .response
                    .labelled_by(key_label.id);
                if ui
                    .add_enabled(target != Target::Default, egui::Button::new("Reset"))
                    .on_hover_text("Restore the default for this shortcut")
                    .clicked()
                {
                    target = Target::Default;
                }
                ui.label(RichText::new(control.modes_label()).small().color(MUTED));
            });
            changed |= preferences.set(control.key, target);
            ui.label(RichText::new(format!("Vim: {}", control.effect)).small());
            ui.label(
                RichText::new(format!("Editor: {editor_effect}"))
                    .small()
                    .color(MUTED),
            );
        });
    }
    if shown == 0 {
        ui.label("No matching Vim keys.");
    }
    changed
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn windows_defaults_are_mode_aware_and_explicit_targets_override_bindings() {
        use VimMode::{Insert, Normal, Visual, VisualBlock, VisualLine};
        let mut preferences = Preferences::default();
        let modes = [Normal, Insert, Visual, VisualLine, VisualBlock];
        let modal = [Normal, Visual, VisualLine, VisualBlock];
        for (key, default, supported) in [
            (Key::C, Target::Editor, modes.as_slice()),
            (Key::X, Target::Editor, &[Normal]),
            (Key::V, Target::Editor, modal.as_slice()),
            (Key::Q, Target::Vim, modal.as_slice()),
            (Key::A, Target::Editor, &[Normal]),
            (Key::F, Target::Vim, modal.as_slice()),
            (Key::B, Target::Vim, modal.as_slice()),
            (Key::D, Target::Editor, modal.as_slice()),
            (Key::U, Target::Vim, modal.as_slice()),
            (Key::O, Target::Vim, &[Normal]),
            (Key::I, Target::Vim, &[Normal]),
            (Key::R, Target::Vim, &[Normal]),
            (Key::N, Target::Vim, &[Insert]),
            (Key::P, Target::Editor, &[Insert]),
        ] {
            for mode in modes {
                let supported = supported.contains(&mode);
                assert_eq!(
                    preferences.target(key, mode, false),
                    supported.then_some(default),
                    "{key:?} {mode:?}",
                );
                assert_eq!(
                    preferences.target(key, mode, true),
                    supported.then_some(Target::Editor)
                );
                preferences.set(key, Target::Vim);
                assert_eq!(
                    preferences.target(key, mode, true),
                    supported.then_some(Target::Vim)
                );
                preferences.set(key, Target::Editor);
                assert_eq!(
                    preferences.target(key, mode, false),
                    supported.then_some(Target::Editor)
                );
                preferences.set(key, Target::Default);
            }
        }
        assert_eq!(preferences.target(Key::S, VimMode::Normal, false), None);
        assert!(!preferences.set(Key::S, Target::Vim));
        assert!(preferences.overrides().is_empty());
    }

    #[test]
    fn saved_choices_and_partial_patches_keep_defaults_and_unrelated_keys() {
        let mut preferences: Preferences =
            serde_json::from_str(r#"{"Ctrl+C":"vim","Ctrl+F":"editor"}"#).unwrap();
        preferences.apply_overrides(&BTreeMap::from([
            ("Ctrl+C".into(), None),
            ("Ctrl+V".into(), Some(VimKeyTarget::Vim)),
        ]));
        assert_eq!(
            serde_json::to_value(&preferences).unwrap(),
            serde_json::json!({"Ctrl+F":"editor","Ctrl+V":"vim"})
        );
        assert_eq!(preferences.preference(Key::C), Target::Default);
        assert_eq!(preferences.preference(Key::V), Target::Vim);
        assert!(!preferences.apply_overrides(&BTreeMap::new()));
        assert_eq!(
            serde_json::from_str::<Preferences>("{}").unwrap(),
            Preferences::default()
        );
        let mut bindings = Bindings::new();
        bindings.insert(Action::Save.id().into(), Some("Ctrl+Q".into()));
        let catalog = preferences.catalog(&bindings);
        let row = catalog
            .as_array()
            .unwrap()
            .iter()
            .find(|row| row["key"] == "Ctrl+Q")
            .unwrap();
        assert_eq!(row["effective"], "editor");
        assert_eq!(row["default"], "vim");
        assert_eq!(row["editor_effect"], Action::Save.label());
        let mut keys: Vec<_> = catalog
            .as_array()
            .unwrap()
            .iter()
            .map(|row| row["key"].as_str().unwrap())
            .collect();
        keys.sort_unstable();
        let mut accepted = crate::cli::VIM_CONTROL_KEYS.to_vec();
        accepted.sort_unstable();
        assert_eq!(keys, accepted);
    }

    #[test]
    fn settings_filter_selects_and_resets_a_key_without_touching_other_overrides() {
        use egui::accesskit::Role;
        use egui_kittest::{Harness, kittest::Queryable};
        let mut preferences = Preferences::default();
        preferences.set(Key::V, Target::Vim);
        let mut screen = Harness::builder().with_size([680.0, 560.0]).build_ui_state(
            |ui, preferences: &mut Preferences| {
                settings_ui(ui, preferences, &Bindings::new());
            },
            preferences,
        );
        screen.get_by_role(egui::accesskit::Role::TextInput).click();
        screen.run_steps(2);
        screen
            .get_by_role(egui::accesskit::Role::TextInput)
            .type_text("Ctrl+C");
        screen.run_steps(2);
        assert_eq!(
            screen.get_by_role(egui::accesskit::Role::TextInput).value(),
            Some("Ctrl+C".into())
        );
        assert_eq!(
            screen
                .get_by_role_and_label(Role::ComboBox, "Ctrl+C")
                .value(),
            Some("Default (Editor)".into())
        );
        assert!(
            screen
                .query_by_role_and_label(Role::ComboBox, "Ctrl+V")
                .is_none()
        );
        screen
            .get_by_role_and_label(Role::ComboBox, "Ctrl+C")
            .click();
        screen.run_steps(2);
        screen.get_by_label("Vim").click();
        screen.run_steps(2);
        assert_eq!(screen.state().preference(Key::C), Target::Vim);
        assert_eq!(screen.state().preference(Key::V), Target::Vim);
        screen.get_by_label("Reset").click();
        screen.run_steps(2);
        assert_eq!(screen.state().preference(Key::C), Target::Default);
        screen.get_by_label("Reset all Vim keys").click();
        screen.run_steps(2);
        assert!(screen.state().overrides().is_empty());
    }
}

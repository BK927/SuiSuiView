use super::commands::shortcut_from_input_event;
use super::settings::{grid_label_with_help, setting_group};
use super::ui::{dialog, theme};
use super::SuiSuiViewApp;
use crate::core::state::{
    default_key_bindings, default_mouse_bindings, AppSettings, CommandId, KeyBinding, KeyCode,
    KeyShortcut, LargeImageAnchor, MouseBinding, MouseGesture, WheelMode,
};
use eframe::egui::{self, RichText};

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(in crate::app) struct ShortcutCapture {
    command: CommandId,
    replace_index: Option<usize>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(in crate::app) struct ShortcutConflict {
    command: CommandId,
    shortcut: KeyShortcut,
    existing_command: CommandId,
    existing_index: usize,
    replace_index: Option<usize>,
}

impl SuiSuiViewApp {
    pub(in crate::app) fn show_keyboard_settings(
        &mut self,
        ctx: &egui::Context,
        ui: &mut egui::Ui,
        draft: &mut AppSettings,
        changed: &mut bool,
    ) {
        self.handle_shortcut_capture(ctx, draft, changed);
        self.show_shortcut_conflict(ui, draft, changed);

        setting_group(
            ui,
            "단축키",
            "단축키를 먼저 보고 어떤 기능에 연결할지 고릅니다. 새 키가 이미 쓰이는 경우 기존 행에서 제거한 뒤 옮길 수 있습니다.",
            |ui| {
                ui.horizontal_wrapped(|ui| {
                    ui.label("새 단축키 동작");
                    if let Some(command) =
                        command_combo(ui, "new_shortcut_command", self.shortcut_new_command, 220.0)
                    {
                        self.shortcut_new_command = command;
                    }
                    if ui.button("단축키 추가").clicked() {
                        self.shortcut_capture = Some(ShortcutCapture {
                            command: self.shortcut_new_command,
                            replace_index: None,
                        });
                        self.shortcut_conflict = None;
                    }
                    if ui.button("전체 기본값 초기화").clicked() {
                        draft.key_bindings = default_key_bindings();
                        self.shortcut_capture = None;
                        self.shortcut_conflict = None;
                        *changed = true;
                    }
                    if self.shortcut_capture.is_some() {
                        ui.label(
                            RichText::new("키 입력 대기 중... Esc 취소, Delete/Backspace 삭제")
                                .color(theme::ACCENT),
                        );
                    }
                });
                ui.add_space(8.0);
                self.shortcut_binding_table(ui, draft, changed);
            },
        );
    }

    fn shortcut_binding_table(
        &mut self,
        ui: &mut egui::Ui,
        draft: &mut AppSettings,
        changed: &mut bool,
    ) {
        if draft.key_bindings.is_empty() {
            dialog::setting_card(ui, |ui| {
                ui.label(RichText::new("등록된 단축키 없음").color(theme::TEXT_MUTED));
            });
            return;
        }

        let mut rows: Vec<(usize, KeyBinding)> =
            draft.key_bindings.iter().cloned().enumerate().collect();
        rows.sort_by_cached_key(|(_, binding)| shortcut_sort_key(binding));

        let mut remove_index = None;
        dialog::setting_card(ui, |ui| {
            egui::Grid::new("settings_key_binding_grid")
                .num_columns(4)
                .spacing([12.0, 8.0])
                .striped(true)
                .show(ui, |ui| {
                    ui.label(RichText::new("단축키").strong().color(theme::TEXT_MUTED));
                    ui.label(RichText::new("기능").strong().color(theme::TEXT_MUTED));
                    ui.label("");
                    ui.label("");
                    ui.end_row();

                    for (index, binding) in rows {
                        ui.label(
                            RichText::new(binding.shortcut.label())
                                .monospace()
                                .color(theme::TEXT_PRIMARY),
                        );
                        if let Some(command) = command_combo(
                            ui,
                            ("key_binding_command", index),
                            binding.command,
                            260.0,
                        ) {
                            if let Some(existing) = draft.key_bindings.get_mut(index) {
                                existing.command = command;
                                *changed = true;
                            }
                        }
                        if ui.small_button("키 변경").clicked() {
                            self.shortcut_capture = Some(ShortcutCapture {
                                command: binding.command,
                                replace_index: Some(index),
                            });
                            self.shortcut_conflict = None;
                        }
                        ui.horizontal(|ui| {
                            let default_command = default_command_for_shortcut(binding.shortcut);
                            let reset = ui
                                .add_enabled(default_command.is_some(), egui::Button::new("기본값"))
                                .on_disabled_hover_text("기본 단축키에 없는 사용자 추가 행입니다.");
                            if reset.clicked() {
                                if let Some(command) = default_command {
                                    if let Some(existing) = draft.key_bindings.get_mut(index) {
                                        existing.command = command;
                                        *changed = true;
                                    }
                                }
                            }
                            if ui.small_button("삭제").clicked() {
                                remove_index = Some(index);
                            }
                        });
                        ui.end_row();
                    }
                });
        });

        if let Some(index) = remove_index {
            draft.key_bindings.remove(index);
            self.shortcut_capture = None;
            self.shortcut_conflict = None;
            *changed = true;
        }
    }

    fn handle_shortcut_capture(
        &mut self,
        ctx: &egui::Context,
        draft: &mut AppSettings,
        changed: &mut bool,
    ) {
        let Some(capture) = self.shortcut_capture else {
            return;
        };
        let events = ctx.input(|input| input.events.clone());
        for event in events {
            if let egui::Event::Key {
                key,
                pressed: true,
                modifiers,
                ..
            } = event
            {
                if key == egui::Key::Escape {
                    self.shortcut_capture = None;
                    return;
                }
                if matches!(key, egui::Key::Delete | egui::Key::Backspace)
                    && capture.replace_index.is_some()
                {
                    if let Some(index) = capture.replace_index {
                        if index < draft.key_bindings.len() {
                            draft.key_bindings.remove(index);
                            *changed = true;
                        }
                    }
                    self.shortcut_capture = None;
                    return;
                }
                let key_event = egui::Event::Key {
                    key,
                    physical_key: None,
                    pressed: true,
                    repeat: false,
                    modifiers,
                };
                let Some(shortcut) = shortcut_from_input_event(&key_event, modifiers) else {
                    continue;
                };
                self.capture_shortcut(draft, changed, capture, shortcut);
                return;
            }
            if let egui::Event::Text(text) = event {
                if text == "*" {
                    self.capture_shortcut(
                        draft,
                        changed,
                        capture,
                        KeyShortcut::new(KeyCode::Asterisk),
                    );
                    return;
                }
            }
        }
    }

    fn capture_shortcut(
        &mut self,
        draft: &mut AppSettings,
        changed: &mut bool,
        capture: ShortcutCapture,
        shortcut: KeyShortcut,
    ) {
        if let Some((index, existing)) = draft
            .key_bindings
            .iter()
            .enumerate()
            .find(|(index, binding)| {
                binding.shortcut == shortcut && Some(*index) != capture.replace_index
            })
            .map(|(index, binding)| (index, binding.command))
        {
            self.shortcut_conflict = Some(ShortcutConflict {
                command: capture.command,
                shortcut,
                existing_command: existing,
                existing_index: index,
                replace_index: capture.replace_index,
            });
            self.shortcut_capture = None;
            return;
        }

        set_shortcut_binding(
            &mut draft.key_bindings,
            capture.command,
            capture.replace_index,
            shortcut,
        );
        self.shortcut_capture = None;
        self.shortcut_conflict = None;
        *changed = true;
    }

    fn show_shortcut_conflict(
        &mut self,
        ui: &mut egui::Ui,
        draft: &mut AppSettings,
        changed: &mut bool,
    ) {
        let Some(conflict) = self.shortcut_conflict else {
            return;
        };
        dialog::setting_card(ui, |ui| {
            ui.label(
                RichText::new(format!(
                    "{}는 이미 '{}'에 등록되어 있습니다.",
                    conflict.shortcut.label(),
                    conflict.existing_command.label()
                ))
                .color(theme::TEXT_PRIMARY),
            );
            ui.horizontal(|ui| {
                if ui.button("기존 명령에서 제거하고 등록").clicked() {
                    if conflict.existing_index < draft.key_bindings.len() {
                        draft.key_bindings.remove(conflict.existing_index);
                    }
                    let adjusted_replace_index = conflict.replace_index.map(|index| {
                        if conflict.existing_index < index {
                            index - 1
                        } else {
                            index
                        }
                    });
                    set_shortcut_binding(
                        &mut draft.key_bindings,
                        conflict.command,
                        adjusted_replace_index,
                        conflict.shortcut,
                    );
                    self.shortcut_conflict = None;
                    *changed = true;
                }
                if ui.button("취소").clicked() {
                    self.shortcut_conflict = None;
                }
            });
        });
        ui.add_space(8.0);
    }
}

pub(in crate::app) fn show_mouse_settings(
    ui: &mut egui::Ui,
    draft: &mut AppSettings,
    changed: &mut bool,
) {
    setting_group(
        ui,
        "마우스 동작",
        "주요 버튼과 휠 조작을 실제 명령에 연결합니다.",
        |ui| {
            ui.horizontal_wrapped(|ui| {
                if ui.button("마우스 기본값 초기화").clicked() {
                    draft.mouse_bindings = default_mouse_bindings();
                    *changed = true;
                }
            });
            ui.add_space(6.0);
            egui::Grid::new("settings_mouse_binding_grid")
                .num_columns(3)
                .spacing([12.0, 8.0])
                .show(ui, |ui| {
                    for gesture in MouseGesture::ALL {
                        ui.label(gesture.label());
                        mouse_command_combo(ui, draft, changed, gesture);
                        if ui.small_button("기본값").clicked() {
                            reset_mouse_gesture(&mut draft.mouse_bindings, gesture);
                            *changed = true;
                        }
                        ui.end_row();
                    }
                });
        },
    );

    ui.add_space(8.0);
    setting_group(
        ui,
        "이동과 큰 이미지",
        "큰 이미지의 시작 위치와 기본 휠 처리 방식을 정합니다.",
        |ui| {
            egui::Grid::new("settings_mouse_grid")
                .num_columns(2)
                .spacing([14.0, 8.0])
                .show(ui, |ui| {
                    grid_label_with_help(
                        ui,
                        "큰 이미지 시작 위치",
                        "화면보다 큰 이미지를 처음 열 때 어느 위치부터 보여줄지 정합니다.",
                    );
                    egui::ComboBox::from_id_salt("large_image_anchor")
                        .selected_text(draft.large_image_anchor.label())
                        .show_ui(ui, |ui| {
                            for anchor in LargeImageAnchor::ALL {
                                *changed |= ui
                                    .selectable_value(
                                        &mut draft.large_image_anchor,
                                        anchor,
                                        anchor.label(),
                                    )
                                    .changed();
                            }
                        });
                    ui.end_row();

                    grid_label_with_help(
                        ui,
                        "휠 동작",
                        "고정 조작이 필요할 때 쓰는 보조 정책입니다. 기본 바인딩은 위 목록에서 바꿉니다.",
                    );
                    egui::ComboBox::from_id_salt("wheel_mode")
                        .selected_text(draft.wheel_mode.label())
                        .show_ui(ui, |ui| {
                            for mode in WheelMode::ALL {
                                *changed |= ui
                                    .selectable_value(&mut draft.wheel_mode, mode, mode.label())
                                    .changed();
                            }
                        });
                    ui.end_row();
                });
        },
    );
}

fn set_shortcut_binding(
    bindings: &mut Vec<KeyBinding>,
    command: CommandId,
    replace_index: Option<usize>,
    shortcut: KeyShortcut,
) {
    if let Some(index) = replace_index {
        if let Some(binding) = bindings.get_mut(index) {
            binding.command = command;
            binding.shortcut = shortcut;
            return;
        }
    }

    bindings.push(KeyBinding { command, shortcut });
}

fn shortcut_sort_key(binding: &KeyBinding) -> (u8, String) {
    let shortcut = binding.shortcut;
    let modifier_rank =
        u8::from(shortcut.ctrl) + u8::from(shortcut.alt) * 2 + u8::from(shortcut.shift) * 4;
    (modifier_rank, shortcut.label())
}

fn default_command_for_shortcut(shortcut: KeyShortcut) -> Option<CommandId> {
    default_key_bindings()
        .into_iter()
        .find(|binding| binding.shortcut == shortcut)
        .map(|binding| binding.command)
}

fn command_combo(
    ui: &mut egui::Ui,
    id: impl std::hash::Hash,
    current: CommandId,
    width: f32,
) -> Option<CommandId> {
    let mut selected = current;
    egui::ComboBox::from_id_salt(id)
        .selected_text(current.label())
        .width(width)
        .show_ui(ui, |ui| {
            for command in CommandId::ALL {
                let label = format!("{} / {}", command.group(), command.label());
                ui.selectable_value(&mut selected, command, label);
            }
        });
    (selected != current).then_some(selected)
}

fn mouse_command_combo(
    ui: &mut egui::Ui,
    draft: &mut AppSettings,
    changed: &mut bool,
    gesture: MouseGesture,
) {
    let current = draft
        .mouse_bindings
        .iter()
        .find(|binding| binding.gesture == gesture)
        .map(|binding| binding.command);
    let selected_text = current.map_or("동작 없음", CommandId::label);
    egui::ComboBox::from_id_salt(("mouse_gesture", gesture))
        .selected_text(selected_text)
        .width(220.0)
        .show_ui(ui, |ui| {
            if ui
                .selectable_label(current.is_none(), "동작 없음")
                .clicked()
            {
                draft
                    .mouse_bindings
                    .retain(|binding| binding.gesture != gesture);
                *changed = true;
            }
            for command in CommandId::ALL {
                if ui
                    .selectable_label(current == Some(command), command.label())
                    .clicked()
                {
                    set_mouse_binding(&mut draft.mouse_bindings, gesture, command);
                    *changed = true;
                }
            }
        });
}

fn set_mouse_binding(bindings: &mut Vec<MouseBinding>, gesture: MouseGesture, command: CommandId) {
    if let Some(binding) = bindings
        .iter_mut()
        .find(|binding| binding.gesture == gesture)
    {
        binding.command = command;
    } else {
        bindings.push(MouseBinding { gesture, command });
    }
}

fn reset_mouse_gesture(bindings: &mut Vec<MouseBinding>, gesture: MouseGesture) {
    bindings.retain(|binding| binding.gesture != gesture);
    if let Some(default_binding) = default_mouse_bindings()
        .into_iter()
        .find(|binding| binding.gesture == gesture)
    {
        bindings.push(default_binding);
    }
}

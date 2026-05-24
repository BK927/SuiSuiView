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
            "명령을 기준으로 현재 키와 내부 ID를 한 표에 표시합니다.",
            |ui| {
                ui.horizontal_wrapped(|ui| {
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
        let mut clear_command = None;
        let mut reset_command = None;
        dialog::setting_card(ui, |ui| {
            egui::Grid::new("settings_key_binding_grid")
                .num_columns(4)
                .spacing([14.0, 5.0])
                .striped(true)
                .show(ui, |ui| {
                    ui.label(RichText::new("명령").strong().color(theme::TEXT_MUTED));
                    ui.label(RichText::new("키").strong().color(theme::TEXT_MUTED));
                    ui.label(RichText::new("ID").strong().color(theme::TEXT_MUTED));
                    ui.label(RichText::new("편집").strong().color(theme::TEXT_MUTED));
                    ui.end_row();

                    for command in CommandId::ALL {
                        let indices = key_binding_indices(&draft.key_bindings, command);
                        let key_label = command_shortcut_label(&draft.key_bindings, &indices);
                        ui.add_sized(
                            [230.0, 20.0],
                            egui::Label::new(
                                RichText::new(command.label()).color(theme::TEXT_PRIMARY),
                            )
                            .truncate(),
                        )
                        .on_hover_text(command.group());
                        ui.add_sized(
                            [260.0, 20.0],
                            egui::Label::new(RichText::new(key_label).monospace().color(
                                if indices.is_empty() {
                                    theme::TEXT_MUTED
                                } else {
                                    theme::TEXT_PRIMARY
                                },
                            ))
                            .truncate(),
                        )
                        .on_hover_text(command_shortcut_hover(&draft.key_bindings, &indices));
                        ui.label(
                            RichText::new(command.id().to_string())
                                .monospace()
                                .color(theme::TEXT_MUTED),
                        );
                        ui.horizontal(|ui| {
                            if ui.small_button("추가").clicked() {
                                self.shortcut_capture = Some(ShortcutCapture {
                                    command,
                                    replace_index: None,
                                });
                                self.shortcut_conflict = None;
                            }
                            let change = ui
                                .add_enabled(!indices.is_empty(), egui::Button::new("변경"))
                                .on_disabled_hover_text("먼저 단축키를 추가해야 합니다.");
                            if change.clicked() {
                                self.shortcut_capture = Some(ShortcutCapture {
                                    command,
                                    replace_index: indices.first().copied(),
                                });
                                self.shortcut_conflict = None;
                            }
                            let delete = ui
                                .add_enabled(!indices.is_empty(), egui::Button::new("삭제"))
                                .on_disabled_hover_text("등록된 단축키가 없습니다.");
                            if delete.clicked() {
                                clear_command = Some(command);
                            }
                            if ui.small_button("기본값").clicked() {
                                reset_command = Some(command);
                            }
                        });
                        ui.end_row();
                    }
                });
        });

        if let Some(command) = clear_command {
            draft
                .key_bindings
                .retain(|binding| binding.command != command);
            self.shortcut_capture = None;
            self.shortcut_conflict = None;
            *changed = true;
        }
        if let Some(command) = reset_command {
            reset_command_shortcuts(&mut draft.key_bindings, command);
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

fn key_binding_indices(bindings: &[KeyBinding], command: CommandId) -> Vec<usize> {
    bindings
        .iter()
        .enumerate()
        .filter_map(|(index, binding)| (binding.command == command).then_some(index))
        .collect()
}

fn command_shortcut_label(bindings: &[KeyBinding], indices: &[usize]) -> String {
    if indices.is_empty() {
        return String::from("-");
    }
    indices
        .iter()
        .filter_map(|index| bindings.get(*index))
        .map(|binding| binding.shortcut.label())
        .collect::<Vec<_>>()
        .join(" / ")
}

fn command_shortcut_hover(bindings: &[KeyBinding], indices: &[usize]) -> String {
    if indices.is_empty() {
        String::from("등록된 단축키 없음")
    } else {
        command_shortcut_label(bindings, indices)
    }
}

fn reset_command_shortcuts(bindings: &mut Vec<KeyBinding>, command: CommandId) {
    bindings.retain(|binding| binding.command != command);
    bindings.extend(
        default_key_bindings()
            .into_iter()
            .filter(|binding| binding.command == command),
    );
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

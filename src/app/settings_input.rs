use super::commands::shortcut_from_input_event;
use super::settings::{grid_label_with_help, setting_group};
use super::ui::{dialog, icons, theme};
use super::SuiSuiViewApp;
use crate::core::state::{
    default_key_bindings, default_mouse_bindings, AppSettings, CommandId, KeyBinding, KeyCode,
    KeyShortcut, LargeImageAnchor, MouseBinding, MouseGesture, WheelMode,
};
use eframe::egui::{self, RichText};

const SHORTCUT_ACTION_WIDTH: f32 = 96.0;
const SHORTCUT_COLUMN_GAP: f32 = 16.0;
const SHORTCUT_MIN_KEY_WIDTH: f32 = 96.0;

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
            "명령을 기준으로 현재 키를 한 표에 표시합니다.",
            |ui| {
                ui.horizontal_wrapped(|ui| {
                    if ui.button("전체 기본값 초기화").clicked() {
                        draft.key_bindings = default_key_bindings();
                        self.shortcut_capture = None;
                        self.shortcut_conflict = None;
                        *changed = true;
                    }
                });
                ui.add_space(8.0);
                self.shortcut_binding_table(ui, draft, changed);
            },
        );
        self.show_shortcut_capture_dialog(ctx, draft);
    }

    fn shortcut_binding_table(
        &mut self,
        ui: &mut egui::Ui,
        draft: &mut AppSettings,
        changed: &mut bool,
    ) {
        dialog::setting_card(ui, |ui| {
            keyboard_table_header(ui);
            for group in shortcut_groups() {
                self.shortcut_group_table(ui, draft, changed, group);
            }
        });
    }

    fn shortcut_group_table(
        &mut self,
        ui: &mut egui::Ui,
        draft: &mut AppSettings,
        changed: &mut bool,
        group: ShortcutGroup,
    ) {
        shortcut_group_header(ui, group);
        let expanded = self.shortcut_expanded_groups.contains(group.title);
        for command in group.visible_commands(expanded) {
            self.shortcut_command_row(ui, draft, changed, *command);
        }
        if group.hidden_count() > 0
            && shortcut_more_row(ui, group.hidden_count(), expanded).clicked()
        {
            if expanded {
                self.shortcut_expanded_groups.remove(group.title);
            } else {
                self.shortcut_expanded_groups.insert(group.title);
            }
        }
    }

    fn shortcut_command_row(
        &mut self,
        ui: &mut egui::Ui,
        draft: &mut AppSettings,
        changed: &mut bool,
        command: CommandId,
    ) {
        let indices = key_binding_indices(&draft.key_bindings, command);
        egui::Frame::new()
            .fill(theme::ROW_FILL)
            .stroke(egui::Stroke::new(1.0, egui::Color32::from_rgb(43, 47, 53)))
            .inner_margin(egui::Margin::symmetric(10, 4))
            .show(ui, |ui| {
                let (command_width, key_width, _) = shortcut_column_widths(ui.available_width());
                ui.horizontal(|ui| {
                    ui.add_sized(
                        [command_width, 24.0],
                        egui::Label::new(RichText::new(command.label()).color(theme::TEXT_PRIMARY))
                            .truncate(),
                    )
                    .on_hover_text(command.group());

                    ui.add_sized(
                        [key_width, 24.0],
                        egui::Label::new(
                            RichText::new(command_shortcut_label(&draft.key_bindings, &indices))
                                .monospace()
                                .color(if indices.is_empty() {
                                    theme::TEXT_MUTED
                                } else {
                                    theme::TEXT_PRIMARY
                                }),
                        )
                        .truncate(),
                    )
                    .on_hover_text(command_shortcut_hover(&draft.key_bindings, &indices));

                    ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                        ui.menu_button("...", |ui| {
                            if ui.button("단축키 추가").clicked() {
                                self.shortcut_capture = Some(ShortcutCapture {
                                    command,
                                    replace_index: None,
                                });
                                self.shortcut_conflict = None;
                                ui.close();
                            }
                            if ui.button("기본값").clicked() {
                                reset_command_shortcuts(&mut draft.key_bindings, command);
                                self.shortcut_capture = None;
                                self.shortcut_conflict = None;
                                *changed = true;
                                ui.close();
                            }
                            let delete = ui
                                .add_enabled(!indices.is_empty(), egui::Button::new("삭제"))
                                .on_disabled_hover_text("등록된 단축키가 없습니다.");
                            if delete.clicked() {
                                draft
                                    .key_bindings
                                    .retain(|binding| binding.command != command);
                                self.shortcut_capture = None;
                                self.shortcut_conflict = None;
                                *changed = true;
                                ui.close();
                            }
                        });
                        let replace_index = indices.first().copied();
                        let edit_label = if replace_index.is_some() {
                            "변경"
                        } else {
                            "추가"
                        };
                        if ui.small_button(edit_label).clicked() {
                            self.shortcut_capture = Some(ShortcutCapture {
                                command,
                                replace_index,
                            });
                            self.shortcut_conflict = None;
                        }
                    });
                });
            });
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

    fn show_shortcut_capture_dialog(&mut self, ctx: &egui::Context, draft: &AppSettings) {
        let Some(capture) = self.shortcut_capture else {
            return;
        };
        let indices = key_binding_indices(&draft.key_bindings, capture.command);
        let current = command_shortcut_label(&draft.key_bindings, &indices);
        let title = if capture.replace_index.is_some() {
            "단축키 변경"
        } else {
            "단축키 추가"
        };
        let help = if capture.replace_index.is_some() {
            "Esc 취소 · Delete/Backspace 선택 단축키 삭제"
        } else {
            "Esc 취소 · 원하는 키를 누르면 추가"
        };

        egui::Window::new(title)
            .id(egui::Id::new("shortcut_capture_dialog"))
            .anchor(egui::Align2::CENTER_CENTER, egui::vec2(0.0, 0.0))
            .fixed_size(egui::vec2(360.0, 170.0))
            .collapsible(false)
            .order(egui::Order::Foreground)
            .resizable(false)
            .show(ctx, |ui| {
                dialog::setting_card(ui, |ui| {
                    ui.vertical_centered(|ui| {
                        ui.label(
                            RichText::new(capture.command.label())
                                .size(18.0)
                                .strong()
                                .color(theme::TEXT_PRIMARY),
                        );
                        ui.add_space(4.0);
                        ui.label(
                            RichText::new("등록할 키를 누르세요")
                                .size(14.0)
                                .color(theme::ACCENT),
                        );
                    });
                    ui.add_space(10.0);
                    ui.add_sized(
                        [ui.available_width(), 20.0],
                        egui::Label::new(
                            RichText::new(format!("현재 단축키: {current}"))
                                .monospace()
                                .color(theme::TEXT_MUTED),
                        )
                        .truncate(),
                    )
                    .on_hover_text(current);
                    ui.label(RichText::new(help).size(12.5).color(theme::TEXT_MUTED));
                    ui.add_space(8.0);
                    ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                        if ui.button("취소").clicked() {
                            self.shortcut_capture = None;
                        }
                    });
                });
            });
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

#[derive(Debug, Clone, Copy)]
struct ShortcutGroup {
    title: &'static str,
    icon: char,
    commands: &'static [CommandId],
    preview_count: usize,
}

impl ShortcutGroup {
    fn visible_commands(self, expanded: bool) -> &'static [CommandId] {
        if expanded {
            self.commands
        } else {
            let count = self.preview_count.min(self.commands.len());
            &self.commands[..count]
        }
    }

    fn hidden_count(self) -> usize {
        self.commands.len().saturating_sub(self.preview_count)
    }
}

const FILE_SHORTCUTS: &[CommandId] = &[
    CommandId::OpenFile,
    CommandId::OpenFolder,
    CommandId::CloseBook,
    CommandId::Quit,
    CommandId::QuitFromEsc,
    CommandId::OpenExplorer,
    CommandId::CopyPath,
];

const VIEW_SHORTCUTS: &[CommandId] = &[
    CommandId::ToggleFullscreen,
    CommandId::ToggleMaximized,
    CommandId::Minimize,
    CommandId::ToggleAlwaysOnTop,
    CommandId::FitOriginal,
    CommandId::FitPage,
    CommandId::FitWidth,
    CommandId::FitHeight,
    CommandId::SetDoubleLeftToRight,
    CommandId::SetDoubleRightToLeft,
    CommandId::ToggleDouble,
    CommandId::ZoomIn,
    CommandId::ZoomOut,
    CommandId::ZoomFineIn,
    CommandId::ZoomFineOut,
];

const NAVIGATION_SHORTCUTS: &[CommandId] = &[
    CommandId::NextPage,
    CommandId::PreviousPage,
    CommandId::Home,
    CommandId::End,
    CommandId::MoveForward10,
    CommandId::MoveBackward10,
    CommandId::MoveForward100,
    CommandId::MoveBackward100,
    CommandId::ForceNextPage,
    CommandId::ForcePreviousPage,
    CommandId::RandomForward,
    CommandId::RandomBackward,
    CommandId::NextBook,
    CommandId::PreviousBook,
];

const IMAGE_SHORTCUTS: &[CommandId] = &[
    CommandId::RotateClockwise,
    CommandId::RotateCounterClockwise,
    CommandId::Rotate0,
    CommandId::Rotate90,
    CommandId::Rotate180,
    CommandId::Rotate270,
    CommandId::FlipHorizontal,
    CommandId::FlipVertical,
    CommandId::ToggleInvert,
    CommandId::FilterNone,
    CommandId::FilterSmooth,
    CommandId::FilterSmoothSharpen,
    CommandId::ToggleGamma,
    CommandId::UpscaleCurrentPage,
];

const ACTION_SHORTCUTS: &[CommandId] = &[
    CommandId::DeleteRecycle,
    CommandId::DeletePermanent,
    CommandId::CopyPageImage,
    CommandId::CopyDisplayImage,
    CommandId::ToggleCurrentPageBookmark,
    CommandId::ToggleBookmarkPopover,
    CommandId::OpenSettings,
    CommandId::OpenAbout,
];

fn shortcut_groups() -> [ShortcutGroup; 5] {
    [
        ShortcutGroup {
            title: "파일",
            icon: icons::FOLDER_OPEN,
            commands: FILE_SHORTCUTS,
            preview_count: 4,
        },
        ShortcutGroup {
            title: "보기",
            icon: icons::EYE,
            commands: VIEW_SHORTCUTS,
            preview_count: 3,
        },
        ShortcutGroup {
            title: "탐색",
            icon: icons::CHEVRON_RIGHT,
            commands: NAVIGATION_SHORTCUTS,
            preview_count: 3,
        },
        ShortcutGroup {
            title: "영상 처리",
            icon: icons::WAND,
            commands: IMAGE_SHORTCUTS,
            preview_count: 4,
        },
        ShortcutGroup {
            title: "작업",
            icon: icons::DOCUMENT,
            commands: ACTION_SHORTCUTS,
            preview_count: 4,
        },
    ]
}

fn keyboard_table_header(ui: &mut egui::Ui) {
    egui::Frame::new()
        .fill(egui::Color32::from_rgb(21, 25, 30))
        .stroke(egui::Stroke::new(1.0, theme::SUBTLE_STROKE))
        .corner_radius(egui::CornerRadius::same(6))
        .inner_margin(egui::Margin::symmetric(14, 8))
        .show(ui, |ui| {
            let (command_width, key_width, _) = shortcut_column_widths(ui.available_width());
            ui.horizontal(|ui| {
                ui.add_sized(
                    [command_width, 20.0],
                    egui::Label::new(RichText::new("명령").strong().color(theme::TEXT_MUTED)),
                );
                ui.add_sized(
                    [key_width, 20.0],
                    egui::Label::new(
                        RichText::new("현재 단축키")
                            .strong()
                            .color(theme::TEXT_MUTED),
                    ),
                );
                ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                    ui.label(RichText::new("작업").strong().color(theme::TEXT_MUTED));
                });
            });
        });
}

fn shortcut_column_widths(total_width: f32) -> (f32, f32, f32) {
    let reserved = SHORTCUT_ACTION_WIDTH + SHORTCUT_COLUMN_GAP;
    let preferred_command = (total_width * 0.36).clamp(180.0, 260.0);
    let command_width = if total_width < preferred_command + reserved + SHORTCUT_MIN_KEY_WIDTH {
        (total_width - reserved - SHORTCUT_MIN_KEY_WIDTH).clamp(140.0, preferred_command)
    } else {
        preferred_command
    };
    let key_width = (total_width - command_width - reserved).max(SHORTCUT_MIN_KEY_WIDTH);
    (command_width, key_width, SHORTCUT_ACTION_WIDTH)
}

fn shortcut_group_header(ui: &mut egui::Ui, group: ShortcutGroup) {
    ui.add_space(8.0);
    egui::Frame::new()
        .fill(egui::Color32::from_rgb(17, 22, 27))
        .stroke(egui::Stroke::new(1.0, egui::Color32::from_rgb(39, 44, 50)))
        .inner_margin(egui::Margin::symmetric(14, 8))
        .show(ui, |ui| {
            ui.horizontal(|ui| {
                ui.label(icons::icon(
                    group.icon,
                    icons::IconStyle::Regular,
                    18.0,
                    theme::SELECT_STROKE,
                ));
                ui.label(
                    RichText::new(group.title)
                        .size(16.0)
                        .strong()
                        .color(theme::TEXT_PRIMARY),
                );
                ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                    ui.label(
                        RichText::new(group.commands.len().to_string())
                            .monospace()
                            .color(theme::TEXT_MUTED),
                    );
                });
            });
        });
}

fn shortcut_more_row(ui: &mut egui::Ui, count: usize, expanded: bool) -> egui::Response {
    let text = if expanded {
        "접기".to_owned()
    } else {
        format!("더보기 {count}개")
    };
    ui.add_sized(
        [ui.available_width(), 30.0],
        egui::Button::new(RichText::new(text).color(theme::TEXT_MUTED)).fill(theme::ROW_FILL),
    )
    .on_hover_cursor(egui::CursorIcon::PointingHand)
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

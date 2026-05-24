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
            "명령을 기준으로 현재 키를 한 표에 표시합니다.",
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
        let row_height = 34.0;
        let row_width = ui.available_width();
        let (rect, response) =
            ui.allocate_exact_size(egui::vec2(row_width, row_height), egui::Sense::click());
        let fill = if response.hovered() {
            theme::ROW_FILL_SELECTED
        } else {
            theme::ROW_FILL
        };
        ui.painter().rect_filled(rect, 0.0, fill);
        ui.painter().hline(
            rect.x_range(),
            rect.bottom(),
            egui::Stroke::new(1.0, egui::Color32::from_rgb(44, 48, 54)),
        );

        let command_rect = egui::Rect::from_min_max(
            rect.min + egui::vec2(56.0, 0.0),
            egui::pos2(rect.left() + row_width * 0.55, rect.bottom()),
        );
        let key_rect = egui::Rect::from_min_max(
            egui::pos2(command_rect.right() + 12.0, rect.top()),
            egui::pos2(rect.right() - 116.0, rect.bottom()),
        );
        let action_rect = egui::Rect::from_min_max(
            egui::pos2(rect.right() - 104.0, rect.top()),
            rect.right_bottom(),
        );

        ui.put(
            command_rect,
            egui::Label::new(RichText::new(command.label()).color(theme::TEXT_PRIMARY)).truncate(),
        )
        .on_hover_text(command.group());
        let parent_clip = ui.clip_rect();
        ui.scope_builder(egui::UiBuilder::new().max_rect(key_rect), |ui| {
            ui.set_clip_rect(key_rect.intersect(parent_clip));
            shortcut_chips(ui, &draft.key_bindings, &indices);
        })
        .response
        .on_hover_text(command_shortcut_hover(&draft.key_bindings, &indices));
        ui.scope_builder(egui::UiBuilder::new().max_rect(action_rect), |ui| {
            ui.set_clip_rect(action_rect.intersect(parent_clip));
            ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                let replace_index = indices.first().copied();
                let edit_hint = if replace_index.is_some() {
                    "첫 번째 단축키 변경"
                } else {
                    "단축키 추가"
                };
                if edit_action_button(ui, edit_hint).clicked() {
                    self.shortcut_capture = Some(ShortcutCapture {
                        command,
                        replace_index,
                    });
                    self.shortcut_conflict = None;
                }
                let reset = ui.small_button("기본값");
                if reset.clicked() {
                    reset_command_shortcuts(&mut draft.key_bindings, command);
                    self.shortcut_capture = None;
                    self.shortcut_conflict = None;
                    *changed = true;
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
                }
            });
        });
        if response.double_clicked() {
            self.shortcut_capture = Some(ShortcutCapture {
                command,
                replace_index: indices.first().copied(),
            });
            self.shortcut_conflict = None;
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

#[derive(Debug, Clone, Copy)]
struct ShortcutGroup {
    title: &'static str,
    icon: &'static str,
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
            icon: "□",
            commands: FILE_SHORTCUTS,
            preview_count: 4,
        },
        ShortcutGroup {
            title: "보기",
            icon: "◉",
            commands: VIEW_SHORTCUTS,
            preview_count: 3,
        },
        ShortcutGroup {
            title: "탐색",
            icon: "▷",
            commands: NAVIGATION_SHORTCUTS,
            preview_count: 3,
        },
        ShortcutGroup {
            title: "영상 처리",
            icon: "✦",
            commands: IMAGE_SHORTCUTS,
            preview_count: 4,
        },
        ShortcutGroup {
            title: "작업",
            icon: "◇",
            commands: ACTION_SHORTCUTS,
            preview_count: 4,
        },
    ]
}

fn keyboard_table_header(ui: &mut egui::Ui) {
    let width = ui.available_width();
    let (rect, _) = ui.allocate_exact_size(egui::vec2(width, 38.0), egui::Sense::hover());
    ui.painter()
        .rect_filled(rect, 6.0, egui::Color32::from_rgb(21, 25, 30));
    ui.painter().rect_stroke(
        rect,
        6.0,
        egui::Stroke::new(1.0, theme::SUBTLE_STROKE),
        egui::StrokeKind::Inside,
    );
    let command_x = rect.left() + 20.0;
    let key_x = rect.left() + width * 0.56;
    let action_x = rect.right() - 120.0;
    let y = rect.center().y;
    let font = egui::FontId::proportional(14.0);
    ui.painter().text(
        egui::pos2(command_x, y),
        egui::Align2::LEFT_CENTER,
        "명령",
        font.clone(),
        theme::TEXT_MUTED,
    );
    ui.painter().text(
        egui::pos2(key_x, y),
        egui::Align2::LEFT_CENTER,
        "현재 단축키",
        font.clone(),
        theme::TEXT_MUTED,
    );
    ui.painter().text(
        egui::pos2(action_x, y),
        egui::Align2::LEFT_CENTER,
        "작업",
        font,
        theme::TEXT_MUTED,
    );
}

fn shortcut_group_header(ui: &mut egui::Ui, group: ShortcutGroup) {
    let width = ui.available_width();
    let (rect, _) = ui.allocate_exact_size(egui::vec2(width, 38.0), egui::Sense::hover());
    ui.painter()
        .rect_filled(rect, 0.0, egui::Color32::from_rgb(17, 22, 27));
    ui.painter().vline(
        rect.left(),
        rect.y_range(),
        egui::Stroke::new(2.0, theme::SELECT_STROKE),
    );
    ui.painter().hline(
        rect.x_range(),
        rect.bottom(),
        egui::Stroke::new(1.0, egui::Color32::from_rgb(39, 44, 50)),
    );
    let y = rect.center().y;
    ui.painter().text(
        egui::pos2(rect.left() + 22.0, y),
        egui::Align2::LEFT_CENTER,
        group.icon,
        egui::FontId::proportional(18.0),
        theme::SELECT_STROKE,
    );
    ui.painter().text(
        egui::pos2(rect.left() + 54.0, y),
        egui::Align2::LEFT_CENTER,
        group.title,
        egui::FontId::proportional(16.0),
        theme::TEXT_PRIMARY,
    );
    ui.painter().text(
        egui::pos2(rect.right() - 96.0, y),
        egui::Align2::LEFT_CENTER,
        group.commands.len().to_string(),
        egui::FontId::monospace(14.0),
        theme::TEXT_MUTED,
    );
}

fn shortcut_more_row(ui: &mut egui::Ui, count: usize, expanded: bool) -> egui::Response {
    let width = ui.available_width();
    let (rect, response) = ui.allocate_exact_size(egui::vec2(width, 34.0), egui::Sense::click());
    let fill = if response.hovered() {
        theme::ROW_FILL_SELECTED
    } else {
        theme::ROW_FILL
    };
    ui.painter().rect_filled(rect, 0.0, fill);
    let text = if expanded {
        "접기".to_owned()
    } else {
        format!("더보기 {count}개")
    };
    ui.painter().text(
        egui::pos2(rect.left() + 56.0, rect.center().y),
        egui::Align2::LEFT_CENTER,
        text,
        egui::FontId::proportional(14.0),
        theme::TEXT_MUTED,
    );
    ui.painter().text(
        egui::pos2(rect.left() + 150.0, rect.center().y),
        egui::Align2::LEFT_CENTER,
        if expanded { "⌃" } else { "⌄" },
        egui::FontId::proportional(16.0),
        theme::TEXT_MUTED,
    );
    response.on_hover_cursor(egui::CursorIcon::PointingHand)
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

fn shortcut_chips(ui: &mut egui::Ui, bindings: &[KeyBinding], indices: &[usize]) {
    if indices.is_empty() {
        ui.label(RichText::new("-").monospace().color(theme::TEXT_MUTED));
        return;
    }

    ui.horizontal(|ui| {
        for (shown, index) in indices.iter().enumerate() {
            if shown >= 3 {
                let remaining = indices.len() - shown;
                ui.label(
                    RichText::new(format!("+{remaining}"))
                        .monospace()
                        .color(theme::TEXT_MUTED),
                );
                break;
            }
            let Some(binding) = bindings.get(*index) else {
                continue;
            };
            egui::Frame::new()
                .fill(egui::Color32::from_rgb(39, 44, 50))
                .stroke(egui::Stroke::new(1.0, egui::Color32::from_rgb(60, 66, 74)))
                .corner_radius(egui::CornerRadius::same(5))
                .inner_margin(egui::Margin::symmetric(7, 2))
                .show(ui, |ui| {
                    ui.label(
                        RichText::new(binding.shortcut.label())
                            .monospace()
                            .color(theme::TEXT_PRIMARY),
                    );
                });
        }
    });
}

fn edit_action_button(ui: &mut egui::Ui, help: &'static str) -> egui::Response {
    ui.add(
        egui::Button::new(RichText::new("✎").size(15.0).color(theme::TEXT_PRIMARY))
            .min_size(egui::vec2(26.0, 24.0)),
    )
    .on_hover_text(help)
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

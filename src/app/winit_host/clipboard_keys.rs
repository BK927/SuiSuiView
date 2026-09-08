//! Preserve shortcut keys that egui-winit translates into clipboard events.
use egui_winit::winit::{
    event::{ElementState, WindowEvent},
    keyboard::{Key, KeyCode, NamedKey, PhysicalKey},
};

pub(super) fn preserve_key_event(
    input: &mut egui::RawInput,
    event_start: usize,
    event: &WindowEvent,
) {
    let WindowEvent::KeyboardInput {
        event,
        is_synthetic,
        ..
    } = event
    else {
        return;
    };
    if *is_synthetic || event.state != ElementState::Pressed {
        return;
    }
    append_clipboard_key(
        input,
        event_start,
        &event.logical_key,
        event.physical_key,
        event.repeat,
    );
}

fn append_clipboard_key(
    input: &mut egui::RawInput,
    event_start: usize,
    logical: &Key,
    physical: PhysicalKey,
    repeat: bool,
) {
    let physical_key = match physical {
        PhysicalKey::Code(KeyCode::KeyC) => Some(egui::Key::C),
        PhysicalKey::Code(KeyCode::KeyX) => Some(egui::Key::X),
        PhysicalKey::Code(KeyCode::KeyV) => Some(egui::Key::V),
        PhysicalKey::Code(KeyCode::Delete) => Some(egui::Key::Delete),
        PhysicalKey::Code(KeyCode::Insert) => Some(egui::Key::Insert),
        _ => None,
    };
    let logical_key = match logical {
        Key::Character(text) => egui::Key::from_name(text.as_str()),
        Key::Named(NamedKey::Delete) => Some(egui::Key::Delete),
        Key::Named(NamedKey::Insert) => Some(egui::Key::Insert),
        _ => None,
    };
    let Some(key) = logical_key.or(physical_key) else {
        return;
    };
    let new_events = &input.events[event_start..];
    if new_events.iter().any(|event| {
        matches!(event, egui::Event::Key { key: emitted, pressed: true, .. } if *emitted == key)
    }) {
        return;
    }
    // An empty or image-only clipboard produces no Paste event, but the
    // adapter still consumes the paste chord. Preserve that key as well.
    let paste_chord = (input.modifiers.command && key == egui::Key::V)
        || (cfg!(target_os = "windows") && input.modifiers.shift && key == egui::Key::Insert);
    if !paste_chord
        && !new_events.iter().any(|event| {
            matches!(
                event,
                egui::Event::Copy | egui::Event::Cut | egui::Event::Paste(_)
            )
        })
    {
        return;
    }
    // Keep clipboard events for text editors and the original key for viewer
    // shortcuts and recording, with modifiers from this exact press.
    input.events.push(egui::Event::Key {
        key,
        physical_key,
        pressed: true,
        repeat,
        modifiers: input.modifiers,
    });
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn paste_key_survives_without_text_and_is_not_duplicated() {
        for events in [vec![], vec![egui::Event::Paste("synthetic text".into())]] {
            let mut input = egui::RawInput {
                modifiers: egui::Modifiers::CTRL | egui::Modifiers::COMMAND,
                events,
                ..Default::default()
            };
            for _ in 0..2 {
                append_clipboard_key(
                    &mut input,
                    0,
                    &Key::Character("v".into()),
                    PhysicalKey::Code(KeyCode::KeyV),
                    false,
                );
            }
            assert_eq!(
                input
                    .events
                    .iter()
                    .filter(|event| matches!(
                        event,
                        egui::Event::Key {
                            key: egui::Key::V,
                            pressed: true,
                            ..
                        }
                    ))
                    .count(),
                1
            );
        }
    }

    #[test]
    fn copy_preserves_modifiers_repeat_and_text_editor_event() {
        let modifiers = egui::Modifiers {
            ctrl: true,
            command: true,
            alt: true,
            shift: true,
            ..Default::default()
        };
        let mut input = egui::RawInput {
            modifiers,
            events: vec![egui::Event::Copy],
            ..Default::default()
        };
        append_clipboard_key(
            &mut input,
            0,
            &Key::Character("c".into()),
            PhysicalKey::Code(KeyCode::KeyC),
            true,
        );
        // Releasing Ctrl before the next frame must not erase the chord.
        input.modifiers = egui::Modifiers::default();
        assert_eq!(
            input.events,
            vec![
                egui::Event::Copy,
                egui::Event::Key {
                    key: egui::Key::C,
                    physical_key: Some(egui::Key::C),
                    pressed: true,
                    repeat: true,
                    modifiers,
                }
            ]
        );
    }

    #[test]
    fn normal_key_does_not_reuse_an_earlier_clipboard_event() {
        let mut input = egui::RawInput {
            events: vec![egui::Event::Copy],
            ..Default::default()
        };
        append_clipboard_key(
            &mut input,
            1,
            &Key::Character("c".into()),
            PhysicalKey::Code(KeyCode::KeyC),
            false,
        );
        assert_eq!(input.events.len(), 1);
    }

    #[test]
    fn non_latin_layout_copy_uses_the_physical_key() {
        let mut input = egui::RawInput {
            events: vec![egui::Event::Copy],
            ..Default::default()
        };
        append_clipboard_key(
            &mut input,
            0,
            &Key::Character("ㅊ".into()),
            PhysicalKey::Code(KeyCode::KeyC),
            false,
        );
        assert!(matches!(
            input.events.last(),
            Some(egui::Event::Key {
                key: egui::Key::C,
                ..
            })
        ));
    }

    #[test]
    fn shift_delete_keeps_its_identity_instead_of_becoming_ctrl_x() {
        let mut input = egui::RawInput {
            modifiers: egui::Modifiers::SHIFT,
            events: vec![egui::Event::Cut],
            ..Default::default()
        };
        append_clipboard_key(
            &mut input,
            0,
            &Key::Named(NamedKey::Delete),
            PhysicalKey::Code(KeyCode::Delete),
            false,
        );
        assert!(matches!(
            input.events.last(),
            Some(egui::Event::Key {
                key: egui::Key::Delete,
                modifiers: egui::Modifiers {
                    shift: true,
                    ctrl: false,
                    ..
                },
                ..
            })
        ));
    }
}

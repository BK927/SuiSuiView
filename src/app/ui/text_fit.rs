use eframe::egui::{self, Color32, FontId};

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum KeepSide {
    Start,
    End,
}

pub(in crate::app) fn compact_end_to_width(
    ui: &egui::Ui,
    text: &str,
    font_id: FontId,
    color: Color32,
    max_width: f32,
) -> String {
    compact_to_width(ui, text, font_id, color, max_width, KeepSide::Start)
}

pub(in crate::app) fn compact_start_to_width(
    ui: &egui::Ui,
    text: &str,
    font_id: FontId,
    color: Color32,
    max_width: f32,
) -> String {
    compact_to_width(ui, text, font_id, color, max_width, KeepSide::End)
}

fn compact_to_width(
    ui: &egui::Ui,
    text: &str,
    font_id: FontId,
    color: Color32,
    max_width: f32,
    keep_side: KeepSide,
) -> String {
    let width_of = |candidate: &str| {
        ui.painter()
            .layout_no_wrap(candidate.to_owned(), font_id.clone(), color)
            .size()
            .x
    };
    if width_of(text) <= max_width {
        return text.to_owned();
    }
    if max_width <= 0.0 {
        return String::new();
    }

    let chars: Vec<_> = text.chars().collect();
    let mut low = 0;
    let mut high = chars.len();
    while low < high {
        let mid = (low + high).div_ceil(2);
        let candidate = compact_chars(&chars, mid, keep_side);
        if width_of(&candidate) <= max_width {
            low = mid;
        } else {
            high = mid - 1;
        }
    }

    if low == 0 {
        return "...".to_owned();
    }
    compact_chars(&chars, low, keep_side)
}

fn compact_chars(chars: &[char], kept: usize, keep_side: KeepSide) -> String {
    match keep_side {
        KeepSide::Start => {
            let head: String = chars[..kept].iter().collect();
            format!("{head}...")
        }
        KeepSide::End => {
            let tail: String = chars[chars.len() - kept..].iter().collect();
            format!("...{tail}")
        }
    }
}

use super::theme;
use egui::{self, text::LayoutJob, Color32, FontFamily, FontId, RichText, TextFormat};

pub(in crate::app) const REGULAR_FONT: &str = "suisuiview-fluent-icons-regular";
pub(in crate::app) const FILLED_FONT: &str = "suisuiview-fluent-icons-filled";

// Some Fluent regular/filled glyphs share codepoints, but not all of them do.
// Use the matching filled constant when a glyph has separate regular/filled codes.
pub(in crate::app) const BOOKMARK: char = '\u{F1F6}';
pub(in crate::app) const BOOKMARK_FILLED: char = BOOKMARK;
pub(in crate::app) const CHEVRON_LEFT: char = '\u{F2AB}';
pub(in crate::app) const CHEVRON_RIGHT: char = '\u{F2B1}';
pub(in crate::app) const CURSOR_CLICK: char = '\u{E446}';
pub(in crate::app) const DELETE: char = '\u{F34D}';
pub(in crate::app) const DISMISS: char = '\u{F36A}';
pub(in crate::app) const DOCUMENT: char = '\u{F379}';
pub(in crate::app) const EYE: char = '\u{E5F3}';
pub(in crate::app) const FOLDER_OPEN: char = '\u{F42F}';
pub(in crate::app) const IMAGE_SPARKLE: char = '\u{F01F2}';
pub(in crate::app) const INFO: char = '\u{F4A4}';
pub(in crate::app) const KEYBOARD: char = '\u{F4B9}';
pub(in crate::app) const LOCK_OPEN: char = '\u{E796}';
pub(in crate::app) const MORE_HORIZONTAL: char = '\u{E824}';
pub(in crate::app) const PIN: char = '\u{F602}';
pub(in crate::app) const PIN_FILLED: char = '\u{F60C}';
pub(in crate::app) const RESIZE_SMALL: char = '\u{EA1A}';
pub(in crate::app) const SEARCH: char = '\u{F690}';
pub(in crate::app) const SETTINGS: char = '\u{F6AA}';
pub(in crate::app) const WAND: char = '\u{EE38}';

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(in crate::app) enum IconStyle {
    Regular,
    Filled,
}

pub(in crate::app) fn icon(code: char, style: IconStyle, size: f32, color: Color32) -> RichText {
    RichText::new(code.to_string())
        .font(icon_font(style, size))
        .color(color)
}

pub(in crate::app) fn icon_text(code: char, text: &str) -> egui::WidgetText {
    let mut job = LayoutJob::default();
    job.append(
        &code.to_string(),
        0.0,
        TextFormat {
            font_id: icon_font(IconStyle::Regular, 18.0),
            color: theme::TEXT_PRIMARY,
            ..Default::default()
        },
    );
    job.append(
        "  ",
        0.0,
        TextFormat {
            font_id: FontId::proportional(14.0),
            color: theme::TEXT_PRIMARY,
            ..Default::default()
        },
    );
    job.append(
        text,
        0.0,
        TextFormat {
            font_id: FontId::proportional(14.0),
            color: theme::TEXT_PRIMARY,
            ..Default::default()
        },
    );
    job.into()
}

pub(in crate::app) fn icon_font(style: IconStyle, size: f32) -> FontId {
    let family = match style {
        IconStyle::Regular => REGULAR_FONT,
        IconStyle::Filled => FILLED_FONT,
    };
    FontId::new(size, FontFamily::Name(family.into()))
}

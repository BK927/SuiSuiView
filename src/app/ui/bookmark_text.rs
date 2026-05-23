use super::bookmark_rows::BookmarkRow;
use super::{path_labels, theme};
use eframe::egui::{self, Align2, Color32, FontId, Rect, Sense};
use std::borrow::Cow;
use std::time::Duration;

const TITLE_HEIGHT: f32 = 54.0;
const TEXT_HORIZONTAL_PADDING: f32 = 6.0;
const TEXT_LINE_GAP: f32 = 3.0;
const TEXT_MAX_CHARS: usize = 180;
const MARQUEE_SPEED: f32 = 82.0;
const MARQUEE_MIN_SCROLL_SECONDS: f32 = 1.2;
const MARQUEE_END_PAUSE_SECONDS: f32 = 0.7;

pub(super) fn allocate_bookmark_title(
    ui: &mut egui::Ui,
    row: &BookmarkRow,
    width: f32,
) -> egui::Response {
    let (_, response) = ui.allocate_exact_size(egui::vec2(width, TITLE_HEIGHT), Sense::click());
    response.on_hover_text(&row.display_name)
}

pub(super) fn paint_bookmark_title(
    ui: &egui::Ui,
    rect: Rect,
    row: &BookmarkRow,
    marquee_started_at: Option<f64>,
) {
    if !rect.is_positive() {
        return;
    }

    let painter = ui.painter_at(rect);
    let font_id = FontId::proportional(14.0);
    let text_width = (rect.width() - TEXT_HORIZONTAL_PADDING * 2.0).max(1.0);
    let (lines, fits_two_lines) =
        bookmark_display_lines(ui, &row.display_name, text_width, &font_id);

    if let Some(started_at) = marquee_started_at.filter(|_| !fits_two_lines) {
        if let Some(repaint_after) = draw_marquee_text(
            ui,
            MarqueeText {
                clip_rect: rect,
                pos: egui::pos2(rect.left() + TEXT_HORIZONTAL_PADDING, rect.center().y),
                text: &row.display_name,
                font_id,
                color: theme::TEXT_PRIMARY,
                width: text_width,
                started_at,
            },
        ) {
            ui.ctx().request_repaint_after(repaint_after);
        }
        return;
    }

    let line_height = text_line_height(font_id.size);
    let total_height = line_height * lines.len().max(1) as f32;
    let first_y = rect.center().y - total_height * 0.5 + line_height * 0.5;
    for (index, line) in lines.iter().enumerate() {
        painter.text(
            egui::pos2(
                rect.left() + TEXT_HORIZONTAL_PADDING,
                first_y + line_height * index as f32,
            ),
            Align2::LEFT_CENTER,
            line,
            font_id.clone(),
            theme::TEXT_PRIMARY,
        );
    }
}

fn text_line_height(font_size: f32) -> f32 {
    font_size + TEXT_LINE_GAP
}

fn bookmark_display_lines(
    ui: &egui::Ui,
    display_name: &str,
    width: f32,
    font_id: &FontId,
) -> (Vec<String>, bool) {
    let static_display = static_bookmark_display(display_name);
    let (lines, fits) = wrap_bookmark_display(ui, &static_display, width, font_id, 2);
    if fits {
        return (lines, true);
    }

    let char_count = static_display.chars().count().min(TEXT_MAX_CHARS);
    let mut best = 4_usize;
    let mut low = 4_usize;
    let mut high = char_count;
    while low <= high {
        let mid = low + (high - low) / 2;
        let candidate = path_labels::compact_start_for_two_lines(&static_display, mid);
        let (_, candidate_fits) = wrap_bookmark_display(ui, &candidate, width, font_id, 2);
        if candidate_fits {
            best = mid;
            low = mid + 1;
        } else {
            high = mid.saturating_sub(1);
        }
    }

    let compact = path_labels::compact_start_for_two_lines(&static_display, best);
    (
        wrap_bookmark_display(ui, &compact, width, font_id, 2).0,
        false,
    )
}

fn static_bookmark_display(display_name: &str) -> Cow<'_, str> {
    if display_name.chars().count() <= TEXT_MAX_CHARS {
        Cow::Borrowed(display_name)
    } else {
        Cow::Owned(path_labels::compact_start_for_two_lines(
            display_name,
            TEXT_MAX_CHARS,
        ))
    }
}

fn wrap_bookmark_display(
    ui: &egui::Ui,
    text: &str,
    width: f32,
    font_id: &FontId,
    max_lines: usize,
) -> (Vec<String>, bool) {
    let mut lines = Vec::new();
    let mut rest = text.trim();
    while !rest.is_empty() && lines.len() < max_lines {
        let (line, next) = take_bookmark_display_line(ui, rest, width, font_id);
        if line.is_empty() {
            break;
        }
        lines.push(line.to_owned());
        rest = next.trim_start();
    }
    (lines, rest.is_empty())
}

fn take_bookmark_display_line<'a>(
    ui: &egui::Ui,
    text: &'a str,
    width: f32,
    font_id: &FontId,
) -> (&'a str, &'a str) {
    if text_width(ui, text, font_id) <= width {
        return (text.trim_end(), "");
    }

    let char_ends: Vec<usize> = text
        .char_indices()
        .map(|(index, ch)| index + ch.len_utf8())
        .collect();
    let last_fit = largest_fitting_prefix(ui, text, width, font_id, &char_ends);
    if last_fit == 0 {
        return ("", text);
    }

    let last_break = text[..last_fit]
        .char_indices()
        .filter_map(|(index, ch)| {
            matches!(ch, '\\' | '/' | ' ' | '|').then_some(index + ch.len_utf8())
        })
        .last();
    let end = last_break.unwrap_or(last_fit);

    (text[..end].trim_end(), &text[end..])
}

fn largest_fitting_prefix(
    ui: &egui::Ui,
    text: &str,
    width: f32,
    font_id: &FontId,
    char_ends: &[usize],
) -> usize {
    let mut best = 0;
    let mut low = 0;
    let mut high = char_ends.len();

    while low < high {
        let mid = low + (high - low) / 2;
        let end = char_ends[mid];
        if text_width(ui, &text[..end], font_id) <= width {
            best = end;
            low = mid + 1;
        } else {
            high = mid;
        }
    }

    best
}

fn text_width(ui: &egui::Ui, text: &str, font_id: &FontId) -> f32 {
    ui.painter()
        .layout_no_wrap(text.to_owned(), font_id.clone(), theme::TEXT_PRIMARY)
        .size()
        .x
}

struct MarqueeText<'a> {
    clip_rect: Rect,
    pos: egui::Pos2,
    text: &'a str,
    font_id: FontId,
    color: Color32,
    width: f32,
    started_at: f64,
}

fn draw_marquee_text(ui: &egui::Ui, text_run: MarqueeText<'_>) -> Option<Duration> {
    let MarqueeText {
        clip_rect,
        pos,
        text,
        font_id,
        color,
        width,
        started_at,
    } = text_run;

    if text.is_empty() {
        return None;
    }
    let galley = ui
        .painter()
        .layout_no_wrap(text.to_owned(), font_id.clone(), color);
    let overflow = (galley.size().x - width).max(0.0);
    if overflow <= 0.0 {
        ui.painter()
            .text(pos, Align2::LEFT_CENTER, text, font_id, color);
        return None;
    }

    let scroll_seconds = (overflow / MARQUEE_SPEED).max(MARQUEE_MIN_SCROLL_SECONDS);
    let cycle = scroll_seconds + MARQUEE_END_PAUSE_SECONDS;
    let elapsed = ((ui.input(|input| input.time) - started_at) as f32).max(0.0) % cycle;
    let scrolling = elapsed < scroll_seconds;
    let offset = if scrolling {
        overflow * (elapsed / scroll_seconds)
    } else {
        overflow
    };
    let text_clip = Rect::from_min_max(
        egui::pos2(pos.x, clip_rect.top()),
        egui::pos2(pos.x + width, clip_rect.bottom()),
    );
    ui.painter().with_clip_rect(text_clip).galley(
        egui::pos2(pos.x - offset, pos.y - galley.size().y * 0.5),
        galley,
        color,
    );
    if scrolling {
        Some(Duration::from_millis(16))
    } else {
        Some(Duration::from_secs_f32((cycle - elapsed).max(0.016)))
    }
}

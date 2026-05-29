use crate::core::state::{PageTransitionStyle, ReadingDirection};
use crate::core::worker::NavigationDirection;
use eframe::egui::{self, Color32, Pos2, Rect, Vec2};

#[derive(Debug, Clone, Copy)]
pub(in crate::app) struct TransitionPaintParams {
    pub(in crate::app) from_offset: Vec2,
    pub(in crate::app) from_scale: Vec2,
    pub(in crate::app) from_alpha: f32,
    pub(in crate::app) to_offset: Vec2,
    pub(in crate::app) to_scale: Vec2,
    pub(in crate::app) to_alpha: f32,
}

pub(in crate::app) fn transition_screen_sign(
    reading: ReadingDirection,
    direction: NavigationDirection,
) -> f32 {
    let forward_sign = match reading {
        ReadingDirection::LeftToRight => 1.0,
        ReadingDirection::RightToLeft => -1.0,
    };
    match direction {
        NavigationDirection::Forward => forward_sign,
        NavigationDirection::Backward => -forward_sign,
    }
}

pub(in crate::app) fn transition_paint_params(
    style: PageTransitionStyle,
    t: f32,
    sign: f32,
    viewport: Rect,
) -> TransitionPaintParams {
    let t = t.clamp(0.0, 1.0);
    match style {
        PageTransitionStyle::None => TransitionPaintParams {
            from_offset: Vec2::ZERO,
            from_scale: Vec2::splat(1.0),
            from_alpha: 0.0,
            to_offset: Vec2::ZERO,
            to_scale: Vec2::splat(1.0),
            to_alpha: 1.0,
        },
        PageTransitionStyle::SlideFade => {
            let distance = viewport.width() * 0.08;
            TransitionPaintParams {
                from_offset: Vec2::new(sign * distance * t, 0.0),
                from_scale: Vec2::splat(1.0),
                from_alpha: 1.0 - t,
                to_offset: Vec2::new(-sign * distance * (1.0 - t), 0.0),
                to_scale: Vec2::splat(1.0),
                to_alpha: t,
            }
        }
        PageTransitionStyle::Fade => TransitionPaintParams {
            from_offset: Vec2::ZERO,
            from_scale: Vec2::splat(1.0),
            from_alpha: 1.0 - t,
            to_offset: Vec2::ZERO,
            to_scale: Vec2::splat(1.0),
            to_alpha: t,
        },
        PageTransitionStyle::Push => {
            let distance = viewport.width();
            TransitionPaintParams {
                from_offset: Vec2::new(sign * distance * t, 0.0),
                from_scale: Vec2::splat(1.0),
                from_alpha: 1.0,
                to_offset: Vec2::new(-sign * distance * (1.0 - t), 0.0),
                to_scale: Vec2::splat(1.0),
                to_alpha: 1.0,
            }
        }
        PageTransitionStyle::ZoomFade => TransitionPaintParams {
            from_offset: Vec2::ZERO,
            from_scale: Vec2::splat(1.0 + 0.04 * t),
            from_alpha: 1.0 - t,
            to_offset: Vec2::ZERO,
            to_scale: Vec2::splat(0.96 + 0.04 * t),
            to_alpha: t,
        },
        PageTransitionStyle::BookFlip2d => {
            let distance = viewport.width() * 0.14;
            TransitionPaintParams {
                from_offset: Vec2::new(sign * distance * t, 0.0),
                from_scale: Vec2::new(1.0 - 0.18 * t, 1.0),
                from_alpha: 1.0 - 0.35 * t,
                to_offset: Vec2::new(-sign * distance * 0.5 * (1.0 - t), 0.0),
                to_scale: Vec2::new(0.92 + 0.08 * t, 1.0),
                to_alpha: t,
            }
        }
    }
}

pub(in crate::app) fn paint_book_flip_shadow(
    painter: &egui::Painter,
    viewport: Rect,
    sign: f32,
    t: f32,
) {
    let strength = 1.0 - (t * 2.0 - 1.0).abs();
    if strength <= 0.0 {
        return;
    }

    let travel = 0.15 + 0.65 * t;
    let x = if sign >= 0.0 {
        viewport.left() + viewport.width() * travel
    } else {
        viewport.right() - viewport.width() * travel
    };
    let width = (viewport.width() * 0.035).clamp(18.0, 54.0);
    let shadow = Rect::from_min_max(
        Pos2::new(x - width * 0.5, viewport.top()),
        Pos2::new(x + width * 0.5, viewport.bottom()),
    );
    painter.rect_filled(
        shadow,
        0.0,
        Color32::from_black_alpha((80.0 * strength) as u8),
    );
}

use super::{page_visual_size, spread_natural_size, PageVisual};
use crate::core::effects::{transformed_page_size, ViewTransform};
use egui::{Color32, ColorImage, Vec2};

#[test]
fn cpu_geometry_pending_and_failed_preserve_rotation_flip_and_spread_size() {
    let ctx = egui::Context::default();
    let texture = ctx.load_texture(
        "cpu-geometry-test",
        ColorImage::filled([1, 1], Color32::WHITE),
        Default::default(),
    );
    let neighbor = Vec2::new(580.0, 940.0);
    for rotation_quadrants in 0..4 {
        for flip_horizontal in [false, true] {
            for flip_vertical in [false, true] {
                let size = transformed_page_size(
                    1600.0,
                    720.0,
                    ViewTransform {
                        rotation_quadrants,
                        flip_horizontal,
                        flip_vertical,
                    },
                );
                let ready = PageVisual::Ready {
                    texture: texture.clone(),
                    size,
                    render_info: None,
                };
                let expected = spread_natural_size([page_visual_size(&ready), neighbor]);
                for visual in [
                    PageVisual::Loading {
                        index: 0,
                        size: Some(size),
                    },
                    PageVisual::Failed {
                        index: 0,
                        size: Some(size),
                        message: "memory budget".into(),
                    },
                ] {
                    assert_eq!(page_visual_size(&visual), page_visual_size(&ready));
                    assert_eq!(
                        spread_natural_size([page_visual_size(&visual), neighbor]),
                        expected
                    );
                }
            }
        }
    }
}

#[test]
fn cpu_geometry_unknown_source_retains_existing_placeholder() {
    for visual in [
        PageVisual::Loading {
            index: 0,
            size: None,
        },
        PageVisual::Failed {
            index: 0,
            size: None,
            message: "decode failed".into(),
        },
    ] {
        assert_eq!(page_visual_size(&visual), Vec2::new(900.0, 1300.0));
    }
}

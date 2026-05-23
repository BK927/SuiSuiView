use super::{recreate_dir, save_image, zip_dir, Args, FixtureFormat};
use image::{DynamicImage, ImageBuffer, Rgb};

pub(super) fn create(args: &Args) -> Result<(), String> {
    let comic_dir = args.out_dir.join("comic-folder");
    recreate_dir(&comic_dir)?;

    for index in 0..args.count {
        let format = if index % 3 == 0 {
            FixtureFormat::Jpeg
        } else {
            FixtureFormat::Png
        };
        let image = comic_image(index, args.min_long_edge);
        let path = comic_dir.join(format!("page-{index:04}.{}", format.extension()));
        save_image(&image, &path, format)?;
    }

    zip_dir(&comic_dir, &args.out_dir.join("comic.zip"))?;
    zip_dir(&comic_dir, &args.out_dir.join("comic.cbz"))?;

    println!("Created comic-like fixtures in {}", args.out_dir.display());
    println!("  comic-folder/");
    println!("  comic.zip");
    println!("  comic.cbz");
    Ok(())
}

fn comic_image(index: usize, min_long_edge: u32) -> DynamicImage {
    let width = (min_long_edge * 2 / 3).max(512);
    let height = min_long_edge.max(768);
    let mut image = ImageBuffer::from_pixel(width, height, Rgb([246, 246, 240]));
    let margin = width / 24;
    let gutter = width / 40;
    let panel_width = (width - margin * 2 - gutter) / 2;
    let panel_height = (height - margin * 2 - gutter * 2) / 3;

    for row in 0..3 {
        for col in 0..2 {
            let panel_index = row * 2 + col;
            let x = margin + col * (panel_width + gutter);
            let y = margin + row * (panel_height + gutter);
            draw_panel(
                &mut image,
                index,
                panel_index as usize,
                x,
                y,
                panel_width,
                panel_height,
            );
        }
    }

    DynamicImage::ImageRgb8(image)
}

fn draw_panel(
    image: &mut ImageBuffer<Rgb<u8>, Vec<u8>>,
    page_index: usize,
    panel_index: usize,
    x: u32,
    y: u32,
    width: u32,
    height: u32,
) {
    fill_rect(image, x, y, width, height, Rgb([252, 252, 248]));
    halftone_rect(
        image,
        x + 8,
        y + 8,
        width.saturating_sub(16),
        height.saturating_sub(16),
        13 + (panel_index as u32 % 4) * 3,
        3 + (page_index as u32 % 3),
    );
    stroke_rect(image, x, y, width, height, 4, Rgb([18, 18, 18]));

    let cx = x + width / 2;
    let cy = y + height / 2;
    for line in 0..10 {
        let offset = line * height / 11;
        draw_line(
            image,
            x + width / 8,
            y + offset,
            x + width - width / 10,
            y + height - offset / 2,
            Rgb([44, 44, 44]),
        );
    }

    fill_ellipse(
        image,
        cx.saturating_sub(width / 5),
        cy.saturating_sub(height / 4),
        width * 2 / 5,
        height / 2,
        Rgb([238, 238, 232]),
    );
    stroke_ellipse(
        image,
        cx.saturating_sub(width / 5),
        cy.saturating_sub(height / 4),
        width * 2 / 5,
        height / 2,
        3,
        Rgb([16, 16, 16]),
    );
    draw_line(
        image,
        cx - width / 16,
        cy - height / 16,
        cx - width / 4,
        cy + height / 4,
        Rgb([16, 16, 16]),
    );
    draw_line(
        image,
        cx + width / 16,
        cy - height / 16,
        cx + width / 4,
        cy + height / 4,
        Rgb([16, 16, 16]),
    );

    let bubble_x = x + width / 9 + (panel_index as u32 % 2) * width / 3;
    let bubble_y = y + height / 10;
    fill_ellipse(
        image,
        bubble_x,
        bubble_y,
        width / 2,
        height / 4,
        Rgb([255, 255, 255]),
    );
    stroke_ellipse(
        image,
        bubble_x,
        bubble_y,
        width / 2,
        height / 4,
        2,
        Rgb([18, 18, 18]),
    );
    draw_pseudo_text(
        image,
        bubble_x + width / 12,
        bubble_y + height / 18,
        width / 3,
        4 + panel_index as u32 % 3,
    );
}

fn fill_rect(
    image: &mut ImageBuffer<Rgb<u8>, Vec<u8>>,
    x: u32,
    y: u32,
    width: u32,
    height: u32,
    color: Rgb<u8>,
) {
    let max_x = (x + width).min(image.width());
    let max_y = (y + height).min(image.height());
    for yy in y..max_y {
        for xx in x..max_x {
            image.put_pixel(xx, yy, color);
        }
    }
}

fn stroke_rect(
    image: &mut ImageBuffer<Rgb<u8>, Vec<u8>>,
    x: u32,
    y: u32,
    width: u32,
    height: u32,
    thickness: u32,
    color: Rgb<u8>,
) {
    fill_rect(image, x, y, width, thickness, color);
    fill_rect(
        image,
        x,
        y + height.saturating_sub(thickness),
        width,
        thickness,
        color,
    );
    fill_rect(image, x, y, thickness, height, color);
    fill_rect(
        image,
        x + width.saturating_sub(thickness),
        y,
        thickness,
        height,
        color,
    );
}

fn halftone_rect(
    image: &mut ImageBuffer<Rgb<u8>, Vec<u8>>,
    x: u32,
    y: u32,
    width: u32,
    height: u32,
    spacing: u32,
    radius: u32,
) {
    let max_x = (x + width).min(image.width());
    let max_y = (y + height).min(image.height());
    let step = spacing.max(4) as usize;
    for yy in (y..max_y).step_by(step) {
        for xx in (x..max_x).step_by(step) {
            fill_ellipse(
                image,
                xx.saturating_sub(radius),
                yy.saturating_sub(radius),
                radius * 2 + 1,
                radius * 2 + 1,
                Rgb([210, 210, 205]),
            );
        }
    }
}

fn fill_ellipse(
    image: &mut ImageBuffer<Rgb<u8>, Vec<u8>>,
    x: u32,
    y: u32,
    width: u32,
    height: u32,
    color: Rgb<u8>,
) {
    ellipse(image, x, y, width, height, 1.0, color);
}

fn stroke_ellipse(
    image: &mut ImageBuffer<Rgb<u8>, Vec<u8>>,
    x: u32,
    y: u32,
    width: u32,
    height: u32,
    thickness: u32,
    color: Rgb<u8>,
) {
    let steps = thickness.max(1);
    for step in 0..steps {
        ellipse(
            image,
            x + step,
            y + step,
            width.saturating_sub(step * 2),
            height.saturating_sub(step * 2),
            0.10,
            color,
        );
    }
}

fn ellipse(
    image: &mut ImageBuffer<Rgb<u8>, Vec<u8>>,
    x: u32,
    y: u32,
    width: u32,
    height: u32,
    threshold: f32,
    color: Rgb<u8>,
) {
    if width == 0 || height == 0 {
        return;
    }
    let rx = width as f32 / 2.0;
    let ry = height as f32 / 2.0;
    let cx = x as f32 + rx;
    let cy = y as f32 + ry;
    let max_x = (x + width).min(image.width());
    let max_y = (y + height).min(image.height());
    for yy in y..max_y {
        for xx in x..max_x {
            let dx = (xx as f32 + 0.5 - cx) / rx.max(1.0);
            let dy = (yy as f32 + 0.5 - cy) / ry.max(1.0);
            let value = dx * dx + dy * dy;
            if (threshold >= 1.0 && value <= 1.0)
                || (threshold < 1.0 && (value - 1.0).abs() <= threshold)
            {
                image.put_pixel(xx, yy, color);
            }
        }
    }
}

fn draw_pseudo_text(
    image: &mut ImageBuffer<Rgb<u8>, Vec<u8>>,
    x: u32,
    y: u32,
    width: u32,
    lines: u32,
) {
    for line in 0..lines {
        let yy = y + line * 14;
        let line_width = width.saturating_sub((line % 3) * width / 5).max(width / 3);
        fill_rect(image, x, yy, line_width, 4, Rgb([22, 22, 22]));
    }
}

fn draw_line(
    image: &mut ImageBuffer<Rgb<u8>, Vec<u8>>,
    x0: u32,
    y0: u32,
    x1: u32,
    y1: u32,
    color: Rgb<u8>,
) {
    let mut x0 = x0 as i32;
    let mut y0 = y0 as i32;
    let x1 = x1 as i32;
    let y1 = y1 as i32;
    let dx = (x1 - x0).abs();
    let sx = if x0 < x1 { 1 } else { -1 };
    let dy = -(y1 - y0).abs();
    let sy = if y0 < y1 { 1 } else { -1 };
    let mut error = dx + dy;
    loop {
        if x0 >= 0 && y0 >= 0 && (x0 as u32) < image.width() && (y0 as u32) < image.height() {
            image.put_pixel(x0 as u32, y0 as u32, color);
        }
        if x0 == x1 && y0 == y1 {
            break;
        }
        let e2 = error * 2;
        if e2 >= dy {
            error += dy;
            x0 += sx;
        }
        if e2 <= dx {
            error += dx;
            y0 += sy;
        }
    }
}

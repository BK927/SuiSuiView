use super::{recreate_dir, save_image, zip_dir, Args, FixtureFormat};
use image::{DynamicImage, ImageBuffer, Rgb};

const GRID_SIZE: u32 = 9;
const PANEL_FRACTION_NUMERATOR: u32 = 88;
const PANEL_FRACTION_DENOMINATOR: u32 = 100;

pub(super) fn create(args: &Args) -> Result<(), String> {
    let identity_dir = args.out_dir.join("identity-folder");
    recreate_dir(&identity_dir)?;

    for page in 1..=args.count {
        let image = identity_image(page, args.min_long_edge);
        let path = identity_dir.join(format!("page-{page:04}.png"));
        save_image(&image, &path, FixtureFormat::Png)?;
    }

    zip_dir(&identity_dir, &args.out_dir.join("identity.zip"))?;
    zip_dir(&identity_dir, &args.out_dir.join("identity.cbz"))?;

    println!(
        "Created identity marker fixtures in {}",
        args.out_dir.display()
    );
    println!("  identity-folder/");
    println!("  identity.zip");
    println!("  identity.cbz");
    Ok(())
}

fn identity_image(page: usize, min_long_edge: u32) -> DynamicImage {
    let size = min_long_edge.max(768);
    let mut image = ImageBuffer::from_pixel(size, size, Rgb([132, 136, 142]));
    draw_identity_panel(&mut image, page);
    draw_page_bars(&mut image, page);
    DynamicImage::ImageRgb8(image)
}

fn draw_identity_panel(image: &mut ImageBuffer<Rgb<u8>, Vec<u8>>, page: usize) {
    let size = image.width().min(image.height());
    let panel = size * PANEL_FRACTION_NUMERATOR / PANEL_FRACTION_DENOMINATOR;
    let left = (image.width() - panel) / 2;
    let top = (image.height() - panel) / 2;
    let cell = (panel / GRID_SIZE).max(1);
    let pattern = identity_marker_pattern(page, GRID_SIZE as usize);

    fill_rect(image, left, top, panel, panel, Rgb([245, 246, 248]));
    for row in 0..GRID_SIZE {
        for col in 0..GRID_SIZE {
            let bit = pattern[(row * GRID_SIZE + col) as usize];
            let color = if bit {
                Rgb([14, 15, 18])
            } else {
                Rgb([246, 247, 249])
            };
            fill_rect(
                image,
                left + col * cell,
                top + row * cell,
                cell,
                cell,
                color,
            );
        }
    }
    stroke_rect(
        image,
        left,
        top,
        panel,
        panel,
        (size / 128).max(4),
        Rgb([20, 22, 26]),
    );
}

fn draw_page_bars(image: &mut ImageBuffer<Rgb<u8>, Vec<u8>>, page: usize) {
    let height = image.height();
    let width = image.width();
    let bar = (height / 48).max(8);
    for bit in 0..12 {
        if (page >> bit) & 1 == 1 {
            let x = width / 24 + bit as u32 * width / 16;
            fill_rect(
                image,
                x,
                height - bar * 3,
                width / 24,
                bar,
                Rgb([32, 80, 210]),
            );
        }
    }
}

fn identity_marker_pattern(page: usize, grid_size: usize) -> Vec<bool> {
    let cell_count = grid_size * grid_size;
    (0..cell_count)
        .map(|index| {
            splitmix64(
                (page as u64)
                    .wrapping_mul(0x9E37_79B9_7F4A_7C15)
                    .wrapping_add(index as u64),
            ) & 1
                == 1
        })
        .collect()
}

fn splitmix64(mut value: u64) -> u64 {
    value = value.wrapping_add(0x9E37_79B9_7F4A_7C15);
    value = (value ^ (value >> 30)).wrapping_mul(0xBF58_476D_1CE4_E5B9);
    value = (value ^ (value >> 27)).wrapping_mul(0x94D0_49BB_1331_11EB);
    value ^ (value >> 31)
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

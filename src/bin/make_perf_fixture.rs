use gif::{
    DisposalMethod as GifDisposalMethod, Encoder as GifEncoder, Frame as GifFrame,
    Repeat as GifRepeat,
};
use image::{DynamicImage, ImageBuffer, ImageFormat, Rgb};
use std::borrow::Cow;
use std::env;
use std::fs::{self, File};
use std::io::{self, Seek, Write};
use std::path::{Path, PathBuf};
#[cfg(feature = "bench-native-webp")]
use webp::{AnimEncoder as WebpAnimEncoder, AnimFrame as WebpAnimFrame, WebPConfig};
use zip::write::SimpleFileOptions;

#[path = "make_perf_fixture/comic.rs"]
mod comic;
#[path = "make_perf_fixture/identity.rs"]
mod identity;

const DEFAULT_COUNT: usize = 20;
const DEFAULT_MIN_LONG_EDGE: u32 = 4000;
const MAX_SEED_BYTES: u64 = 100 * 1024 * 1024;
const DEFAULT_MAX_SEED_LONG_EDGE: u32 = 8000;

#[derive(Debug, Clone, Copy)]
enum FixtureFormat {
    Jpeg,
    Png,
    Webp,
    Bmp,
    Gif,
    Ico,
    Svg,
}

impl FixtureFormat {
    const ALL: [Self; 7] = [
        Self::Jpeg,
        Self::Png,
        Self::Webp,
        Self::Bmp,
        Self::Gif,
        Self::Ico,
        Self::Svg,
    ];

    fn extension(self) -> &'static str {
        match self {
            Self::Jpeg => "jpg",
            Self::Png => "png",
            Self::Webp => "webp",
            Self::Bmp => "bmp",
            Self::Gif => "gif",
            Self::Ico => "ico",
            Self::Svg => "svg",
        }
    }

    fn image_format(self) -> Option<ImageFormat> {
        match self {
            Self::Jpeg => Some(ImageFormat::Jpeg),
            Self::Png => Some(ImageFormat::Png),
            Self::Webp => Some(ImageFormat::WebP),
            Self::Bmp => Some(ImageFormat::Bmp),
            Self::Gif => Some(ImageFormat::Gif),
            Self::Ico => Some(ImageFormat::Ico),
            Self::Svg => None,
        }
    }

    fn label(self) -> &'static str {
        match self {
            Self::Jpeg => "jpeg",
            Self::Png => "png",
            Self::Webp => "webp",
            Self::Bmp => "bmp",
            Self::Gif => "gif",
            Self::Ico => "ico",
            Self::Svg => "svg",
        }
    }

    fn parse(value: &str) -> Result<Self, String> {
        match value.trim().to_ascii_lowercase().as_str() {
            "jpg" | "jpeg" => Ok(Self::Jpeg),
            "png" => Ok(Self::Png),
            "webp" => Ok(Self::Webp),
            "bmp" => Ok(Self::Bmp),
            "gif" => Ok(Self::Gif),
            "ico" => Ok(Self::Ico),
            "svg" => Ok(Self::Svg),
            other => Err(format!("unknown fixture format: {other}")),
        }
    }
}

struct Args {
    out_dir: PathBuf,
    seed_dir: Option<PathBuf>,
    count: usize,
    min_long_edge: u32,
    profile: FixtureProfile,
    formats: Vec<FixtureFormat>,
}

#[derive(Debug, Default, Clone, Copy, PartialEq, Eq)]
enum FixtureProfile {
    #[default]
    Mixed,
    Comic,
    Animation,
    Identity,
}

impl FixtureProfile {
    fn parse(value: &str) -> Result<Self, String> {
        match value {
            "mixed" => Ok(Self::Mixed),
            "comic" => Ok(Self::Comic),
            "animation" => Ok(Self::Animation),
            "identity" => Ok(Self::Identity),
            _ => Err("--profile must be one of: mixed, comic, animation, identity".to_owned()),
        }
    }
}

fn main() -> Result<(), String> {
    let args = Args::parse()?;
    fs::create_dir_all(&args.out_dir).map_err(|error| error.to_string())?;
    clear_generated_archives(&args.out_dir, &args.formats)?;

    if args.profile == FixtureProfile::Comic {
        comic::create(&args)?;
        return Ok(());
    }
    if args.profile == FixtureProfile::Animation {
        create_animation_fixtures(&args.out_dir)?;
        return Ok(());
    }
    if args.profile == FixtureProfile::Identity {
        identity::create(&args)?;
        return Ok(());
    }

    let seeds = load_seed_paths(args.seed_dir.as_deref());
    let mixed_dir = args.out_dir.join("mixed-folder");
    recreate_dir(&mixed_dir)?;

    for index in 0..args.count {
        let format = args.formats[index % args.formats.len()];
        let image = page_image(index, args.min_long_edge, &seeds)?;
        let path = mixed_dir.join(format!("page-{index:04}.{}", format.extension()));
        save_image(&image, &path, format)?;
    }

    zip_dir(&mixed_dir, &args.out_dir.join("mixed.zip"))?;
    zip_dir(&mixed_dir, &args.out_dir.join("mixed.cbz"))?;

    let build_dir = args.out_dir.join("_format-build");
    recreate_dir(&build_dir)?;
    for format in args.formats.iter().copied() {
        let format_dir = build_dir.join(format.label());
        recreate_dir(&format_dir)?;
        for index in 0..args.count {
            let image = page_image(index, args.min_long_edge, &seeds)?;
            let path = format_dir.join(format!("page-{index:04}.{}", format.extension()));
            save_image(&image, &path, format)?;
        }
        zip_dir(
            &format_dir,
            &args.out_dir.join(format!("large-{}.cbz", format.label())),
        )?;
    }
    if build_dir.exists() {
        fs::remove_dir_all(&build_dir).map_err(|error| error.to_string())?;
    }

    println!("Created performance fixtures in {}", args.out_dir.display());
    println!("  mixed-folder/");
    println!("  mixed.zip");
    println!("  mixed.cbz");
    for format in args.formats.iter().copied() {
        println!("  large-{}.cbz", format.label());
    }
    Ok(())
}

impl Args {
    fn parse() -> Result<Self, String> {
        let mut out_dir = PathBuf::from("perf-fixtures");
        let mut seed_dir = None;
        let mut count = DEFAULT_COUNT;
        let mut min_long_edge = DEFAULT_MIN_LONG_EDGE;
        let mut profile = FixtureProfile::Mixed;
        let mut formats = FixtureFormat::ALL.to_vec();
        let mut args = env::args_os().skip(1);

        while let Some(arg) = args.next() {
            match arg.to_string_lossy().as_ref() {
                "--out" => {
                    out_dir = args
                        .next()
                        .map(PathBuf::from)
                        .ok_or("--out requires a path")?;
                }
                "--seed-dir" => {
                    seed_dir = args.next().map(PathBuf::from);
                }
                "--count" => {
                    count = args
                        .next()
                        .ok_or("--count requires a number")?
                        .to_string_lossy()
                        .parse()
                        .map_err(|_| "--count must be a positive integer")?;
                }
                "--min-long-edge" => {
                    min_long_edge = args
                        .next()
                        .ok_or("--min-long-edge requires a number")?
                        .to_string_lossy()
                        .parse()
                        .map_err(|_| "--min-long-edge must be a positive integer")?;
                }
                "--profile" => {
                    let value = args
                        .next()
                        .ok_or("--profile requires one of: mixed, comic, animation, identity")?;
                    profile = FixtureProfile::parse(&value.to_string_lossy())?;
                }
                "--formats" => {
                    let value = args
                        .next()
                        .ok_or("--formats requires comma-separated formats")?;
                    formats = parse_formats(&value.to_string_lossy())?;
                }
                "--help" | "-h" => {
                    print_help();
                    std::process::exit(0);
                }
                other => return Err(format!("Unknown argument: {other}")),
            }
        }

        Ok(Self {
            out_dir,
            seed_dir,
            count: count.max(1),
            min_long_edge: min_long_edge.max(256),
            profile,
            formats,
        })
    }
}

fn print_help() {
    println!("make_perf_fixture --out perf-fixtures --count 50 --min-long-edge 4000");
    println!(
        "  --profile mixed|comic|animation|identity chooses random format stress, comic-like line art, animated GIF/WebP, or marker pages"
    );
    println!("  --seed-dir <path> uses downloaded/source images when available");
    println!("  --formats jpeg,png,webp,bmp,gif,ico,svg limits generated archive formats");
}

fn parse_formats(value: &str) -> Result<Vec<FixtureFormat>, String> {
    let formats = value
        .split(',')
        .map(FixtureFormat::parse)
        .collect::<Result<Vec<_>, _>>()?;
    if formats.is_empty() {
        return Err("--formats must contain at least one format".to_owned());
    }
    Ok(formats)
}

fn clear_generated_archives(out_dir: &Path, formats: &[FixtureFormat]) -> Result<(), String> {
    let mut names = vec!["mixed.zip".to_owned(), "mixed.cbz".to_owned()];
    names.extend(
        formats
            .iter()
            .copied()
            .map(|format| format!("large-{}.cbz", format.label())),
    );

    for name in names {
        let path = out_dir.join(name);
        if path.exists() {
            fs::remove_file(&path).map_err(|error| error.to_string())?;
        }
    }
    Ok(())
}

fn create_animation_fixtures(out_dir: &Path) -> Result<(), String> {
    let animation_dir = out_dir.join("animation-folder");
    recreate_dir(&animation_dir)?;
    write_simple_gif(&animation_dir.join("animated-simple.gif"))?;
    write_dispose_gif(&animation_dir.join("animated-dispose.gif"))?;
    write_animation_webp(&animation_dir.join("animated-lossless.webp"))?;
    zip_dir(&animation_dir, &out_dir.join("animation.cbz"))?;

    println!(
        "Created animation decoder fixtures in {}",
        out_dir.display()
    );
    println!("  animation-folder/");
    println!("  animation.cbz");
    Ok(())
}

fn write_simple_gif(path: &Path) -> Result<(), String> {
    let width = 96;
    let height = 64;
    let palette = animation_palette();
    let mut file = File::create(path).map_err(|error| error.to_string())?;
    let mut encoder =
        GifEncoder::new(&mut file, width, height, &palette).map_err(|error| error.to_string())?;
    encoder
        .set_repeat(GifRepeat::Infinite)
        .map_err(|error| error.to_string())?;

    for index in 0..8 {
        let mut pixels = vec![4u8; width as usize * height as usize];
        let square_x = 8 + index * 9;
        for y in 18..42 {
            for x in square_x..square_x + 18 {
                pixels[y as usize * width as usize + x as usize] = 1 + (index % 3) as u8;
            }
        }
        let mut frame = GifFrame::default();
        frame.width = width;
        frame.height = height;
        frame.delay = 4 + index as u16;
        frame.dispose = GifDisposalMethod::Keep;
        frame.buffer = Cow::Owned(pixels);
        encoder
            .write_frame(&frame)
            .map_err(|error| error.to_string())?;
    }
    Ok(())
}

fn write_dispose_gif(path: &Path) -> Result<(), String> {
    let width = 96;
    let height = 64;
    let palette = animation_palette();
    let mut file = File::create(path).map_err(|error| error.to_string())?;
    let mut encoder =
        GifEncoder::new(&mut file, width, height, &palette).map_err(|error| error.to_string())?;
    encoder
        .set_repeat(GifRepeat::Infinite)
        .map_err(|error| error.to_string())?;

    for index in 0..7 {
        let frame_width = 32;
        let frame_height = 24;
        let left = 4 + index * 9;
        let top = 8 + (index % 3) * 10;
        let mut pixels = vec![0u8; frame_width as usize * frame_height as usize];
        for y in 2..frame_height - 2 {
            for x in 2..frame_width - 2 {
                let edge = x < 5 || x >= frame_width - 5 || y < 5 || y >= frame_height - 5;
                pixels[y as usize * frame_width as usize + x as usize] =
                    if edge { 3 } else { 1 + (index % 3) as u8 };
            }
        }
        let mut frame = GifFrame::default();
        frame.left = left;
        frame.top = top;
        frame.width = frame_width;
        frame.height = frame_height;
        frame.delay = 6;
        frame.dispose = if index % 2 == 0 {
            GifDisposalMethod::Background
        } else {
            GifDisposalMethod::Previous
        };
        frame.transparent = Some(0);
        frame.buffer = Cow::Owned(pixels);
        encoder
            .write_frame(&frame)
            .map_err(|error| error.to_string())?;
    }
    Ok(())
}

fn animation_palette() -> Vec<u8> {
    vec![
        0, 0, 0, // transparent slot
        226, 54, 80, // red
        35, 169, 122, // green
        50, 98, 220, // blue
        245, 208, 78, // yellow background
    ]
}

#[cfg(feature = "bench-native-webp")]
fn write_animation_webp(path: &Path) -> Result<(), String> {
    let width = 96;
    let height = 64;
    let mut config = WebPConfig::new().map_err(|_| "failed to create WebPConfig".to_owned())?;
    config.lossless = 1;
    config.quality = 90.0;
    config.alpha_compression = 0;
    config.alpha_filtering = 0;

    let mut buffers = Vec::new();
    for index in 0..8 {
        buffers.push(animation_rgba_frame(width, height, index));
    }

    let mut encoder = WebpAnimEncoder::new(width, height, &config);
    encoder.set_bgcolor([0, 0, 0, 0]);
    encoder.set_loop_count(0);
    for (index, frame) in buffers.iter().enumerate() {
        let timestamp_ms = 1000 + index as i32 * 90;
        encoder.add_frame(WebpAnimFrame::from_rgba(frame, width, height, timestamp_ms));
    }

    let encoded = encoder
        .try_encode()
        .map_err(|error| format!("failed to encode animated WebP fixture: {error:?}"))?;
    fs::write(path, &*encoded).map_err(|error| error.to_string())
}

#[cfg(not(feature = "bench-native-webp"))]
fn write_animation_webp(_path: &Path) -> Result<(), String> {
    Err(
        "--profile animation requires --features bench-native-webp to generate animated WebP"
            .to_owned(),
    )
}

#[cfg(feature = "bench-native-webp")]
fn animation_rgba_frame(width: u32, height: u32, index: usize) -> Vec<u8> {
    let mut pixels = vec![0u8; width as usize * height as usize * 4];
    for y in 0..height {
        for x in 0..width {
            let offset = (y as usize * width as usize + x as usize) * 4;
            pixels[offset] = ((x * 3 + index as u32 * 17) % 256) as u8;
            pixels[offset + 1] = ((y * 4 + index as u32 * 29) % 256) as u8;
            pixels[offset + 2] = ((x + y + index as u32 * 41) % 256) as u8;
            pixels[offset + 3] = 255;
        }
    }

    let square_x = 8 + index as u32 * 8;
    for y in 18..42 {
        for x in square_x..(square_x + 18).min(width) {
            let offset = (y as usize * width as usize + x as usize) * 4;
            pixels[offset..offset + 4].copy_from_slice(&[255, 255, 255, 220]);
        }
    }
    pixels
}

fn load_seed_paths(seed_dir: Option<&Path>) -> Vec<PathBuf> {
    let Some(seed_dir) = seed_dir else {
        return Vec::new();
    };

    fs::read_dir(seed_dir)
        .ok()
        .into_iter()
        .flat_map(|entries| entries.flatten())
        .map(|entry| entry.path())
        .filter(|path| path.is_file())
        .collect()
}

fn page_image(index: usize, min_long_edge: u32, seeds: &[PathBuf]) -> Result<DynamicImage, String> {
    for offset in 0..seeds.len() {
        let seed = &seeds[(index + offset) % seeds.len()];
        if seed_is_reasonable(seed) {
            if let Ok(image) = image::open(seed) {
                return Ok(normalize_seed_image(image, min_long_edge));
            }
        }
    }

    Ok(synthetic_image(index, min_long_edge))
}

fn synthetic_image(index: usize, min_long_edge: u32) -> DynamicImage {
    let width = min_long_edge + ((index % 4) as u32 * 320);
    let height = (min_long_edge * 3 / 2) + ((index % 5) as u32 * 257);
    let image = ImageBuffer::from_fn(width, height, |x, y| {
        let r = ((x / 13 + y / 29 + index as u32 * 17) % 256) as u8;
        let g = ((x / 7 + index as u32 * 31) % 256) as u8;
        let b = ((y / 11 + index as u32 * 47) % 256) as u8;
        Rgb([r, g, b])
    });
    DynamicImage::ImageRgb8(image)
}

fn seed_is_reasonable(path: &Path) -> bool {
    fs::metadata(path)
        .map(|metadata| metadata.len() <= MAX_SEED_BYTES)
        .unwrap_or(false)
}

fn normalize_seed_image(image: DynamicImage, min_long_edge: u32) -> DynamicImage {
    let longest = image.width().max(image.height());
    if longest < min_long_edge {
        return resize_to_long_edge(image, min_long_edge);
    }

    let max_long_edge = min_long_edge.max(DEFAULT_MAX_SEED_LONG_EDGE);
    if longest > max_long_edge {
        return resize_to_long_edge(image, max_long_edge);
    }

    image
}

fn resize_to_long_edge(image: DynamicImage, long_edge: u32) -> DynamicImage {
    let longest = image.width().max(image.height());
    if longest == long_edge {
        return image;
    }

    let scale = long_edge as f32 / longest as f32;
    let width = (image.width() as f32 * scale).round().max(1.0) as u32;
    let height = (image.height() as f32 * scale).round().max(1.0) as u32;
    image.resize_exact(width, height, image::imageops::FilterType::Triangle)
}

fn save_image(image: &DynamicImage, path: &Path, format: FixtureFormat) -> Result<(), String> {
    if matches!(format, FixtureFormat::Svg) {
        return save_svg_fixture(image.width(), image.height(), path);
    }

    let ico_image;
    let image = if matches!(format, FixtureFormat::Ico) {
        ico_image = DynamicImage::ImageRgba8(
            image
                .resize(256, 256, image::imageops::FilterType::Triangle)
                .to_rgba8(),
        );
        &ico_image
    } else {
        image
    };

    image
        .save_with_format(
            path,
            format
                .image_format()
                .ok_or("fixture format does not map to image crate")?,
        )
        .map_err(|error| format!("failed to write {}: {error}", path.display()))
}

fn save_svg_fixture(width: u32, height: u32, path: &Path) -> Result<(), String> {
    let svg = format!(
        r##"<svg xmlns="http://www.w3.org/2000/svg" width="{width}" height="{height}" viewBox="0 0 {width} {height}">
<rect width="100%" height="100%" fill="#f7f7f2"/>
<path d="M0 {mid} C {c1} 0, {c2} {height}, {width} {mid}" fill="none" stroke="#1f2937" stroke-width="12"/>
<g fill="#d946ef" fill-opacity="0.45">
  <circle cx="{cx1}" cy="{cy1}" r="{r1}"/>
  <circle cx="{cx2}" cy="{cy2}" r="{r2}"/>
</g>
<text x="48" y="96" font-size="64" fill="#111827">SuiSuiView SVG decoder bench</text>
</svg>
"##,
        mid = height / 2,
        c1 = width / 3,
        c2 = width * 2 / 3,
        cx1 = width / 4,
        cy1 = height / 3,
        r1 = width.min(height) / 8,
        cx2 = width * 3 / 4,
        cy2 = height * 2 / 3,
        r2 = width.min(height) / 10,
    );
    fs::write(path, svg).map_err(|error| format!("failed to write {}: {error}", path.display()))
}

fn recreate_dir(path: &Path) -> Result<(), String> {
    if path.exists() {
        fs::remove_dir_all(path).map_err(|error| error.to_string())?;
    }
    fs::create_dir_all(path).map_err(|error| error.to_string())
}

fn zip_dir(source_dir: &Path, output: &Path) -> Result<(), String> {
    let file = File::create(output).map_err(|error| error.to_string())?;
    let mut zip = zip::ZipWriter::new(file);
    let options = SimpleFileOptions::default().compression_method(zip::CompressionMethod::Deflated);
    add_dir_to_zip(&mut zip, source_dir, source_dir, options).map_err(|error| error.to_string())?;
    zip.finish().map_err(|error| error.to_string())?;
    Ok(())
}

fn add_dir_to_zip<W: Write + Seek>(
    zip: &mut zip::ZipWriter<W>,
    root: &Path,
    current: &Path,
    options: SimpleFileOptions,
) -> io::Result<()> {
    for entry in fs::read_dir(current)? {
        let entry = entry?;
        let path = entry.path();
        if path.is_dir() {
            add_dir_to_zip(zip, root, &path, options)?;
            continue;
        }

        let relative = path.strip_prefix(root).unwrap_or(&path);
        let name = relative.to_string_lossy().replace('\\', "/");
        zip.start_file(name, options)?;
        let mut input = File::open(&path)?;
        io::copy(&mut input, zip)?;
    }
    Ok(())
}

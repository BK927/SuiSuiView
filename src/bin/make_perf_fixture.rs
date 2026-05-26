use image::{DynamicImage, ImageBuffer, ImageFormat, Rgb};
use std::env;
use std::fs::{self, File};
use std::io::{self, Seek, Write};
use std::path::{Path, PathBuf};
use zip::write::SimpleFileOptions;

#[path = "make_perf_fixture/comic.rs"]
mod comic;

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
}

impl FixtureFormat {
    const ALL: [Self; 5] = [Self::Jpeg, Self::Png, Self::Webp, Self::Bmp, Self::Gif];

    fn extension(self) -> &'static str {
        match self {
            Self::Jpeg => "jpg",
            Self::Png => "png",
            Self::Webp => "webp",
            Self::Bmp => "bmp",
            Self::Gif => "gif",
        }
    }

    fn image_format(self) -> ImageFormat {
        match self {
            Self::Jpeg => ImageFormat::Jpeg,
            Self::Png => ImageFormat::Png,
            Self::Webp => ImageFormat::WebP,
            Self::Bmp => ImageFormat::Bmp,
            Self::Gif => ImageFormat::Gif,
        }
    }

    fn label(self) -> &'static str {
        match self {
            Self::Jpeg => "jpeg",
            Self::Png => "png",
            Self::Webp => "webp",
            Self::Bmp => "bmp",
            Self::Gif => "gif",
        }
    }

    fn parse(value: &str) -> Result<Self, String> {
        match value.trim().to_ascii_lowercase().as_str() {
            "jpg" | "jpeg" => Ok(Self::Jpeg),
            "png" => Ok(Self::Png),
            "webp" => Ok(Self::Webp),
            "bmp" => Ok(Self::Bmp),
            "gif" => Ok(Self::Gif),
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
}

impl FixtureProfile {
    fn parse(value: &str) -> Result<Self, String> {
        match value {
            "mixed" => Ok(Self::Mixed),
            "comic" => Ok(Self::Comic),
            _ => Err("--profile must be one of: mixed, comic".to_owned()),
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
    for format in args
        .formats
        .iter()
        .copied()
        .filter(|format| !matches!(format, FixtureFormat::Gif))
    {
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
    for format in args
        .formats
        .iter()
        .copied()
        .filter(|format| !matches!(format, FixtureFormat::Gif))
    {
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
                        .ok_or("--profile requires one of: mixed, comic")?;
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
    println!("  --profile mixed|comic chooses random format stress or comic-like line art");
    println!("  --seed-dir <path> uses downloaded/source images when available");
    println!("  --formats jpeg,png,webp limits generated archive formats");
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
            .filter(|format| !matches!(format, FixtureFormat::Gif))
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
    image
        .save_with_format(path, format.image_format())
        .map_err(|error| format!("failed to write {}: {error}", path.display()))
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

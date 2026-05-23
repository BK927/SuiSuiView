mod app;
mod core;

use crate::core::state::{StateStore, WindowPlacement};
use crate::core::worker::{DecodeStrategy, DEFAULT_TARGET_LONG_EDGE, MIN_TARGET_LONG_EDGE};
use app::SuiSuiViewApp;
use std::ffi::OsString;
use std::path::PathBuf;
use std::sync::Arc;

const DEFAULT_WINDOW_SIZE: [f32; 2] = [1280.0, 820.0];
const MIN_WINDOW_SIZE: [f32; 2] = [860.0, 560.0];

fn main() -> eframe::Result<()> {
    if let Some(command) = CliCommand::parse(std::env::args_os().skip(1).collect()) {
        match command {
            CliCommand::PerfScan {
                path,
                report_path,
                target_long_edge,
                decode_strategy,
            } => {
                if let Err(error) = core::perf::run_perf_scan(
                    &path,
                    report_path.as_deref(),
                    target_long_edge,
                    decode_strategy,
                ) {
                    eprintln!("perf scan failed: {error}");
                    std::process::exit(1);
                }
                return Ok(());
            }
            CliCommand::QualityScan {
                path,
                target_long_edge,
                report_path,
            } => {
                if let Err(error) =
                    core::quality::run_quality_scan(&path, target_long_edge, report_path.as_deref())
                {
                    eprintln!("quality scan failed: {error}");
                    std::process::exit(1);
                }
                return Ok(());
            }
            CliCommand::EffectBench {
                path,
                target_long_edge,
                report_path,
            } => {
                if let Err(error) = core::effect_bench::run_effect_bench(
                    &path,
                    report_path.as_deref(),
                    target_long_edge,
                ) {
                    eprintln!("effect bench failed: {error}");
                    std::process::exit(1);
                }
                return Ok(());
            }
            CliCommand::UpscaleBench {
                path,
                source_long_edge,
                target_long_edge,
                report_path,
            } => {
                if let Err(error) = core::upscale_bench::run_upscale_bench(
                    &path,
                    report_path.as_deref(),
                    source_long_edge,
                    target_long_edge,
                ) {
                    eprintln!("upscale bench failed: {error}");
                    std::process::exit(1);
                }
                return Ok(());
            }
            CliCommand::UpscaleQualityScan {
                path,
                source_long_edge,
                target_long_edge,
                report_path,
                visual_dir,
            } => {
                if let Err(error) = core::upscale_quality::run_upscale_quality_scan(
                    &path,
                    report_path.as_deref(),
                    visual_dir.as_deref(),
                    source_long_edge,
                    target_long_edge,
                ) {
                    eprintln!("upscale quality scan failed: {error}");
                    std::process::exit(1);
                }
                return Ok(());
            }
        }
    }

    let store = StateStore::load();
    let options = eframe::NativeOptions {
        viewport: initial_viewport(&store, window_icon()),
        renderer: eframe::Renderer::Wgpu,
        persist_window: false,
        ..Default::default()
    };

    eframe::run_native(
        "SuiSuiView",
        options,
        Box::new(|cc| Ok(Box::new(SuiSuiViewApp::new(cc, store)))),
    )
}

fn initial_viewport(
    store: &StateStore,
    icon: eframe::egui::IconData,
) -> eframe::egui::ViewportBuilder {
    let placement = store.window_placement();
    let inner_size = valid_window_size(placement).unwrap_or(DEFAULT_WINDOW_SIZE);
    let mut viewport = eframe::egui::ViewportBuilder::default()
        .with_inner_size(inner_size)
        .with_min_inner_size(MIN_WINDOW_SIZE)
        .with_clamp_size_to_monitor_size(true)
        .with_icon(Arc::new(icon));

    if let Some(position) = valid_window_position(placement) {
        viewport = viewport.with_position(position);
    }
    if placement.maximized {
        viewport = viewport.with_maximized(true);
    }
    viewport
}

fn valid_window_size(placement: &WindowPlacement) -> Option<[f32; 2]> {
    let [width, height] = placement.inner_size?;
    (width.is_finite()
        && height.is_finite()
        && width >= MIN_WINDOW_SIZE[0]
        && height >= MIN_WINDOW_SIZE[1])
        .then_some([width, height])
}

fn valid_window_position(placement: &WindowPlacement) -> Option<[f32; 2]> {
    let [x, y] = placement.outer_position?;
    (x.is_finite() && y.is_finite()).then_some([x, y])
}

fn window_icon() -> eframe::egui::IconData {
    let image = image::load_from_memory(include_bytes!("../assets/app-icon.png"))
        .expect("embedded app icon should be a valid PNG")
        .into_rgba8();
    let width = image.width();
    let height = image.height();

    eframe::egui::IconData {
        rgba: image.into_raw(),
        width,
        height,
    }
}

enum CliCommand {
    PerfScan {
        path: PathBuf,
        report_path: Option<PathBuf>,
        target_long_edge: u32,
        decode_strategy: DecodeStrategy,
    },
    QualityScan {
        path: PathBuf,
        target_long_edge: u32,
        report_path: Option<PathBuf>,
    },
    EffectBench {
        path: PathBuf,
        target_long_edge: u32,
        report_path: Option<PathBuf>,
    },
    UpscaleBench {
        path: PathBuf,
        source_long_edge: u32,
        target_long_edge: u32,
        report_path: Option<PathBuf>,
    },
    UpscaleQualityScan {
        path: PathBuf,
        source_long_edge: u32,
        target_long_edge: u32,
        report_path: Option<PathBuf>,
        visual_dir: Option<PathBuf>,
    },
}

impl CliCommand {
    fn parse(args: Vec<OsString>) -> Option<Self> {
        let mut args = args.into_iter();
        let first = args.next()?;
        if first == "--perf-scan" {
            return Some(Self::parse_perf_scan(args));
        }
        if first == "--quality-scan" {
            return Some(Self::parse_quality_scan(args));
        }
        if first == "--effect-bench" {
            return Some(Self::parse_effect_bench(args));
        }
        if first == "--upscale-bench" {
            return Some(Self::parse_upscale_bench(args));
        }
        if first == "--upscale-quality-scan" {
            return Some(Self::parse_upscale_quality_scan(args));
        }

        None
    }

    fn parse_perf_scan(mut args: impl Iterator<Item = OsString>) -> Self {
        let path = args.next().map(PathBuf::from).unwrap_or_else(|| {
            eprintln!(
                "usage: suisuiview --perf-scan <path> [--perf-report <report.json>] [--decode-strategy auto|image-crate]"
            );
            std::process::exit(2);
        });

        let mut report_path = None;
        let mut target_long_edge = DEFAULT_TARGET_LONG_EDGE;
        let mut decode_strategy = DecodeStrategy::Auto;
        while let Some(arg) = args.next() {
            if arg == "--perf-report" {
                report_path = Some(args.next().map(PathBuf::from).unwrap_or_else(|| {
                    eprintln!("--perf-report requires a path");
                    std::process::exit(2);
                }));
            } else if arg == "--perf-report-default" {
                report_path = Some(core::perf::default_report_path());
            } else if arg == "--target-long-edge" {
                target_long_edge = args
                    .next()
                    .and_then(|value| value.to_string_lossy().parse().ok())
                    .unwrap_or_else(|| {
                        eprintln!("--target-long-edge requires a positive integer");
                        std::process::exit(2);
                    });
            } else if arg == "--decode-strategy" {
                decode_strategy = args
                    .next()
                    .and_then(|value| DecodeStrategy::parse_cli(&value.to_string_lossy()))
                    .unwrap_or_else(|| {
                        eprintln!("--decode-strategy requires one of: auto, image-crate");
                        std::process::exit(2);
                    });
            } else {
                eprintln!("unknown argument: {}", arg.to_string_lossy());
                std::process::exit(2);
            }
        }

        Self::PerfScan {
            path,
            report_path,
            target_long_edge,
            decode_strategy,
        }
    }

    fn parse_quality_scan(mut args: impl Iterator<Item = OsString>) -> Self {
        let path = args.next().map(PathBuf::from).unwrap_or_else(|| {
            eprintln!(
                "usage: suisuiview --quality-scan <path> [--target-long-edge <px>] [--quality-report <report.json>]"
            );
            std::process::exit(2);
        });

        let mut target_long_edge = DEFAULT_TARGET_LONG_EDGE;
        let mut report_path = None;
        while let Some(arg) = args.next() {
            if arg == "--target-long-edge" {
                target_long_edge = args
                    .next()
                    .and_then(|value| value.to_string_lossy().parse().ok())
                    .unwrap_or_else(|| {
                        eprintln!("--target-long-edge requires a positive integer");
                        std::process::exit(2);
                    });
            } else if arg == "--quality-report" {
                report_path = Some(args.next().map(PathBuf::from).unwrap_or_else(|| {
                    eprintln!("--quality-report requires a path");
                    std::process::exit(2);
                }));
            } else {
                eprintln!("unknown argument: {}", arg.to_string_lossy());
                std::process::exit(2);
            }
        }

        Self::QualityScan {
            path,
            target_long_edge,
            report_path,
        }
    }

    fn parse_effect_bench(mut args: impl Iterator<Item = OsString>) -> Self {
        let path = args.next().map(PathBuf::from).unwrap_or_else(|| {
            eprintln!(
                "usage: suisuiview --effect-bench <path> [--target-long-edge <px>] [--effect-report <report.json>]"
            );
            std::process::exit(2);
        });

        let mut target_long_edge = DEFAULT_TARGET_LONG_EDGE;
        let mut report_path = None;
        while let Some(arg) = args.next() {
            if arg == "--target-long-edge" {
                target_long_edge = args
                    .next()
                    .and_then(|value| value.to_string_lossy().parse().ok())
                    .unwrap_or_else(|| {
                        eprintln!("--target-long-edge requires a positive integer");
                        std::process::exit(2);
                    });
            } else if arg == "--effect-report" {
                report_path = Some(args.next().map(PathBuf::from).unwrap_or_else(|| {
                    eprintln!("--effect-report requires a path");
                    std::process::exit(2);
                }));
            } else if arg == "--effect-report-default" {
                report_path = Some(core::effect_bench::default_effect_report_path());
            } else {
                eprintln!("unknown argument: {}", arg.to_string_lossy());
                std::process::exit(2);
            }
        }

        Self::EffectBench {
            path,
            target_long_edge,
            report_path,
        }
    }

    fn parse_upscale_bench(mut args: impl Iterator<Item = OsString>) -> Self {
        let path = args.next().map(PathBuf::from).unwrap_or_else(|| {
            eprintln!(
                "usage: suisuiview --upscale-bench <path> [--source-long-edge <px>] [--target-long-edge <px>] [--upscale-report <report.json>]"
            );
            std::process::exit(2);
        });

        let mut target_long_edge = DEFAULT_TARGET_LONG_EDGE;
        let mut source_long_edge = None;
        let mut report_path = None;
        while let Some(arg) = args.next() {
            if arg == "--target-long-edge" {
                target_long_edge = args
                    .next()
                    .and_then(|value| value.to_string_lossy().parse().ok())
                    .unwrap_or_else(|| {
                        eprintln!("--target-long-edge requires a positive integer");
                        std::process::exit(2);
                    });
            } else if arg == "--source-long-edge" {
                source_long_edge = Some(
                    args.next()
                        .and_then(|value| value.to_string_lossy().parse().ok())
                        .unwrap_or_else(|| {
                            eprintln!("--source-long-edge requires a positive integer");
                            std::process::exit(2);
                        }),
                );
            } else if arg == "--upscale-report" {
                report_path = Some(args.next().map(PathBuf::from).unwrap_or_else(|| {
                    eprintln!("--upscale-report requires a path");
                    std::process::exit(2);
                }));
            } else if arg == "--upscale-report-default" {
                report_path = Some(core::upscale_bench::default_upscale_report_path());
            } else {
                eprintln!("unknown argument: {}", arg.to_string_lossy());
                std::process::exit(2);
            }
        }

        Self::UpscaleBench {
            path,
            source_long_edge: source_long_edge
                .unwrap_or_else(|| (target_long_edge / 2).max(MIN_TARGET_LONG_EDGE)),
            target_long_edge,
            report_path,
        }
    }

    fn parse_upscale_quality_scan(mut args: impl Iterator<Item = OsString>) -> Self {
        let path = args.next().map(PathBuf::from).unwrap_or_else(|| {
            eprintln!(
                "usage: suisuiview --upscale-quality-scan <path> [--source-long-edge <px>] [--target-long-edge <px>] [--upscale-quality-report <report.json>] [--upscale-quality-visuals <dir>]"
            );
            std::process::exit(2);
        });

        let mut target_long_edge = DEFAULT_TARGET_LONG_EDGE;
        let mut source_long_edge = None;
        let mut report_path = None;
        let mut visual_dir = None;
        while let Some(arg) = args.next() {
            if arg == "--target-long-edge" {
                target_long_edge = args
                    .next()
                    .and_then(|value| value.to_string_lossy().parse().ok())
                    .unwrap_or_else(|| {
                        eprintln!("--target-long-edge requires a positive integer");
                        std::process::exit(2);
                    });
            } else if arg == "--source-long-edge" {
                source_long_edge = Some(
                    args.next()
                        .and_then(|value| value.to_string_lossy().parse().ok())
                        .unwrap_or_else(|| {
                            eprintln!("--source-long-edge requires a positive integer");
                            std::process::exit(2);
                        }),
                );
            } else if arg == "--upscale-quality-report" {
                report_path = Some(args.next().map(PathBuf::from).unwrap_or_else(|| {
                    eprintln!("--upscale-quality-report requires a path");
                    std::process::exit(2);
                }));
            } else if arg == "--upscale-quality-report-default" {
                report_path = Some(core::upscale_quality::default_upscale_quality_report_path());
            } else if arg == "--upscale-quality-visuals" {
                visual_dir = Some(args.next().map(PathBuf::from).unwrap_or_else(|| {
                    eprintln!("--upscale-quality-visuals requires a directory");
                    std::process::exit(2);
                }));
            } else {
                eprintln!("unknown argument: {}", arg.to_string_lossy());
                std::process::exit(2);
            }
        }

        Self::UpscaleQualityScan {
            path,
            source_long_edge: source_long_edge
                .unwrap_or_else(|| (target_long_edge / 2).max(MIN_TARGET_LONG_EDGE)),
            target_long_edge,
            report_path,
            visual_dir,
        }
    }
}

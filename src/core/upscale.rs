use crate::core::perf_trace::{self, PerfField};
use crate::core::source::SharedSource;
use crate::core::state::NcnnRealEsrganSettings;
use crate::core::worker::{prepare_image_with_options, DecodeOptions, PreparedPage};
use cache::{read_cached_output, store_cached_output, AiUpscaleCacheEntry};
use crossbeam_channel::{bounded, unbounded, Receiver, Sender};
use eframe::egui::Context;
use image::{DynamicImage, ImageFormat};
use std::fs::{self, File};
use std::path::{Path, PathBuf};
use std::process::{Command, ExitStatus, Stdio};
use std::sync::{
    atomic::{AtomicBool, Ordering},
    Arc,
};
use std::thread::{self, JoinHandle};
use std::time::{Duration, Instant, SystemTime, UNIX_EPOCH};

mod cache;

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct NcnnRealEsrganCommand {
    pub program: PathBuf,
    pub args: Vec<String>,
}

impl NcnnRealEsrganCommand {
    pub fn to_command(&self) -> Command {
        let mut command = Command::new(&self.program);
        command.args(&self.args);
        command
    }
}

pub fn build_ncnn_realesrgan_command(
    settings: &NcnnRealEsrganSettings,
    input_path: &Path,
    output_path: &Path,
) -> Result<NcnnRealEsrganCommand, String> {
    let executable = settings.executable_path.trim();
    if executable.is_empty() {
        return Err("Real-ESRGAN 실행 파일 경로가 비어 있습니다.".to_owned());
    }

    let model_name = settings.model_name.trim();
    if model_name.is_empty() {
        return Err("Real-ESRGAN 모델 이름이 비어 있습니다.".to_owned());
    }

    let scale = settings.scale.clamp(2, 4);
    let tile_size = normalized_tile_size(settings.tile_size);
    let output_format = normalized_output_format(&settings.output_format);

    let mut args = vec![
        "-i".to_owned(),
        path_arg(input_path),
        "-o".to_owned(),
        path_arg(output_path),
        "-n".to_owned(),
        model_name.to_owned(),
        "-s".to_owned(),
        scale.to_string(),
        "-t".to_owned(),
        tile_size.to_string(),
        "-f".to_owned(),
        output_format,
    ];

    let model_path = settings.model_path.trim();
    if !model_path.is_empty() {
        args.push("-m".to_owned());
        args.push(model_path.to_owned());
    }

    Ok(NcnnRealEsrganCommand {
        program: PathBuf::from(executable),
        args,
    })
}

pub struct AiUpscaleWorker {
    command_tx: Sender<UpscaleCommand>,
    event_rx: Receiver<UpscaleEvent>,
    shutdown_requested: Arc<AtomicBool>,
    stopped_rx: Receiver<()>,
    join: Option<JoinHandle<()>>,
}

impl AiUpscaleWorker {
    pub fn new(ctx: Context) -> Self {
        let (command_tx, command_rx) = unbounded();
        let (event_tx, event_rx) = unbounded();
        let (stopped_tx, stopped_rx) = bounded(1);
        let shutdown_requested = Arc::new(AtomicBool::new(false));
        let worker_shutdown_requested = shutdown_requested.clone();
        let join = thread::Builder::new()
            .name("suisuiview-ai-upscale-worker".to_owned())
            .spawn(move || {
                run_upscale_worker(command_rx, event_tx, ctx, worker_shutdown_requested);
                let _ = stopped_tx.send(());
            })
            .expect("AI upscale worker thread should start");

        Self {
            command_tx,
            event_rx,
            shutdown_requested,
            stopped_rx,
            join: Some(join),
        }
    }

    pub fn upscale(&self, request: UpscaleRequest) {
        let _ = self.command_tx.send(UpscaleCommand::Run(request));
    }

    pub fn try_recv(&self) -> Option<UpscaleEvent> {
        self.event_rx.try_recv().ok()
    }

    pub fn request_shutdown(&mut self) -> bool {
        if self.shutdown_requested.swap(true, Ordering::AcqRel) {
            return self.join.is_none();
        }
        let started = Instant::now();
        let sent = self.command_tx.send(UpscaleCommand::Shutdown).is_ok();
        let had_thread = self.join.take().is_some();
        let stopped = self
            .stopped_rx
            .recv_timeout(Duration::from_millis(300))
            .is_ok();
        perf_trace::record_duration(
            "shutdown_request",
            started.elapsed(),
            &[
                PerfField::Str("component", "ai_upscale"),
                PerfField::Bool("command_sent", sent),
                PerfField::Bool("thread_detached", had_thread && !stopped),
                PerfField::Bool("thread_stopped", stopped),
            ],
        );
        stopped
    }
}

impl Drop for AiUpscaleWorker {
    fn drop(&mut self) {
        self.request_shutdown();
    }
}

#[derive(Clone)]
pub struct UpscaleRequest {
    pub generation: u64,
    pub book_id: String,
    pub source: SharedSource,
    pub page_index: usize,
    pub page_name: Option<String>,
    pub target_long_edge: u32,
    pub decode: DecodeOptions,
    pub settings: NcnnRealEsrganSettings,
}

pub enum UpscaleEvent {
    Finished {
        generation: u64,
        book_id: String,
        page_index: usize,
        source_hash: String,
        decode: DecodeOptions,
        page: Arc<PreparedPage>,
    },
    Failed {
        generation: u64,
        book_id: String,
        page_index: usize,
        target_long_edge: u32,
        decode: DecodeOptions,
        message: String,
    },
}

enum UpscaleCommand {
    Run(UpscaleRequest),
    Shutdown,
}

fn run_upscale_worker(
    command_rx: Receiver<UpscaleCommand>,
    event_tx: Sender<UpscaleEvent>,
    ctx: Context,
    shutdown_requested: Arc<AtomicBool>,
) {
    while !shutdown_requested.load(Ordering::Acquire) {
        let Ok(command) = command_rx.recv() else {
            break;
        };
        if shutdown_requested.load(Ordering::Acquire) {
            break;
        }
        match command {
            UpscaleCommand::Run(request) => {
                let Some(request) = latest_upscale_request(&command_rx, request) else {
                    break;
                };
                let generation = request.generation;
                let book_id = request.book_id.clone();
                let page_index = request.page_index;
                let target_long_edge = request.target_long_edge;
                let decode = request.decode;
                let result = run_realesrgan_request(request, &shutdown_requested);
                if shutdown_requested.load(Ordering::Acquire) {
                    break;
                }
                let event = match result {
                    Ok(Some((source_hash, page))) => UpscaleEvent::Finished {
                        generation,
                        book_id,
                        page_index,
                        source_hash,
                        decode,
                        page: Arc::new(page),
                    },
                    Err(message) => UpscaleEvent::Failed {
                        generation,
                        book_id,
                        page_index,
                        target_long_edge,
                        decode,
                        message,
                    },
                    Ok(None) => break,
                };
                let _ = event_tx.send(event);
                ctx.request_repaint();
            }
            UpscaleCommand::Shutdown => break,
        }
    }
}

fn latest_upscale_request(
    command_rx: &Receiver<UpscaleCommand>,
    mut request: UpscaleRequest,
) -> Option<UpscaleRequest> {
    while let Ok(command) = command_rx.try_recv() {
        match command {
            UpscaleCommand::Run(next_request) => request = next_request,
            UpscaleCommand::Shutdown => return None,
        }
    }
    Some(request)
}

fn run_realesrgan_request(
    request: UpscaleRequest,
    shutdown_requested: &AtomicBool,
) -> Result<Option<(String, PreparedPage)>, String> {
    let bytes = request
        .source
        .read_page(request.page_index)
        .map_err(|error| error.to_string())?;
    if shutdown_requested.load(Ordering::Acquire) {
        return Ok(None);
    }
    let source_hash = blake3::hash(&bytes).to_hex().to_string();
    let cache_entry = AiUpscaleCacheEntry::new(&source_hash, &request.settings);
    if let Some(output_bytes) = read_cached_output(&cache_entry)? {
        match prepare_image_with_options(&output_bytes, request.target_long_edge, request.decode) {
            Ok(page) => {
                perf_trace::record_duration(
                    "ai_upscale_cache_hit",
                    Duration::ZERO,
                    &[
                        PerfField::Usize("page", request.page_index),
                        PerfField::Str("format", cache_entry.extension_label()),
                    ],
                );
                return Ok(Some((source_hash, page)));
            }
            Err(_) => {
                let _ = fs::remove_file(&cache_entry.path);
                perf_trace::record_duration(
                    "ai_upscale_cache_invalid",
                    Duration::ZERO,
                    &[PerfField::Usize("page", request.page_index)],
                );
            }
        }
    }

    let work_dir = create_temp_work_dir(request.generation, request.page_index)?;
    let input_path = work_dir.join(input_file_name(request.page_name.as_deref()));
    let output_path = work_dir.join(format!(
        "output.{}",
        normalized_output_format(&request.settings.output_format)
    ));
    let stdout_path = work_dir.join("realesrgan.stdout.txt");
    let stderr_path = work_dir.join("realesrgan.stderr.txt");

    let result = (|| {
        write_ncnn_input(&bytes, request.page_name.as_deref(), &input_path)?;
        if shutdown_requested.load(Ordering::Acquire) {
            return Ok(None);
        }
        let command = build_ncnn_realesrgan_command(&request.settings, &input_path, &output_path)?;
        let process_started = Instant::now();
        let Some(status) = run_ncnn_command_interruptibly(
            &command,
            shutdown_requested,
            &stdout_path,
            &stderr_path,
        )?
        else {
            return Ok(None);
        };
        perf_trace::record_duration_if_at_least(
            "ai_upscale_process",
            process_started.elapsed(),
            Duration::from_millis(250),
            &[
                PerfField::Bool("success", status.success()),
                PerfField::Usize("page", request.page_index),
            ],
        );
        if !status.success() {
            let detail = process_output_detail(&stderr_path, &stdout_path)
                .unwrap_or_else(|| format!("exit code {:?}", status.code()));
            return Err(format!("Real-ESRGAN 처리 실패: {detail}"));
        }
        let output_bytes = fs::read(&output_path)
            .map_err(|error| format!("업스케일 결과를 읽을 수 없습니다: {error}"))?;
        if store_cached_output(&cache_entry, &output_bytes).is_err() {
            perf_trace::record_duration(
                "ai_upscale_cache_store_failed",
                Duration::ZERO,
                &[
                    PerfField::Usize("page", request.page_index),
                    PerfField::Str("component", "cache_store"),
                ],
            );
        }
        prepare_image_with_options(&output_bytes, request.target_long_edge, request.decode)
            .map(|page| Some((source_hash, page)))
    })();

    let _ = fs::remove_dir_all(&work_dir);
    result
}

fn run_ncnn_command_interruptibly(
    command: &NcnnRealEsrganCommand,
    shutdown_requested: &AtomicBool,
    stdout_path: &Path,
    stderr_path: &Path,
) -> Result<Option<ExitStatus>, String> {
    let mut command = command.to_command();
    let program = PathBuf::from(command.get_program());
    if should_check_executable_path(&program) && !program.is_file() {
        return Err(format!(
            "Real-ESRGAN 실행 파일을 찾을 수 없습니다: {}",
            program.display()
        ));
    }
    let stdout = File::create(stdout_path)
        .map_err(|error| format!("Real-ESRGAN stdout 준비 실패: {error}"))?;
    let stderr = File::create(stderr_path)
        .map_err(|error| format!("Real-ESRGAN stderr 준비 실패: {error}"))?;
    let mut child = command
        .stdout(Stdio::from(stdout))
        .stderr(Stdio::from(stderr))
        .spawn()
        .map_err(|error| format!("Real-ESRGAN 실행 실패: {error}"))?;

    loop {
        if shutdown_requested.load(Ordering::Acquire) {
            let _ = child.kill();
            let _ = child.wait();
            return Ok(None);
        }

        if let Some(status) = child
            .try_wait()
            .map_err(|error| format!("Real-ESRGAN 상태 확인 실패: {error}"))?
        {
            return Ok(Some(status));
        }

        thread::sleep(Duration::from_millis(50));
    }
}

fn process_output_detail(stderr_path: &Path, stdout_path: &Path) -> Option<String> {
    [stderr_path, stdout_path].into_iter().find_map(|path| {
        let text = fs::read_to_string(path).ok()?;
        let trimmed = text.trim();
        (!trimmed.is_empty()).then(|| trimmed.to_owned())
    })
}

fn create_temp_work_dir(generation: u64, page_index: usize) -> Result<PathBuf, String> {
    let stamp = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map_err(|error| error.to_string())?
        .as_nanos();
    let dir = std::env::temp_dir().join(format!(
        "suisuiview-upscale-{}-{generation}-{page_index}-{stamp}",
        std::process::id()
    ));
    fs::create_dir_all(&dir)
        .map_err(|error| format!("임시 업스케일 폴더를 만들 수 없습니다: {error}"))?;
    Ok(dir)
}

fn input_file_name(page_name: Option<&str>) -> String {
    match page_name.and_then(input_extension_for_page_name) {
        Some(extension) => format!("input.{extension}"),
        None => "input.png".to_owned(),
    }
}

fn write_ncnn_input(
    bytes: &[u8],
    page_name: Option<&str>,
    input_path: &Path,
) -> Result<(), String> {
    if page_name
        .and_then(input_extension_for_page_name)
        .is_some_and(|extension| matches!(extension, "jpg" | "jpeg" | "png" | "webp"))
    {
        return fs::write(input_path, bytes)
            .map_err(|error| format!("업스케일 입력 파일을 쓸 수 없습니다: {error}"));
    }

    let image = image::load_from_memory(bytes)
        .map_err(|error| format!("AI 업스케일 입력 이미지를 디코딩할 수 없습니다: {error}"))?;
    DynamicImage::ImageRgba8(image.into_rgba8())
        .save_with_format(input_path, ImageFormat::Png)
        .map_err(|error| format!("AI 업스케일 PNG 입력을 만들 수 없습니다: {error}"))
}

fn input_extension_for_page_name(page_name: &str) -> Option<&'static str> {
    match Path::new(page_name)
        .extension()
        .and_then(|extension| extension.to_str())
        .map(|extension| extension.to_ascii_lowercase())
        .as_deref()
    {
        Some("jpg") | Some("jpeg") => Some("jpg"),
        Some("png") => Some("png"),
        Some("webp") => Some("webp"),
        _ => None,
    }
}

pub fn normalized_tile_size(tile_size: u32) -> u32 {
    if tile_size == 0 {
        0
    } else {
        tile_size.clamp(32, 2048)
    }
}

pub fn normalized_output_format(output_format: &str) -> String {
    match output_format.trim().to_ascii_lowercase().as_str() {
        "jpg" | "jpeg" => "jpg".to_owned(),
        "webp" => "webp".to_owned(),
        _ => "png".to_owned(),
    }
}

fn should_check_executable_path(program: &Path) -> bool {
    program.is_absolute() || program.components().count() > 1
}

fn path_arg(path: &Path) -> String {
    path.as_os_str().to_string_lossy().into_owned()
}

#[cfg(test)]
mod tests;

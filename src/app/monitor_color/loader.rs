use super::gpu::OutputLut;
use super::MonitorColorStatus;
use crate::core::monitor_color::build_display_lut;
use std::collections::{hash_map::DefaultHasher, VecDeque};
use std::hash::{Hash, Hasher};
use std::io::Read;
use std::sync::{mpsc, Arc, Mutex};
use std::time::Duration;

const RECHECK_INTERVAL: Duration = Duration::from_secs(5);
const MAX_PROFILE_BYTES: u64 = 16 * 1024 * 1024;

#[derive(Clone)]
pub(super) struct LoadedProfile {
    pub display: String,
    pub fingerprint: u64,
    pub lut: Option<Arc<OutputLut>>,
    pub renderer: Option<Arc<Mutex<super::scene::SceneRenderer>>>,
    pub status: MonitorColorStatus,
}

pub(super) struct ProfileLoader {
    pub requests: mpsc::Sender<String>,
    pub results: mpsc::Receiver<LoadedProfile>,
}

impl ProfileLoader {
    pub fn start(ctx: egui::Context, state: egui_wgpu::RenderState) -> Result<Self, String> {
        let (requests, receiver) = mpsc::channel();
        let (sender, results) = mpsc::channel();
        std::thread::Builder::new()
            .name("suisuiview-display-profile".to_owned())
            .spawn(move || run(receiver, sender, ctx, state))
            .map_err(|error| error.to_string())?;
        Ok(Self { requests, results })
    }
}

fn run(
    requests: mpsc::Receiver<String>,
    results: mpsc::Sender<LoadedProfile>,
    ctx: egui::Context,
    state: egui_wgpu::RenderState,
) {
    let Ok(mut display) = requests.recv() else {
        return;
    };
    let mut cache: VecDeque<(u64, Arc<OutputLut>)> = VecDeque::new();
    let mut renderer = None;
    let mut last: Option<(String, u64, MonitorColorStatus)> = None;
    loop {
        while let Ok(next) = requests.try_recv() {
            display = next;
        }
        let mut loaded = load(&display, &mut cache, &state);
        if loaded.lut.is_some() {
            // Compile the optional egui FP16 pipeline here, never on the UI
            // thread. Every profile shares it and the borrowed texture bindings.
            loaded.renderer = Some(
                renderer
                    .get_or_insert_with(|| {
                        Arc::new(Mutex::new(super::scene::SceneRenderer::new(&state.device)))
                    })
                    .clone(),
            );
        }
        let identity = (display.clone(), loaded.fingerprint, loaded.status.clone());
        if last.as_ref() != Some(&identity) {
            if results.send(loaded).is_err() {
                return;
            }
            last = Some(identity);
            ctx.request_repaint();
        }
        match requests.recv_timeout(RECHECK_INTERVAL) {
            Ok(next) => display = next,
            Err(mpsc::RecvTimeoutError::Timeout) => {}
            Err(mpsc::RecvTimeoutError::Disconnected) => return,
        }
    }
}

fn load(
    display: &str,
    cache: &mut VecDeque<(u64, Arc<OutputLut>)>,
    state: &egui_wgpu::RenderState,
) -> LoadedProfile {
    match load_inner(display, cache, state) {
        Ok((fingerprint, lut, status)) => LoadedProfile {
            display: display.to_owned(),
            fingerprint,
            lut,
            renderer: None,
            status,
        },
        Err(reason) => LoadedProfile {
            display: display.to_owned(),
            fingerprint: 0,
            lut: None,
            renderer: None,
            status: MonitorColorStatus::Fallback { reason },
        },
    }
}

fn load_inner(
    display: &str,
    cache: &mut VecDeque<(u64, Arc<OutputLut>)>,
    state: &egui_wgpu::RenderState,
) -> Result<(u64, Option<Arc<OutputLut>>, MonitorColorStatus), String> {
    #[cfg(target_os = "windows")]
    let path = super::windows::default_profile(display)?;
    #[cfg(not(target_os = "windows"))]
    let path: Option<std::path::PathBuf> = {
        let _ = display;
        None
    };
    let Some(path) = path else {
        return Ok((0, None, MonitorColorStatus::SystemSrgb));
    };
    let metadata = std::fs::metadata(&path)
        .map_err(|error| format!("Cannot open display profile: {error}"))?;
    if metadata.len() > MAX_PROFILE_BYTES {
        return Err("Display profile exceeds the 16 MiB limit".to_owned());
    }
    // File reads, hashing and LittleCMS transforms all stay on this worker.
    let mut bytes = Vec::with_capacity(metadata.len() as usize);
    std::fs::File::open(&path)
        .and_then(|file| file.take(MAX_PROFILE_BYTES + 1).read_to_end(&mut bytes))
        .map_err(|error| format!("Cannot read display profile: {error}"))?;
    if bytes.len() as u64 > MAX_PROFILE_BYTES {
        return Err("Display profile grew beyond the limit".to_owned());
    }
    let mut hasher = DefaultHasher::new();
    bytes.hash(&mut hasher);
    let fingerprint = hasher.finish();
    let lut = if let Some(index) = cache.iter().position(|(key, _)| *key == fingerprint) {
        let entry = cache.remove(index).unwrap();
        let lut = entry.1.clone();
        cache.push_front(entry);
        lut
    } else {
        let values = build_display_lut(&bytes)?;
        let lut = Arc::new(OutputLut::new(
            &state.device,
            &state.queue,
            state.target_format,
            &values,
        ));
        cache.push_front((fingerprint, lut.clone()));
        cache.truncate(2);
        lut
    };
    Ok((
        fingerprint,
        Some(lut),
        MonitorColorStatus::Managed {
            profile: path
                .file_name()
                .unwrap_or(path.as_os_str())
                .to_string_lossy()
                .into_owned(),
        },
    ))
}

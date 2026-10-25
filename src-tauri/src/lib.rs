use chrono::Local;
use cpal::traits::{DeviceTrait, HostTrait, StreamTrait};
use crossbeam_channel::unbounded;
use fs2::available_space;
use hound::{WavSpec, WavWriter};
use serde::Serialize;
use std::fs;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex};
use std::time::Duration;
use tauri::menu::{Menu, MenuItem};
use tauri::tray::{MouseButton, TrayIconBuilder, TrayIconEvent};
use tauri::{Emitter, Manager, State, WebviewUrl, WebviewWindowBuilder};
use tauri_plugin_global_shortcut::{GlobalShortcutExt, ShortcutState};
use tauri_plugin_notification::NotificationExt;
use tauri_plugin_opener::OpenerExt;
use tauri_plugin_shell::process::CommandChild;
use tauri_plugin_shell::ShellExt;

// ==================== TYPES ====================

#[derive(Serialize, Clone)]
struct RecordingFile {
    name: String,
    path: String,
    size: u64,
    modified: u64,
}

#[derive(Serialize, Clone)]
struct OrphanRecord {
    temp_video: String,
    temp_sys: Option<String>,
    temp_mic: Option<String>,
    suggested_final: String,
}

#[derive(Serialize, Clone)]
struct EncoderCapabilities {
    cpu: bool,
    nvenc: bool,
    amf: bool,
    draw_mouse_supported: bool,
}

struct ActiveCapture {
    video_child: Option<CommandChild>,
    mic_stop_flag: Option<Arc<AtomicBool>>,
    sys_stop_flag: Option<Arc<AtomicBool>>,
    _mic_stream: Option<cpal::Stream>,
    _sys_stream: Option<cpal::Stream>,
    temp_video_path: String,
    temp_sys_path: Option<String>,
    temp_mic_path: Option<String>,
    final_output_path: String,
    is_gif: bool,
}

#[derive(Clone)]
struct ReplayBufferState {
    buffer_dir: String,
    #[allow(dead_code)]
    duration_secs: u32,
    #[allow(dead_code)]
    child: Arc<Mutex<Option<CommandChild>>>,
}

struct RecorderState(Mutex<Option<ActiveCapture>>);
struct ReplayState(Mutex<Option<ReplayBufferState>>);

// ==================== LOGGING ====================

fn init_logging() {
    use simplelog::*;
    let dir = log_dir();
    let _ = fs::create_dir_all(&dir);
    let log_file = dir.join(format!("cosrecorder-{}.log", Local::now().format("%Y-%m-%d")));

    let mut loggers: Vec<Box<dyn SharedLogger>> = Vec::new();
    if let Ok(file) = std::fs::File::create(&log_file) {
        loggers.push(WriteLogger::new(LevelFilter::Info, Config::default(), file));
    }
    #[cfg(debug_assertions)]
    loggers.push(TermLogger::new(
        LevelFilter::Debug,
        Config::default(),
        TerminalMode::Mixed,
        ColorChoice::Auto,
    ));
    let _ = CombinedLogger::init(loggers);
    log::info!("COSRecorder starting");
}

fn log_dir() -> PathBuf {
    let base = std::env::var("APPDATA")
        .map(PathBuf::from)
        .unwrap_or_else(|_| std::env::temp_dir());
    base.join("COSRecorder").join("logs")
}

#[tauri::command]
fn read_recent_log(lines: usize) -> Result<String, String> {
    let dir = log_dir();
    let mut files: Vec<PathBuf> = fs::read_dir(&dir)
        .map_err(|e| e.to_string())?
        .flatten()
        .filter_map(|e| {
            let p = e.path();
            if p.extension().and_then(|s| s.to_str()) == Some("log") {
                Some(p)
            } else {
                None
            }
        })
        .collect();
    files.sort_by(|a, b| b.cmp(a));
    let Some(latest) = files.into_iter().next() else {
        return Ok("(no log file yet)".into());
    };
    let content = fs::read_to_string(&latest).map_err(|e| e.to_string())?;
    let tail: Vec<&str> = content.lines().rev().take(lines).collect();
    let mut out: Vec<&str> = tail.into_iter().collect();
    out.reverse();
    Ok(out.join("\n"))
}

// ==================== AUTOSTART ====================

#[tauri::command]
fn set_autostart(enable: bool) -> Result<(), String> {
    #[cfg(target_os = "windows")]
    {
        use winreg::enums::*;
        use winreg::RegKey;
        let hkcu = RegKey::predef(HKEY_CURRENT_USER);
        let path = r"Software\Microsoft\Windows\CurrentVersion\Run";
        if let Ok(key) = hkcu.open_subkey_with_flags(path, KEY_SET_VALUE) {
            if enable {
                if let Ok(exe_path) = std::env::current_exe() {
                    let _ = key.set_value("COSRecorder", &exe_path.to_string_lossy().as_ref());
                }
            } else {
                let _ = key.delete_value("COSRecorder");
            }
        }
    }
    Ok(())
}

// ==================== FILENAME ====================

fn render_filename(template: &str, is_gif: bool) -> String {
    let now = Local::now();
    let ext = if is_gif { "gif" } else { "mp4" };
    let tpl = if template.trim().is_empty() {
        "COSRecorder_%datetime%"
    } else {
        template
    };
    let name = tpl
        .replace("%datetime%", &now.format("%Y-%m-%d_%H-%M-%S").to_string())
        .replace("%date%", &now.format("%Y-%m-%d").to_string())
        .replace("%time%", &now.format("%H-%M-%S").to_string());
    let sanitized: String = name
        .chars()
        .map(|c| match c {
            '<' | '>' | ':' | '"' | '/' | '\\' | '|' | '?' | '*' => '_',
            c => c,
        })
        .collect();
    format!("{}.{}", sanitized, ext)
}

fn map_preset(codec: &str, preset: &str) -> &'static str {
    match codec {
        "h264_nvenc" => match preset {
            "ultrafast" => "p1",
            "superfast" => "p2",
            "veryfast" => "p3",
            "faster" => "p4",
            "fast" => "p5",
            "medium" => "p6",
            "slow" => "p7",
            _ => "p1",
        },
        "h264_amf" => "speed",
        _ => match preset {
            "ultrafast" => "ultrafast",
            "superfast" => "superfast",
            "veryfast" => "veryfast",
            _ => "ultrafast",
        },
    }
}

// ==================== ENCODER PROBING ====================

#[tauri::command]
async fn probe_encoders(app: tauri::AppHandle) -> Result<EncoderCapabilities, String> {
    let check = |enc: &'static str| {
        let app = app.clone();
        async move {
            let Ok(cmd) = app.shell().sidecar("ffmpeg") else { return false; };
            let result = cmd
                .args(["-hide_banner", "-h", &format!("encoder={}", enc)])
                .output()
                .await;
            matches!(result, Ok(out) if out.status.success())
        }
    };
    let nvenc = check("h264_nvenc").await;
    let amf = check("h264_amf").await;
    Ok(EncoderCapabilities {
        cpu: true,
        nvenc,
        amf,
        draw_mouse_supported: true,
    })
}

// ==================== AUDIO ====================

#[tauri::command]
fn get_output_devices() -> Result<Vec<String>, String> {
    let host = cpal::default_host();
    let mut devices = Vec::new();
    if let Ok(outputs) = host.output_devices() {
        for dev in outputs {
            let name = dev.to_string();
            if !devices.contains(&name) {
                devices.push(name);
            }
        }
    }
    Ok(devices)
}

#[tauri::command]
fn get_input_devices() -> Result<Vec<String>, String> {
    let host = cpal::default_host();
    let mut devices = Vec::new();
    if let Ok(inputs) = host.input_devices() {
        for dev in inputs {
            let name = dev.to_string();
            if !devices.contains(&name) {
                devices.push(name);
            }
        }
    }
    Ok(devices)
}

fn spawn_audio_capture(
    device_name: &str,
    path: String,
    is_system: bool,
    start_flag: Arc<AtomicBool>,
) -> Result<(cpal::Stream, Arc<AtomicBool>), String> {
    let host = cpal::default_host();

    let device = if is_system {
        host.output_devices()
            .map_err(|e| e.to_string())?
            .find(|x| x.to_string() == device_name)
            .ok_or_else(|| format!("System audio device '{}' not found", device_name))?
    } else {
        host.input_devices()
            .map_err(|e| e.to_string())?
            .find(|x| x.to_string() == device_name)
            .ok_or_else(|| format!("Mic device '{}' not found", device_name))?
    };

    let config = if is_system {
        device.default_output_config().map_err(|e| e.to_string())?
    } else {
        device.default_input_config().map_err(|e| e.to_string())?
    };

    let sample_format = config.sample_format();
    let stream_config: cpal::StreamConfig = config.into();

    let spec = WavSpec {
        channels: stream_config.channels as u16,
        sample_rate: stream_config.sample_rate,
        bits_per_sample: 32,
        sample_format: hound::SampleFormat::Float,
    };

    let (tx, rx) = unbounded::<f32>();
    let is_recording = Arc::new(AtomicBool::new(true));
    let is_recording_clone = is_recording.clone();

    std::thread::spawn(move || {
        let spec_channels = spec.channels as f64;
        let spec_sample_rate = spec.sample_rate as f64;
        let mut writer = match WavWriter::create(&path, spec) {
            Ok(w) => w,
            Err(e) => {
                log::error!("Failed to create wav file {}: {}", path, e);
                return;
            }
        };
        
        let mut started = false;
        let mut start_time = std::time::Instant::now();
        let mut samples_written: u64 = 0;
        
        while is_recording_clone.load(Ordering::Relaxed) || !rx.is_empty() {
            if !started {
                if start_flag.load(Ordering::Relaxed) {
                    started = true;
                    while rx.try_recv().is_ok() {}
                    start_time = std::time::Instant::now();
                } else {
                    while rx.try_recv().is_ok() {}
                    std::thread::sleep(Duration::from_millis(5));
                    continue;
                }
            }

            while let Ok(sample) = rx.try_recv() {
                let _ = writer.write_sample(sample);
                samples_written += 1;
            }

            let elapsed_seconds = start_time.elapsed().as_secs_f64();
            let expected_samples = (elapsed_seconds * spec_sample_rate * spec_channels) as u64;
            let tolerance = (0.1 * spec_sample_rate * spec_channels) as u64; 
            
            if expected_samples > samples_written + tolerance {
                let missing = expected_samples - samples_written;
                for _ in 0..missing {
                    let _ = writer.write_sample(0.0f32);
                }
                samples_written += missing;
            }

            std::thread::sleep(Duration::from_millis(5));
        }
        if let Err(e) = writer.finalize() {
            log::error!("Error finalizing WAV writer: {}", e);
        }
    });

    let err_fn = |err| log::error!("Audio stream error: {}", err);

    let stream = match sample_format {
        cpal::SampleFormat::F32 => device.build_input_stream(
            stream_config,
            move |data: &[f32], _: &cpal::InputCallbackInfo| {
                for &s in data { let _ = tx.send(s); }
            },
            err_fn,
            None,
        ),
        cpal::SampleFormat::I16 => device.build_input_stream(
            stream_config,
            move |data: &[i16], _: &cpal::InputCallbackInfo| {
                for &s in data { let _ = tx.send(s as f32 / i16::MAX as f32); }
            },
            err_fn,
            None,
        ),
        cpal::SampleFormat::U16 => device.build_input_stream(
            stream_config,
            move |data: &[u16], _: &cpal::InputCallbackInfo| {
                for &s in data {
                    let _ = tx.send((s as f32 - u16::MAX as f32 / 2.0) / (u16::MAX as f32 / 2.0));
                }
            },
            err_fn,
            None,
        ),
        _ => return Err("Unsupported audio format".to_string()),
    }.map_err(|e| e.to_string())?;

    stream.play().map_err(|e| e.to_string())?;
    Ok((stream, is_recording))
}

// ==================== REPLAY BUFFER ====================

#[tauri::command]
async fn start_replay_buffer(
    app: tauri::AppHandle,
    encoder: String,
    duration_secs: u32,
    state: State<'_, ReplayState>,
) -> Result<(), String> {
    let mut lock = state.0.lock().unwrap();
    if lock.is_some() {
        return Err("Replay buffer zaten aktif.".into());
    }

    let buf_dir = std::env::temp_dir().join("COSRecorder_Replay");
    let _ = fs::remove_dir_all(&buf_dir);
    fs::create_dir_all(&buf_dir).map_err(|e| e.to_string())?;

    let selected_codec = match encoder.as_str() {
        "nvenc" => "h264_nvenc",
        "amf" => "h264_amf",
        _ => "libx264",
    };

    let output_pattern = buf_dir.join("segment_%03d.ts").to_string_lossy().replace('\\', "/");

    let mut args = vec![
        "-y".to_string(),
        "-f".to_string(), "gdigrab".to_string(),
        "-framerate".to_string(), "60".to_string(),
        "-i".to_string(), "desktop".to_string(),
        "-c:v".to_string(), selected_codec.to_string(),
    ];

    if selected_codec == "h264_nvenc" {
        args.extend(vec![
            "-preset".to_string(), "p1".to_string(),
            "-rc".to_string(), "constqp".to_string(),
            "-qp".to_string(), "23".to_string(),
            "-pix_fmt".to_string(), "yuv420p".to_string(),
        ]);
    } else {
        args.extend(vec![
            "-preset".to_string(), "ultrafast".to_string(),
            "-crf".to_string(), "22".to_string(),
            "-pix_fmt".to_string(), "yuv420p".to_string(),
        ]);
    }

    let segment_count = (duration_secs / 3) + 2;

    args.extend(vec![
        "-f".to_string(), "segment".to_string(),
        "-segment_time".to_string(), "3".to_string(),
        "-segment_wrap".to_string(), format!("{}", segment_count),
        "-reset_timestamps".to_string(), "1".to_string(),
        output_pattern,
    ]);

    log::info!("Starting Replay Buffer FFmpeg: {:?}", args);

    let (_, child) = app
        .shell()
        .sidecar("ffmpeg")
        .map_err(|e| format!("Sidecar izni hatası: {}", e))?
        .args(&args)
        .spawn()
        .map_err(|e| format!("FFmpeg başlatılamadı: {}", e))?;

    *lock = Some(ReplayBufferState {
        buffer_dir: buf_dir.to_string_lossy().to_string(),
        duration_secs,
        child: Arc::new(Mutex::new(Some(child))),
    });

    log::info!("Replay Buffer Started ({} seconds)", duration_secs);
    Ok(())
}

#[tauri::command]
async fn save_replay_clip(
    app: tauri::AppHandle,
    output_dir: String,
    state: State<'_, ReplayState>,
) -> Result<String, String> {
    let replay = {
        let lock = state.0.lock().unwrap();
        let Some(ref r) = *lock else {
            return Err("Replay Buffer çalışmıyor. Lütfen önce başlatın.".into());
        };
        r.clone()
    };

    let target_dir = Path::new(&output_dir);
    if !target_dir.exists() {
        let _ = fs::create_dir_all(target_dir).map_err(|e| e.to_string())?;
    }

    let buf_dir = Path::new(&replay.buffer_dir);
    
    let mut ts_files: Vec<PathBuf> = match fs::read_dir(buf_dir) {
        Ok(entries) => entries
            .flatten()
            .filter_map(|e| {
                let p = e.path();
                if p.extension().and_then(|s| s.to_str()) == Some("ts") {
                    if fs::metadata(&p).map(|m| m.len() > 100).unwrap_or(false) {
                        Some(p)
                    } else {
                        None
                    }
                } else {
                    None
                }
            })
            .collect(),
        Err(e) => return Err(format!("Buffer dizini okunamadı: {}", e)),
    };

    if ts_files.is_empty() {
        return Err("Henüz yeterli buffer parçası oluşmadı. Lütfen birkaç saniye bekleyin.".into());
    }

    ts_files.sort_by_key(|p| fs::metadata(p).and_then(|m| m.modified()).ok());

    let list_file = buf_dir.join("concat_list.txt");
    let mut concat_content = String::new();
    for f in &ts_files {
        let clean_path = f.to_string_lossy().replace('\\', "/");
        concat_content.push_str(&format!("file '{}'\n", clean_path));
    }
    fs::write(&list_file, concat_content).map_err(|e| e.to_string())?;

    let clip_name = render_filename("COSRecorder_Clip_%datetime%", false);
    let final_path = target_dir.join(&clip_name).to_string_lossy().to_string();

    let mux_args = vec![
        "-y".to_string(),
        "-f".to_string(), "concat".to_string(),
        "-safe".to_string(), "0".to_string(),
        "-i".to_string(), list_file.to_string_lossy().replace('\\', "/"),
        "-c".to_string(), "copy".to_string(),
        "-movflags".to_string(), "+faststart".to_string(),
        final_path.clone(),
    ];

    let out = app
        .shell()
        .sidecar("ffmpeg")
        .map_err(|e| e.to_string())?
        .args(&mux_args)
        .output()
        .await
        .map_err(|e| e.to_string())?;

    if out.status.success() && fs::metadata(&final_path).map(|m| m.len() > 1024).unwrap_or(false) {
        let _ = app.notification().builder().title("COSRecorder Klip").body("Klip kaydedildi!").show();
        let _ = app.opener().reveal_item_in_dir(&final_path);
        Ok(final_path)
    } else {
        Err("Klip oluşturulamadı.".into())
    }
}

// ==================== SCREENSHOT ====================

#[tauri::command]
async fn take_screenshot(
    app: tauri::AppHandle,
    x: i32,
    y: i32,
    w: i32,
    h: i32,
    output_dir: String,
) -> Result<String, String> {
    let even_w = if w % 2 != 0 { w - 1 } else { w };
    let even_h = if h % 2 != 0 { h - 1 } else { h };

    let target_dir_str = if output_dir.trim().is_empty() {
        r"D:\Videolar".to_string()
    } else {
        output_dir
    };
    
    let target_path = Path::new(&target_dir_str);
    if !target_path.exists() {
        let _ = fs::create_dir_all(target_path);
    }

    let filename = format!("COSScreenshot_{}.png", Local::now().format("%Y-%m-%d_%H-%M-%S"));
    let final_path = target_path.join(&filename).to_string_lossy().to_string();

    let args = vec![
        "-y".to_string(),
        "-f".to_string(), "gdigrab".to_string(),
        "-offset_x".to_string(), x.to_string(),
        "-offset_y".to_string(), y.to_string(),
        "-video_size".to_string(), format!("{}x{}", even_w, even_h),
        "-i".to_string(), "desktop".to_string(),
        "-vframes".to_string(), "1".to_string(),
        final_path.clone(),
    ];

    let out = app
        .shell()
        .sidecar("ffmpeg")
        .map_err(|e| e.to_string())?
        .args(&args)
        .output()
        .await
        .map_err(|e| e.to_string())?;

    if out.status.success() && fs::metadata(&final_path).map(|m| m.len() > 100).unwrap_or(false) {
        let _ = app.notification().builder().title("COSRecorder").body("Ekran görüntüsü kaydedildi.").show();
        Ok(final_path)
    } else {
        Err("Ekran görüntüsü alınamadı.".into())
    }
}

// ==================== CAPTURE ====================

#[tauri::command]
fn check_disk_space(dir: String) -> Result<u64, String> {
    let p = Path::new(&dir);
    let probe = if p.exists() { p.to_path_buf() } else { PathBuf::from(".") };
    available_space(&probe).map_err(|e| e.to_string())
}

#[tauri::command]
async fn start_capture(
    app: tauri::AppHandle,
    x: i32,
    y: i32,
    w: i32,
    h: i32,
    preset: String,
    encoder: String,
    export_gif: bool,
    gif_fps: u32,
    output_dir: String,
    record_system_audio: bool,
    record_mic: bool,
    sys_audio_device: String,
    mic_device: String,
    draw_mouse: bool,
    show_timer: bool,
    filename_template: String,
    state: State<'_, RecorderState>,
) -> Result<(), String> {
    {
        let recorder = state.0.lock().unwrap();
        if recorder.is_some() {
            return Err("Kayıt zaten devam ediyor.".into());
        }
    }

    let fps = "60".to_string();

    let target_dir_str = if output_dir.trim().is_empty() {
        r"D:\Videolar".to_string()
    } else {
        output_dir
    };
    let target_path = Path::new(&target_dir_str);
    if !target_path.exists() {
        fs::create_dir_all(target_path).map_err(|e| format!("Dizin oluşturulamadı: {}", e))?;
    }

    if let Ok(free) = available_space(target_path) {
        if free < 200 * 1024 * 1024 {
            return Err(format!("Disk alanı yetersiz: {} MB kaldı", free / 1024 / 1024));
        }
    }

    let final_file_name = render_filename(&filename_template, export_gif);
    let final_output_path = target_path.join(&final_file_name).to_string_lossy().to_string();

    let temp_video_path = target_path.join(format!("temp_vid_{}", final_file_name)).to_string_lossy().to_string();
    let temp_sys_path = target_path.join(format!("temp_sys_{}.wav", final_file_name)).to_string_lossy().to_string();
    let temp_mic_path = target_path.join(format!("temp_mic_{}.wav", final_file_name)).to_string_lossy().to_string();

    let selected_codec = match encoder.as_str() {
        "nvenc" => "h264_nvenc",
        "amf" => "h264_amf",
        _ => "libx264",
    };

    let mut sys_stop_flag = None;
    let mut sys_stream = None;
    let mut actual_temp_sys = None;

    let mut mic_stop_flag = None;
    let mut mic_stream = None;
    let mut actual_temp_mic = None;

    let audio_start_flag = Arc::new(AtomicBool::new(false));

    if !export_gif {
        if record_system_audio && !sys_audio_device.trim().is_empty() {
            if let Ok((stream, flag)) = spawn_audio_capture(&sys_audio_device, temp_sys_path.clone(), true, audio_start_flag.clone()) {
                sys_stream = Some(stream);
                sys_stop_flag = Some(flag);
                actual_temp_sys = Some(temp_sys_path.clone());
            }
        }
        if record_mic && !mic_device.trim().is_empty() {
            if let Ok((stream, flag)) = spawn_audio_capture(&mic_device, temp_mic_path.clone(), false, audio_start_flag.clone()) {
                mic_stream = Some(stream);
                mic_stop_flag = Some(flag);
                actual_temp_mic = Some(temp_mic_path.clone());
            }
        }
    }

    let output_target = if export_gif {
        final_output_path.clone()
    } else {
        temp_video_path.clone()
    };

    let even_w = if w % 2 != 0 { w - 1 } else { w };
    let even_h = if h % 2 != 0 { h - 1 } else { h };

    let valid_gif_fps = if gif_fps == 30 { 30 } else { 15 };
    let current_fps = if export_gif { valid_gif_fps.to_string() } else { fps.clone() };

    let mut ffmpeg_args: Vec<String> = vec![
        "-y".to_string(),
        "-f".to_string(), "gdigrab".to_string(),
        "-framerate".to_string(), current_fps,
        "-offset_x".to_string(), x.to_string(),
        "-offset_y".to_string(), y.to_string(),
        "-video_size".to_string(), format!("{}x{}", even_w, even_h),
        "-draw_mouse".to_string(), if draw_mouse { "1" } else { "0" }.to_string(),
        "-i".to_string(), "desktop".to_string(),
        "-progress".to_string(), "pipe:1".to_string(),
        "-nostats".to_string(),
    ];

    if export_gif {
        let gif_vf = format!(
            "fps={},scale=800:-1:flags=lanczos,split[s0][s1];[s0]palettegen=stats_mode=single[p];[s1][p]paletteuse=new=1:dither=bayer:bayer_scale=5",
            valid_gif_fps
        );

        ffmpeg_args.extend(vec![
            "-vf".into(), gif_vf,
            "-loop".into(), "0".into(),
            output_target.clone(),
        ]);
    } else {
        let effective_preset = map_preset(selected_codec, &preset);

        match selected_codec {
            "h264_nvenc" => {
                ffmpeg_args.extend(vec![
                    "-c:v".into(), "h264_nvenc".into(),
                    "-preset".into(), effective_preset.to_string(),
                    "-rc".into(), "constqp".into(),
                    "-qp".into(), "23".into(),
                    "-pix_fmt".into(), "yuv420p".into(),
                    output_target.clone(),
                ]);
            }
            "h264_amf" => {
                ffmpeg_args.extend(vec![
                    "-c:v".into(), "h264_amf".into(),
                    "-quality".into(), effective_preset.to_string(),
                    "-rc".into(), "cqp".into(),
                    "-qp_p".into(), "23".into(),
                    "-qp_i".into(), "23".into(),
                    "-pix_fmt".into(), "yuv420p".into(),
                    output_target.clone(),
                ]);
            }
            _ => {
                ffmpeg_args.extend(vec![
                    "-c:v".into(), "libx264".into(),
                    "-preset".into(), effective_preset.to_string(),
                    "-tune".into(), "zerolatency".into(),
                    "-crf".into(), "22".into(),
                    "-pix_fmt".into(), "yuv420p".into(),
                    "-threads".into(), "0".into(),
                    output_target.clone(),
                ]);
            }
        }
    }

    let (mut rx, child) = app
        .shell()
        .sidecar("ffmpeg")
        .map_err(|e| format!("Sidecar izni hatası: {}", e))?
        .args(&ffmpeg_args)
        .spawn()
        .map_err(|e| format!("FFmpeg başlatılamadı: {}", e))?;

    let app_progress = app.clone();
    let fallback_flag = audio_start_flag.clone();
    
    tauri::async_runtime::spawn(async move {
        tokio::time::sleep(tokio::time::Duration::from_millis(1500)).await;
        fallback_flag.store(true, Ordering::Relaxed);
    });

    tauri::async_runtime::spawn(async move {
        use tauri_plugin_shell::process::CommandEvent;
        let mut stdout_buf = String::new();
        let mut stderr_buf = String::new();
        let mut audio_started = false;

        while let Some(ev) = rx.recv().await {
            match ev {
                CommandEvent::Stdout(bytes) => {
                    let text = String::from_utf8_lossy(&bytes);
                    stdout_buf.push_str(&text);
                    
                    if !audio_started && (text.contains("out_time_ms=") || text.contains("frame=")) {
                        audio_start_flag.store(true, Ordering::Relaxed);
                        audio_started = true;
                    }

                    for line in stdout_buf.lines() {
                        if let Some((k, v)) = line.split_once('=') {
                            if k == "frame" || k == "fps" || k == "total_size" || k == "out_time_ms" {
                                let _ = app_progress.emit(
                                    "recording-progress",
                                    serde_json::json!({ "key": k, "value": v }),
                                );
                            }
                        }
                    }
                    if stdout_buf.len() > 4096 { stdout_buf.clear(); }
                }
                CommandEvent::Stderr(bytes) => {
                    let text = String::from_utf8_lossy(&bytes);
                    stderr_buf.push_str(&text);
                    
                    if !audio_started && text.contains("Press [q] to stop") {
                        audio_start_flag.store(true, Ordering::Relaxed);
                        audio_started = true;
                    }

                    for line in stderr_buf.lines() {
                        let trimmed = line.trim();
                        if trimmed.contains("Error:") || trimmed.starts_with("Error") {
                            log::error!("ffmpeg: {trimmed}");
                        }
                    }
                    if stderr_buf.len() > 8192 { stderr_buf.clear(); }
                }
                _ => {}
            }
        }
    });

    {
        let mut recorder = state.0.lock().unwrap();
        *recorder = Some(ActiveCapture {
            video_child: Some(child),
            mic_stop_flag,
            sys_stop_flag,
            _mic_stream: mic_stream,
            _sys_stream: sys_stream,
            temp_video_path,
            temp_sys_path: actual_temp_sys,
            temp_mic_path: actual_temp_mic,
            final_output_path,
            is_gif: export_gif,
        });
    }

    if let Some(w) = app.get_webview_window("main") {
        let _ = w.hide();
    }
    
    if show_timer {
        if let Some(w) = app.get_webview_window("timer") {
            let _ = w.show();
            let _ = w.set_focus();
        }
    }
    
    let _ = app.emit("recording-started", ());

    Ok(())
}

#[tauri::command]
fn stop_capture(app: tauri::AppHandle, state: State<'_, RecorderState>) -> Result<(), String> {
    let capture = {
        let mut lock = state.0.lock().unwrap();
        lock.take()
    };
    let Some(mut capture) = capture else { return Ok(()); };

    if let Some(sys_flag) = capture.sys_stop_flag { sys_flag.store(false, Ordering::Relaxed); }
    if let Some(mic_flag) = capture.mic_stop_flag { mic_flag.store(false, Ordering::Relaxed); }
    if let Some(mut child) = capture.video_child.take() {
        let _ = child.write(b"q");
    }

    if let Some(w) = app.get_webview_window("timer") { let _ = w.hide(); }
    if let Some(w) = app.get_webview_window("main") {
        let _ = w.set_ignore_cursor_events(false);
        let _ = w.hide();
    }

    let app_clone = app.clone();

    tauri::async_runtime::spawn(async move {
        tokio::time::sleep(Duration::from_millis(1500)).await;

        if capture.is_gif {
            let gif_ok = fs::metadata(&capture.final_output_path)
                .map(|m| m.len() > 100)
                .unwrap_or(false);

            if gif_ok {
                let _ = app_clone.emit("recording-stopped", serde_json::json!({ "path": capture.final_output_path }));
                let _ = app_clone.notification().builder().title("COSRecorder").body("GIF kaydedildi.").show();
                let _ = app_clone.opener().reveal_item_in_dir(&capture.final_output_path);
            } else {
                let msg = "GIF dosyası oluşturulamadı.".to_string();
                log::error!("{msg}");
                let _ = app_clone.emit("recording-error", msg);
                let _ = app_clone.emit("recording-stopped", serde_json::json!({ "path": "" }));
            }
            return;
        }

        let mut mux_args = vec!["-y".to_string(), "-i".to_string(), capture.temp_video_path.clone()];
        let mut audio_count = 0;

        if let Some(sys) = &capture.temp_sys_path {
            mux_args.extend(vec!["-i".to_string(), sys.clone()]);
            audio_count += 1;
        }
        if let Some(mic) = &capture.temp_mic_path {
            mux_args.extend(vec!["-i".to_string(), mic.clone()]);
            audio_count += 1;
        }

        if audio_count == 2 {
            mux_args.extend(vec![
                "-filter_complex".to_string(),
                "[1:a][2:a]amix=inputs=2:duration=longest[aout]".to_string(),
                "-map".to_string(), "0:v".to_string(),
                "-map".to_string(), "[aout]".to_string(),
            ]);
        } else if audio_count == 1 {
            mux_args.extend(vec![
                "-map".to_string(), "0:v".to_string(),
                "-map".to_string(), "1:a".to_string(),
            ]);
        } else {
            mux_args.extend(vec!["-map".to_string(), "0:v".to_string()]);
        }

        mux_args.extend(vec![
            "-c:v".to_string(), "copy".to_string(),
            "-c:a".to_string(), "aac".to_string(),
            "-b:a".to_string(), "128k".to_string(),
            "-movflags".to_string(), "+faststart".to_string(),
            capture.final_output_path.clone(),
        ]);

        let sidecar_res = app_clone.shell().sidecar("ffmpeg");
        let out = match sidecar_res {
            Ok(cmd) => cmd.args(&mux_args).output().await,
            Err(e) => {
                log::error!("Sidecar error during muxing: {}", e);
                let _ = app_clone.emit("recording-error", format!("Muxing başlatılamadı: {}", e));
                return;
            }
        };

        let _ = fs::remove_file(&capture.temp_video_path);
        if let Some(sys) = capture.temp_sys_path { let _ = fs::remove_file(sys); }
        if let Some(mic) = capture.temp_mic_path { let _ = fs::remove_file(mic); }

        let final_ok = matches!(out, Ok(ref o) if o.status.success())
            && fs::metadata(&capture.final_output_path)
                .map(|m| m.len() > 10 * 1024)
                .unwrap_or(false);

        if final_ok {
            let _ = app_clone.emit("recording-stopped", serde_json::json!({ "path": capture.final_output_path }));
            let _ = app_clone.notification().builder().title("COSRecorder").body("Kayıt tamamlandı.").show();
            let _ = app_clone.opener().reveal_item_in_dir(&capture.final_output_path);
            log::info!("Recording finished: {}", capture.final_output_path);
        } else {
            let _ = fs::remove_file(&capture.final_output_path);
            let msg = "Kayıt oluşturulamadı. Lütfen log dosyasını kontrol edin.".to_string();
            log::error!("{msg}");
            let _ = app_clone.emit("recording-error", msg);
            let _ = app_clone.emit("recording-stopped", serde_json::json!({ "path": "" }));
        }
    });

    Ok(())
}

// ==================== CRASH RECOVERY ====================

#[tauri::command]
fn recover_temp_files(dir: String) -> Result<Vec<OrphanRecord>, String> {
    let path = Path::new(&dir);
    if !path.exists() { return Ok(Vec::new()); }
    let mut orphans = Vec::new();
    let mut seen_video_stems: std::collections::HashSet<String> = std::collections::HashSet::new();

    for entry in fs::read_dir(path).map_err(|e| e.to_string())?.flatten() {
        let p = entry.path();
        let name = match p.file_name().and_then(|n| n.to_str()) {
            Some(n) => n.to_string(),
            None => continue,
        };
        if let Some(stem) = name.strip_prefix("temp_vid_").and_then(|s| s.strip_suffix(".mp4")) {
            seen_video_stems.insert(stem.to_string());
        }
    }

    for stem in seen_video_stems {
        let temp_video = path.join(format!("temp_vid_{}.mp4", stem));
        let temp_sys = path.join(format!("temp_sys_{}.wav", stem));
        let temp_mic = path.join(format!("temp_mic_{}.wav", stem));
        let final_candidate = path.join(&stem);

        if final_candidate.exists() {
            let _ = fs::remove_file(&temp_video);
            if temp_sys.exists() { let _ = fs::remove_file(&temp_sys); }
            if temp_mic.exists() { let _ = fs::remove_file(&temp_mic); }
            continue;
        }

        orphans.push(OrphanRecord {
            temp_video: temp_video.to_string_lossy().to_string(),
            temp_sys: if temp_sys.exists() { Some(temp_sys.to_string_lossy().to_string()) } else { None },
            temp_mic: if temp_mic.exists() { Some(temp_mic.to_string_lossy().to_string()) } else { None },
            suggested_final: final_candidate.to_string_lossy().to_string(),
        });
    }
    Ok(orphans)
}

#[tauri::command]
async fn recover_recording(
    app: tauri::AppHandle,
    temp_video: String,
    temp_sys: Option<String>,
    temp_mic: Option<String>,
    final_path: String,
) -> Result<(), String> {
    let mut mux_args = vec!["-y".to_string(), "-i".to_string(), temp_video.clone()];
    let mut audio_count = 0;

    if let Some(sys) = &temp_sys {
        mux_args.extend(vec!["-i".to_string(), sys.clone()]);
        audio_count += 1;
    }
    if let Some(mic) = &temp_mic {
        mux_args.extend(vec!["-i".to_string(), mic.clone()]);
        audio_count += 1;
    }

    if audio_count == 2 {
        mux_args.extend(vec![
            "-filter_complex".to_string(),
            "[1:a][2:a]amix=inputs=2:duration=longest[aout]".to_string(),
            "-map".to_string(), "0:v".to_string(),
            "-map".to_string(), "[aout]".to_string(),
        ]);
    } else if audio_count == 1 {
        mux_args.extend(vec![
            "-map".to_string(), "0:v".to_string(),
            "-map".to_string(), "1:a".to_string(),
        ]);
    } else {
        mux_args.extend(vec!["-map".to_string(), "0:v".to_string()]);
    }

    mux_args.extend(vec![
        "-c:v".to_string(), "copy".to_string(),
        "-c:a".to_string(), "aac".to_string(),
        "-b:a".to_string(), "128k".to_string(),
        "-movflags".to_string(), "+faststart".to_string(),
        final_path.clone(),
    ]);

    let out = app
        .shell()
        .sidecar("ffmpeg")
        .map_err(|e| e.to_string())?
        .args(&mux_args)
        .output()
        .await
        .map_err(|e| e.to_string())?;

    if !out.status.success() {
        return Err(String::from_utf8_lossy(&out.stderr).to_string());
    }
    let _ = fs::remove_file(&temp_video);
    if let Some(sys) = temp_sys { let _ = fs::remove_file(sys); }
    if let Some(mic) = temp_mic { let _ = fs::remove_file(mic); }
    let _ = app.emit("recording-stopped", serde_json::json!({ "path": final_path }));
    Ok(())
}

#[tauri::command]
fn discard_orphan(
    temp_video: String,
    temp_sys: Option<String>,
    temp_mic: Option<String>,
) -> Result<(), String> {
    let _ = fs::remove_file(&temp_video);
    if let Some(sys) = temp_sys { let _ = fs::remove_file(sys); }
    if let Some(mic) = temp_mic { let _ = fs::remove_file(mic); }
    Ok(())
}

// ==================== FILES ====================

#[tauri::command]
fn list_recordings(dir: String) -> Result<Vec<RecordingFile>, String> {
    let mut out = Vec::new();
    let entries = match fs::read_dir(&dir) {
        Ok(e) => e,
        Err(_) => return Ok(out),
    };
    for entry in entries.flatten() {
        let path = entry.path();
        if !path.is_file() { continue; }
        let ext = path.extension().and_then(|e| e.to_str()).unwrap_or("").to_lowercase();
        if ext != "mp4" && ext != "gif" && ext != "png" { continue; }
        let name = path.file_name().and_then(|n| n.to_str()).unwrap_or("");
        if name.starts_with("temp_") { continue; }
        let meta = match entry.metadata() { Ok(m) => m, Err(_) => continue };
        let modified = meta
            .modified()
            .ok()
            .and_then(|t| t.duration_since(std::time::UNIX_EPOCH).ok())
            .map(|d| d.as_secs())
            .unwrap_or(0);
        out.push(RecordingFile {
            name: path.file_name().unwrap().to_string_lossy().to_string(),
            path: path.to_string_lossy().to_string(),
            size: meta.len(),
            modified,
        });
    }
    out.sort_by(|a, b| b.modified.cmp(&a.modified));
    Ok(out)
}

#[tauri::command]
fn open_in_folder(app: tauri::AppHandle, path: String) -> Result<(), String> {
    app.opener().reveal_item_in_dir(path).map_err(|e| e.to_string())
}

#[tauri::command]
fn open_file(app: tauri::AppHandle, path: String) -> Result<(), String> {
    app.opener().open_path(path, None::<&str>).map_err(|e| e.to_string())
}

#[tauri::command]
fn delete_recording(path: String) -> Result<(), String> {
    fs::remove_file(&path).map_err(|e| e.to_string())
}

// ==================== SHORTCUT ====================

#[tauri::command]
fn is_shortcut_registered(app: tauri::AppHandle, accelerator: String) -> bool {
    app.global_shortcut().is_registered(accelerator.as_str())
}

#[tauri::command]
fn set_shortcut(app: tauri::AppHandle, accelerator: String) -> Result<(), String> {
    let gs = app.global_shortcut();
    let _ = gs.unregister_all();
    if accelerator.trim().is_empty() { return Ok(()); }
    gs.register(accelerator.as_str()).map_err(|e| e.to_string())
}

#[tauri::command]
fn disable_shortcut(app: tauri::AppHandle) -> Result<(), String> {
    let _ = app.global_shortcut().unregister_all();
    Ok(())
}

// ==================== RUN ====================

#[cfg_attr(mobile, tauri::mobile_entry_point)]
pub fn run() {
    init_logging();

    tauri::Builder::default()
        .plugin(tauri_plugin_single_instance::init(|app, _args, _cwd| {
            if let Some(w) = app.get_webview_window("main") {
                let _ = w.show();
                let _ = w.set_focus();
            }
        }))
        .plugin(tauri_plugin_shell::init())
        .plugin(tauri_plugin_notification::init())
        .plugin(tauri_plugin_opener::init())
        .plugin(tauri_plugin_dialog::init())
        .manage(RecorderState(Mutex::new(None)))
        .manage(ReplayState(Mutex::new(None)))
        .invoke_handler(tauri::generate_handler![
            start_capture,
            stop_capture,
            take_screenshot,
            start_replay_buffer,
            save_replay_clip,
            check_disk_space,
            get_output_devices,
            get_input_devices,
            list_recordings,
            open_in_folder,
            open_file,
            delete_recording,
            set_shortcut,
            disable_shortcut,
            is_shortcut_registered,
            recover_temp_files,
            recover_recording,
            discard_orphan,
            probe_encoders,
            read_recent_log,
            set_autostart,
        ])
        .setup(|app| {
            let config_i = MenuItem::with_id(app, "config", "Ayarlar", true, None::<&str>)?;
            let show_i = MenuItem::with_id(app, "show", "Kayıt Ekranı", true, None::<&str>)?;
            let gallery_i = MenuItem::with_id(app, "gallery", "Galeri", true, None::<&str>)?;
            let folder_i = MenuItem::with_id(app, "folder", "Klasörü Aç", true, None::<&str>)?;
            let quit_i = MenuItem::with_id(app, "quit", "Çıkış", true, None::<&str>)?;
            let menu = Menu::with_items(app, &[&show_i, &gallery_i, &folder_i, &config_i, &quit_i])?;

            let _tray = TrayIconBuilder::new()
                .icon(app.default_window_icon().unwrap().clone())
                .menu(&menu)
                .tooltip("COSRecorder - Hazır")
                .on_menu_event(|app, event| match event.id().as_ref() {
                    "quit" => { app.exit(0); }
                    "show" => {
                        if let Some(w) = app.get_webview_window("main") {
                            let _ = w.show();
                            let _ = w.set_focus();
                        }
                    }
                    "gallery" => { open_sub_window(app, "gallery", "COSRecorder - Galeri", 900.0, 640.0) }
                    "config" => { open_sub_window(app, "config", "COSRecorder - Ayarlar", 620.0, 780.0) }
                    "folder" => { let _ = app.opener().reveal_item_in_dir(r"D:\Videolar"); }
                    _ => {}
                })
                .on_tray_icon_event(|tray, event| {
                    if let TrayIconEvent::Click { button: MouseButton::Left, .. } = event {
                        if let Some(w) = tray.app_handle().get_webview_window("main") {
                            let _ = w.show();
                            let _ = w.set_focus();
                        }
                    }
                })
                .build(app)?;
            Ok(())
        })
        .plugin(
            tauri_plugin_global_shortcut::Builder::new()
                .with_shortcuts(["ctrl+shift+r", "alt+c", "ctrl+shift+c"])
                .expect("Failed to register shortcut")
                .with_handler(|app, shortcut, event| {
                    if event.state == ShortcutState::Pressed {
                        let sc_str = shortcut.to_string().to_lowercase();
                        if sc_str.contains("alt+c") || sc_str.contains("ctrl+shift+c") {
                            let app_handle = app.clone();
                            tauri::async_runtime::spawn(async move {
                                let dir = "D:\\Videolar".to_string();
                                match save_replay_clip(app_handle.clone(), dir, app_handle.state()).await {
                                    Ok(path) => {
                                        log::info!("Klip oluşturuldu: {}", path);
                                    }
                                    Err(e) => {
                                        let _ = app_handle.notification()
                                            .builder()
                                            .title("COSRecorder Uyarısı")
                                            .body(&e)
                                            .show();
                                    }
                                }
                            });
                        } else {
                            let state = app.state::<RecorderState>();
                            let is_recording = state.0.lock().unwrap().is_some();
                            if is_recording {
                                let _ = stop_capture(app.clone(), state);
                            } else if let Some(w) = app.get_webview_window("main") {
                                let is_visible = w.is_visible().unwrap_or(false);
                                if is_visible {
                                    let _ = w.hide();
                                } else {
                                    let _ = w.show();
                                    let _ = w.set_focus();
                                }
                            }
                        }
                    }
                })
                .build(),
        )
        .on_window_event(|window, event| {
            if let tauri::WindowEvent::CloseRequested { api, .. } = event {
                if window.label() == "main" {
                    api.prevent_close();
                    let _ = window.hide();
                }
            }
        })
        .run(tauri::generate_context!())
        .expect("error while running tauri application");
}

fn open_sub_window(app: &tauri::AppHandle, label: &str, title: &str, w: f64, h: f64) {
    if let Some(win) = app.get_webview_window(label) {
        let _ = win.show();
        let _ = win.set_focus();
    } else {
        let _ = WebviewWindowBuilder::new(
            app,
            label,
            WebviewUrl::App(format!("index.html#/{}", label).into()),
        )
        .title(title)
        .inner_size(w, h)
        .resizable(false)
        .build();
    }
}
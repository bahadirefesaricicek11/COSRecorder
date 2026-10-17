use std::fs;
use std::path::Path;
use std::sync::Mutex;
use std::time::{SystemTime, UNIX_EPOCH};
use tauri::menu::{Menu, MenuItem};
use tauri::tray::{MouseButton, TrayIconBuilder, TrayIconEvent};
use tauri::{Emitter, Manager, State, WebviewUrl, WebviewWindowBuilder};
use tauri_plugin_global_shortcut::{Code, Modifiers, ShortcutState};
use tauri_plugin_shell::process::CommandChild;
use tauri_plugin_shell::ShellExt;

struct RecorderState(Mutex<Option<CommandChild>>);

fn generate_filename(is_gif: bool) -> String {
    let start = SystemTime::now();
    let since_the_epoch = start
        .duration_since(UNIX_EPOCH)
        .unwrap_or_default()
        .as_secs();

    let secs = since_the_epoch % 86400;
    let hours = secs / 3600;
    let minutes = (secs % 3600) / 60;
    let seconds = secs % 60;

    let days_since_epoch = since_the_epoch / 86400;
    let year = 1970 + (days_since_epoch / 365);
    let day_of_year = days_since_epoch % 365;
    let month = (day_of_year / 30) + 1;
    let day = (day_of_year % 30) + 1;

    let ext = if is_gif { "gif" } else { "mp4" };

    format!(
        "COSRecorder_{:04}-{:02}-{:02}_{:02}-{:02}-{:02}.{}",
        year, month, day, hours, minutes, seconds, ext
    )
}

#[tauri::command]
fn start_capture(
    app: tauri::AppHandle,
    x: i32,
    y: i32,
    w: i32,
    h: i32,
    fps: String,
    preset: String,
    encoder: String,
    export_gif: bool,
    output_dir: String,
    state: State<'_, RecorderState>,
) -> Result<(), String> {
    let mut recorder = state.0.lock().unwrap();
    if recorder.is_some() {
        return Err("Kayıt zaten devam ediyor.".into());
    }

    let target_dir_str = if output_dir.trim().is_empty() {
        r"D:\Videolar".to_string()
    } else {
        output_dir
    };

    let target_path = Path::new(&target_dir_str);
    if !target_path.exists() {
        fs::create_dir_all(target_path).map_err(|e| format!("Dizin oluşturulamadı: {}", e))?;
    }

    // YUV420p genişlik ve yükseklikte çift sayı (even number) gerektirir!
    let even_w = if w % 2 != 0 { w - 1 } else { w };
    let even_h = if h % 2 != 0 { h - 1 } else { h };

    let file_name = generate_filename(export_gif);
    let output_path = target_path.join(&file_name);

    let selected_codec = match encoder.as_str() {
        "nvenc" => "h264_nvenc",
        "amf" => "h264_amf",
        _ => "libx264",
    };

    let mut ffmpeg_args = vec![
        "-y".to_string(),
        "-f".to_string(),
        "gdigrab".to_string(),
        "-framerate".to_string(),
        fps,
        "-offset_x".to_string(),
        x.to_string(),
        "-offset_y".to_string(),
        y.to_string(),
        "-video_size".to_string(),
        format!("{}x{}", even_w, even_h),
        "-i".to_string(),
        "desktop".to_string(),
    ];

    if export_gif {
        ffmpeg_args.extend(vec![
            "-vf".to_string(),
            "fps=15,scale=800:-1:flags=lanczos,split[s0][s1];[s0]palettegen[p];[s1][p]paletteuse".to_string(),
            output_path.to_str().unwrap().to_string(),
        ]);
    } else {
        ffmpeg_args.extend(vec![
            "-c:v".to_string(),
            selected_codec.to_string(),
            "-pix_fmt".to_string(),
            "yuv420p".to_string(),
            "-preset".to_string(),
            preset,
            "-movflags".to_string(),
            "+faststart".to_string(),
            output_path.to_str().unwrap().to_string(),
        ]);
    }

    let (_rx, child) = app
        .shell()
        .sidecar("ffmpeg")
        .map_err(|e| format!("Sidecar izni veya dosya hatası: {}", e))?
        .args(&ffmpeg_args)
        .spawn()
        .map_err(|e| format!("FFmpeg başlatılamadı: {}", e))?;

    *recorder = Some(child);
    Ok(())
}

#[tauri::command]
fn stop_capture(app: tauri::AppHandle, state: State<'_, RecorderState>) -> Result<(), String> {
    let mut lock = state.0.lock().unwrap();

    if let Some(mut child) = lock.take() {
        let _ = child.write(b"q");
        let _ = app.emit("recording-stopped", ());
    }

    Ok(())
}

#[cfg_attr(mobile, tauri::mobile_entry_point)]
pub fn run() {
    tauri::Builder::default()
        .plugin(tauri_plugin_shell::init()) // Shell eklentisini kaydediyoruz
        .manage(RecorderState(Mutex::new(None)))
        .invoke_handler(tauri::generate_handler![start_capture, stop_capture])
        .setup(|app| {
            let config_i = MenuItem::with_id(app, "config", "Ayarlar", true, None::<&str>)?;
            let show_i = MenuItem::with_id(app, "show", "Kayıt Ekranı", true, None::<&str>)?;
            let quit_i = MenuItem::with_id(app, "quit", "Çıkış", true, None::<&str>)?;
            let menu = Menu::with_items(app, &[&show_i, &config_i, &quit_i])?;

            let _tray = TrayIconBuilder::new()
                .icon(app.default_window_icon().unwrap().clone())
                .menu(&menu)
                .tooltip("COSRecorder - Hazır")
                .on_menu_event(|app, event| match event.id().as_ref() {
                    "quit" => {
                        app.exit(0);
                    }
                    "show" => {
                        if let Some(window) = app.get_webview_window("main") {
                            let _ = window.show();
                            let _ = window.set_focus();
                        }
                    }
                    "config" => {
                        if let Some(config_win) = app.get_webview_window("config") {
                            let _ = config_win.show();
                            let _ = config_win.set_focus();
                        } else {
                            let _ = WebviewWindowBuilder::new(
                                app,
                                "config",
                                WebviewUrl::App("index.html#/config".into()),
                            )
                            .title("COSRecorder - Ayarlar")
                            .inner_size(620.0, 520.0)
                            .resizable(false)
                            .build();
                        }
                    }
                    _ => {}
                })
                .on_tray_icon_event(|tray, event| {
                    if let TrayIconEvent::Click {
                        button: MouseButton::Left,
                        ..
                    } = event
                    {
                        let app = tray.app_handle();
                        if let Some(window) = app.get_webview_window("main") {
                            let _ = window.show();
                            let _ = window.set_focus();
                        }
                    }
                })
                .build(app)?;

            Ok(())
        })
        .plugin(
            tauri_plugin_global_shortcut::Builder::new()
                .with_shortcuts(["ctrl+shift+r"])
                .expect("Failed to register shortcut")
                .with_handler(|app, shortcut, event| {
                    if event.state == ShortcutState::Pressed
                        && shortcut.matches(Modifiers::CONTROL | Modifiers::SHIFT, Code::KeyR)
                    {
                        let state = app.state::<RecorderState>();
                        let is_recording = state.0.lock().unwrap().is_some();

                        if let Some(window) = app.get_webview_window("main") {
                            if is_recording {
                                let app_handle = app.clone();
                                let _ = stop_capture(app_handle, state);
                                let _ = window.set_ignore_cursor_events(false);
                                let _ = window.show();
                                let _ = window.set_focus();
                            } else {
                                if window.is_visible().unwrap_or(false) {
                                    let _ = window.hide();
                                } else {
                                    let _ = window.show();
                                    let _ = window.set_focus();
                                }
                            }
                        }
                    }
                })
                .build(),
        )
        .run(tauri::generate_context!())
        .expect("error while running tauri application");
}
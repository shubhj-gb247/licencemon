//#![windows_subsystem = "windows"]
use anyhow::{Context, Result};
use chrono::Utc;
use image::ImageReader;
use image::{ImageBuffer, Rgba};
use once_cell::sync::Lazy;
use parking_lot::Mutex;
use serde::{Deserialize, Serialize};
use std::collections::HashSet;
use std::path;
use std::sync::{atomic::{AtomicBool, Ordering}, Arc};
use std::thread;
use std::time::Duration;
use tray_icon::{Icon, TrayIcon, TrayIconBuilder};
use windows::Win32::Foundation::*;
use windows::Win32::UI::WindowsAndMessaging::*;
static IS_RUNNING: Lazy<Arc<AtomicBool>> = Lazy::new(|| Arc::new(AtomicBool::new(false)));
static MONITOR_THREAD: Lazy<Arc<Mutex<Option<thread::JoinHandle<()>>>>> = Lazy::new(|| Arc::new(Mutex::new(None)));

#[derive(Debug, Clone, Deserialize, Serialize)]
struct AppConfig {
    server_url: String,
    interval_secs: u64,
}

impl Default for AppConfig {
    fn default() -> Self {
        Self {
            server_url: "http://127.0.0.1:8080/api/windows".to_string(),
            interval_secs: 3,
        }
    }
}

#[derive(Serialize)]
struct Payload {
    timestamp: String,
    host: String,
    windows: Vec<String>,
}

fn app_dirs() -> Result<(path::PathBuf, path::PathBuf)> {
    let proj = directories::ProjectDirs::from("", "Neilsoft", "licensemon").context("cannot resolve ProjectDirs")?;
    let cfg_dir = proj.config_dir().to_path_buf();
    let data_dir = proj.data_dir().to_path_buf();
    std::fs::create_dir_all(&cfg_dir).ok();
    std::fs::create_dir_all(&data_dir).ok();
    Ok((cfg_dir, data_dir))
}

fn load_config() -> Result<AppConfig> {
    let (cfg_dir, _) = app_dirs()?;
    let cfg_path = cfg_dir.join("config.json");
    if cfg_path.exists() {
        let s = std::fs::read_to_string(&cfg_path).context("read config.json")?;
        let cfg: AppConfig = serde_json::from_str(&s).context("parse config.json")?;
        Ok(cfg)
    } else {
        let cfg = AppConfig::default();
        std::fs::write(&cfg_path, serde_json::to_vec_pretty(&cfg)?).ok();
        Ok(cfg)
    }
}

fn load_keywords() -> Result<HashSet<String>> {
    let (cfg_dir, _) = app_dirs()?;
    let kw_path = cfg_dir.join("keywords.json");
    if kw_path.exists() {
        let s = std::fs::read_to_string(&kw_path).context("read keywords.json")?;
        let vals: Vec<String> = serde_json::from_str(&s).context("parse keywords.json")?;
        Ok(vals.into_iter().map(|s| s.to_lowercase()).collect())
    } else {
        let sample = vec!["chrome", "visual studio", "grafana", "notepad"];
        std::fs::write(&kw_path, serde_json::to_vec_pretty(&sample)?).ok();
        Ok(sample.into_iter().map(|s| s.to_string()).map(|s| s.to_lowercase()).collect())
    }
}

fn get_window_titles() -> Vec<String> {
    let mut titles = Vec::new();

    unsafe extern "system" fn enum_windows_proc(hwnd: HWND, lparam: LPARAM) -> BOOL {
        if IsWindowVisible(hwnd).as_bool() {
            let length = GetWindowTextLengthW(hwnd);
            if length > 0 {
                let mut buffer: Vec<u16> = vec![0; (length as usize) + 1];
                let copied = GetWindowTextW(hwnd, &mut buffer);
                if copied > 0 {
                    if let Ok(title) = String::from_utf16(&buffer[..copied as usize]) {
                        if !title.trim().is_empty() {
                            let titles = unsafe { &mut *(lparam.0 as *mut Vec<String>) };
                            titles.push(title);
                        }
                    }
                }
            }
        }
        true.into()
    }

    unsafe {
        let ptr = &mut titles as *mut _ as isize;
        EnumWindows(Some(enum_windows_proc), LPARAM(ptr)).expect("TODO: panic message");
    }

    titles
}

fn send_payload(server_url: &str, matched: Vec<String>) {
    let host = hostname::get().map(|h| h.to_string_lossy().into_owned()).unwrap_or_else(|_| "unknown".into());
    let payload = Payload {
        timestamp: Utc::now().to_rfc3339(),
        host,
        windows: matched,
    };

    let client = reqwest::blocking::Client::new();
    let res = client.post(server_url).json(&payload).send();
    match res {
        Ok(resp) => {
            if !resp.status().is_success() {
                eprintln!("[winwatch] server responded: {}", resp.status());
            }
        }
        Err(e) => eprintln!("[winwatch] send failed: {e:?}"),
    }
}

fn monitor_loop(is_running: Arc<AtomicBool>, cfg: AppConfig, _keywords: HashSet<String>) {
    let interval = Duration::from_secs(cfg.interval_secs.max(5));
    while is_running.load(Ordering::SeqCst) {
        let titles = get_window_titles();
        println!("{:#?}",titles);
        // let matched: Vec<String> = titles
        //     .into_iter()
        //     .filter(|t| {
        //         let lower = t.to_lowercase();
        //         keywords.iter().any(|k| lower.contains(k))
        //     })
        //     .collect();
        //
        // if !matched.is_empty() {
        //     send_payload(&cfg.server_url, matched);
        // }
        send_payload(&cfg.server_url,titles);

        let mut slept = Duration::ZERO;
        while slept < interval {
            if !is_running.load(Ordering::SeqCst) { break; }
            thread::sleep(Duration::from_millis(200));
            slept += Duration::from_millis(200);
        }
    }
}

fn build_icon() -> Icon {
    let size = 16u32;
    let mut img: ImageBuffer<Rgba<u8>, Vec<u8>> = ImageBuffer::new(size, size);
    for (x, y, p) in img.enumerate_pixels_mut() {
        let on = ((x / 2 + y / 2) % 2) == 0;
        *p = if on { Rgba([0u8, 140u8, 255u8, 255u8]) } else { Rgba([255, 255, 255, 255]) };
    }
    let rgba = img.into_raw();
    Icon::from_rgba(rgba, size, size).expect("icon")
}

fn load_icon(path: &str) -> Icon {
    let img = ImageReader::open(path)
        .expect("Failed to open icon file")
        .decode()
        .expect("Failed to decode image")
        .to_rgba8();
    let (width, height) = img.dimensions();
    Icon::from_rgba(img.into_raw(), width, height).expect("Failed to create icon")
}

fn main() -> Result<()> {
    let cfg = load_config()?;
    let keywords = load_keywords()?;
    let icon = build_icon();

    //let icon = load_icon("assets/ns.ico");

    let _tray: TrayIcon = TrayIconBuilder::new()
        .with_icon(icon)
        //.with_menu(Box::new(menu))
        .with_tooltip("NSLicenseMon")
        .build()?;

    IS_RUNNING.store(true, Ordering::SeqCst);
    let cfg_clone = cfg.clone();
    let kw_clone = keywords.clone();
    let is_running = IS_RUNNING.clone();
    let handle = thread::spawn(move || monitor_loop(is_running, cfg_clone, kw_clone));
    *MONITOR_THREAD.lock() = Some(handle);

    loop {
        std::thread::sleep(Duration::from_secs(1));
    }
}

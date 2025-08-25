//#![windows_subsystem = "windows"]
use chrono::{DateTime, Utc};
use directories::ProjectDirs;
use image::{ImageBuffer, Rgba};
use notify::Event;
use notify::event::ModifyKind;
use notify::{EventKind, RecursiveMode, Result, Watcher};
use once_cell::sync::Lazy;
use serde::Deserialize;
use serde_json::json;
use std::path::Path;
use std::sync::mpsc;
use std::{
    collections::HashMap,
    fs,
    sync::{Arc, Mutex},
    thread,
    time::Duration,
};
use tray_icon::{Icon, TrayIcon, TrayIconBuilder};
use windows::Win32::Foundation::*;
use windows::Win32::System::Threading::*;
use windows::Win32::UI::Accessibility::*;
use windows::Win32::UI::WindowsAndMessaging::*;
use windows::core::PWSTR;

// ================================
// Structs
// ================================
#[derive(Debug, serde::Serialize, Deserialize, Clone)]
struct TrackingConfig {
    applications: Vec<String>, // vector of strings representing process name which we have to track.
    interval: i64, // after every [interval] seconds of work on an app, send payload to server.
    server: String,
}

#[derive(Debug, Clone, serde::Serialize, Deserialize)]
struct TimeCell {
    time_start: Option<DateTime<Utc>>,
    time_end: Option<DateTime<Utc>>,
    duration: i64, // in seconds
    reporting_interval: i64,
}

impl TimeCell {
    fn update_start_time(&mut self) {
        self.time_start = Some(Utc::now());
    }

    fn update_end_time(&mut self, reporting_interval: i64) -> u8 {
        self.time_end = Some(Utc::now());
        match self.time_start {
            Some(time_start) => {
                let current = (self.time_end.unwrap() - time_start).num_seconds();
                self.duration = current;
                println!("Duration: {}s", self.duration);
                if current > reporting_interval { 1 } else { 0 }
            }
            None => {
                /*
                in the case when we switch to an untracked application, then the config changes make it tracked
                and when the said application is switched from, update_end_time is called but the start_time is none
                 */
                2
            }
        }
    }
}

#[derive(Debug, Clone)]
struct WinRecord {
    last: Option<String>,
}

// ================================
// Global state
// ================================
static CONFIG: Lazy<Mutex<Option<TrackingConfig>>> =
    Lazy::new(|| Mutex::new(load_tracking_config()));
static TIME_CELL_MAP: Lazy<Mutex<HashMap<String, TimeCell>>> =
    Lazy::new(|| Mutex::new(HashMap::new()));
static WIN_RECORD_INSTANCE: Lazy<Mutex<WinRecord>> =
    Lazy::new(|| Mutex::new(WinRecord { last: None }));
static PROCESS_NAME_CACHE: Lazy<Mutex<HashMap<u32, String>>> =
    Lazy::new(|| Mutex::new(HashMap::new()));
static CONFIG_PATH: Lazy<&Path> = Lazy::new(|| {Path::new("//10.1.3.154/Users/Admin/AppData/Roaming/Neilsoft/LicensemonTT/config/config.json")});

// ================================
// Load config
// ================================
fn load_tracking_config() -> Option<TrackingConfig> {
    // let proj_dirs =
    //     ProjectDirs::from("", "Neilsoft", "LicensemonTT").expect("Project directory not found");
    // let config_path = proj_dirs.config_dir().join("config.json");

    if !CONFIG_PATH.exists() {
        return None;
    }

    let data = fs::read_to_string(CONFIG_PATH.as_os_str()).ok()?;
    println!("Config: {}", data);
    serde_json::from_str(&data).ok()
}

fn reload_config() {
    if let Some(cfg) = load_tracking_config() {
        {
            let mut config_lock = CONFIG.lock().unwrap();
            *config_lock = Some(cfg.clone());
            println!("[Config] Reloaded config.json");
        }
        /*
        to create key-val pairs of new tracked applications.
        this approach can be improved by removing keys which exist in map but not in new config,
        and adding keys present in config but not in map.
        using retain and entry().or_insert() functions.

        Right now we nuke the entire map and create keys again.
        */
        init_time_cell();

        println!("Reloaded TimeCell Map");
    }
}

// ================================
// Watch config file
// ================================
fn watch_config_file() {
    // let proj_dirs = ProjectDirs::from("", "Neilsoft", "LicensemonTT").unwrap();
    // let config_path = proj_dirs.config_dir().join("config.json");

    let (tx, rx) = mpsc::channel::<Result<Event>>();
    let mut watcher = notify::recommended_watcher(tx).expect("Unable to create file watcher");
    // watcher.watch(Path::new("C:/Users/ShubhS/AppData/Roaming/Neilsoft/LicensemonTT/config/config.json"), RecursiveMode::Recursive).expect("Couldn't start watching");
    watcher
        .watch(Path::new(CONFIG_PATH.as_os_str()), RecursiveMode::Recursive)
        .expect("Couldn't start watching");

    println!("Watching: {}", CONFIG_PATH.display());
    for res in rx {
        match res {
            Ok(event) => {
                println!("{:?}", event);
                if event.kind == EventKind::Modify(ModifyKind::Any) {
                    println!("Modify Event -> Reloading Config");
                    reload_config();
                }
            }
            Err(e) => println!("watch error: {:?}", e),
        }
    }
}

// ================================
// Windows process detection
// ================================
fn get_process_name_from_hwnd(hwnd: HWND) -> Option<String> {
    unsafe {
        let mut pid: u32 = 0;
        GetWindowThreadProcessId(hwnd, Some(&mut pid));
        if pid == 0 {
            return None;
        }

        // check cache
        if let Some(name) = PROCESS_NAME_CACHE.lock().unwrap().get(&pid) {
            return Some(name.clone());
        }

        let h_process = OpenProcess(PROCESS_QUERY_LIMITED_INFORMATION, false, pid).ok()?;
        let mut buffer = [0u16; 260];
        let mut size = buffer.len() as u32;
        let result = QueryFullProcessImageNameW(
            h_process,
            PROCESS_NAME_FORMAT(0),
            PWSTR(buffer.as_mut_ptr()),
            &mut size,
        );
        CloseHandle(h_process).ok()?;
        if result.is_err() {
            return None;
        }

        let exe_path = String::from_utf16_lossy(&buffer[..size as usize])
            .split('\\')
            .last()?
            .to_string();
        PROCESS_NAME_CACHE
            .lock()
            .unwrap()
            .insert(pid, exe_path.clone());
        Some(exe_path)
    }
}

// ================================
// Focus change event
// ================================
unsafe extern "system" fn win_event_proc(
    _hook: HWINEVENTHOOK,
    event: u32,
    hwnd: HWND,
    _id_object: i32,
    _id_child: i32,
    _id_event_thread: u32,
    _dwms_event_time: u32,
) {
    if event == EVENT_SYSTEM_FOREGROUND {
        println!("=========NEW EVENT_SYSTEM_FOREGROUND=============");
        if let Some(app_name) = get_process_name_from_hwnd(hwnd) {
            let title_lower = app_name.to_lowercase();
            println!("Switched to: {}", title_lower.trim());

            let mut current_reporting_interval: i64 = 5;
            {
                let cfg = CONFIG.lock().unwrap();
                if let Some(cfg) = cfg.as_ref() {
                    current_reporting_interval = cfg.interval
                }
            }

            /*
            Update last app time
             */
            let mut send_payload_to_server: u8 = 0;
            let mut last_app = WIN_RECORD_INSTANCE.lock().unwrap();
            let data = last_app.last.as_ref();
            let mut last_app_str: String = String::new();
            match data {
                Some(app_str) => {
                    println!("last app name: {}", app_str);
                    last_app_str = app_str.clone();
                    let mut data_map = TIME_CELL_MAP.lock().unwrap();
                    let ref_data_last = data_map.get_mut(last_app.last.as_ref().unwrap());
                    match ref_data_last {
                        Some(data) => {
                            println!("Updating end time for {app_str}");
                            send_payload_to_server =
                                data.update_end_time(current_reporting_interval);
                        }
                        None => {
                            println!("Not a tracked last app.");
                        }
                    }
                }
                None => println!("No last app."),
            }

            if send_payload_to_server == 1 {
                send_payload(CONFIG.lock().unwrap().as_ref().unwrap().server.as_str(),last_app_str.as_str());
            }
            else if send_payload_to_server == 2 {
                println!(
                    "Corner Case [untracked application with None start_time was made tracked when config changed.]"
                )
            }

            last_app.last = None;

            let mut data_map = TIME_CELL_MAP.lock().unwrap();
            let ref_data = data_map.get_mut(&title_lower);
            match ref_data {
                Some(data) => {
                    println!("Updating start for {}", title_lower.trim());
                    data.update_start_time();
                }
                None => {
                    println!("Not interested in the switched application");
                    return;
                    /*
                    If we switched to a non-tracked app, we won't update it as last app and
                    return above.
                     */
                }
            }

            last_app.last = Some(title_lower);
        }
    }
}

// ================================
// Send payload
// ================================
fn send_payload(server: &str, key: &str) {
    let val = {
        let map = TIME_CELL_MAP.lock().unwrap();
        if let Some(cell) = map.get(key) {
            cell.clone()
        } else {
            return;
        }
    };

    let hostname = hostname::get().unwrap().to_string_lossy().to_string();
    let mut payload = serde_json::Map::new();
    payload.insert("hostname".into(), json!(hostname));
    payload.insert(key.into(), json!({ "duration": val.duration }));

    let client = reqwest::blocking::Client::new();
    match client
        .post(server)
        .header("Content-Type", "application/json")
        .json(&payload)
        .send()
    {
        Ok(resp) if resp.status().is_success() => {}
        Ok(resp) => eprintln!("Server responded with status: {}", resp.status()),
        Err(e) => eprintln!("Server error: {:?}", e),
    }
    println!("Payload sent: {:?}", payload);
}

// ================================
// Tray icon
// ================================
fn build_icon() -> Icon {
    let size = 16;
    let mut img: ImageBuffer<Rgba<u8>, Vec<u8>> = ImageBuffer::new(size, size);
    for (x, y, p) in img.enumerate_pixels_mut() {
        let on = ((x / 2 + y / 2) % 2) == 0;
        *p = if on {
            Rgba([0, 140, 255, 255])
        } else {
            Rgba([255, 255, 255, 255])
        };
    }
    Icon::from_rgba(img.into_raw(), size, size).unwrap()
}

fn init_time_cell() {
    let cfg = CONFIG.lock().unwrap().as_ref().unwrap().clone();
    {
        let mut map = TIME_CELL_MAP.lock().unwrap();
        map.clear(); //clearing required when config is reloaded.
        for app in &cfg.applications {
            map.entry(app.clone()).or_insert(TimeCell {
                time_start: None,
                time_end: None,
                duration: 0,
                reporting_interval: cfg.interval,
            });
        }
    }
}
// ================================
// Main
// ================================
fn main() {
    {
        let cfg_lock = CONFIG.lock().unwrap();
        if cfg_lock.is_none() {
            println!("No config file found.");
            thread::sleep(Duration::from_secs(3));
            return;
        }
    }

    let watcher_handle = thread::spawn(watch_config_file);

    // Initialize TIME_CELL_MAP
    init_time_cell();

    let icon = build_icon();
    let _tray_icon: Arc<TrayIcon> = Arc::new(
        TrayIconBuilder::new()
            .with_tooltip("LicenseMon")
            .with_icon(icon)
            .build()
            .unwrap(),
    );

    unsafe {
        let hook = SetWinEventHook(
            EVENT_SYSTEM_FOREGROUND,
            EVENT_SYSTEM_FOREGROUND,
            HINSTANCE::default(),
            Some(win_event_proc),
            0,
            0,
            WINEVENT_OUTOFCONTEXT,
        );

        println!("Tray app running. Listening for focus changes...");

        let mut msg = MSG::default();
        while GetMessageW(&mut msg, HWND(std::ptr::null_mut()), 0, 0).into() {
            let _ = TranslateMessage(&msg);
            DispatchMessageW(&msg);
        }

        let _ = UnhookWinEvent(hook);
    }

    watcher_handle.join().unwrap(); // not necessary I think
}

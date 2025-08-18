use chrono::{DateTime, Duration, Local, Utc};
use directories::ProjectDirs;
use image::{ImageBuffer, ImageReader, Rgba};
use once_cell::sync::Lazy;
use serde::Deserialize;
use std::{
    collections::HashMap,
    fs,
    path::Path,
    sync::{Arc, Mutex},
};
use std::process::Output;
use tray_icon::{Icon, TrayIcon, TrayIconBuilder};
use windows::Win32::Foundation::*;
use windows::Win32::Storage::FileSystem::*;
use windows::Win32::System::Diagnostics::ToolHelp::*;
use windows::Win32::System::Threading::*;
use windows::Win32::UI::Accessibility::*;
use windows::Win32::UI::WindowsAndMessaging::*;
use windows::core::PWSTR;

#[derive(Debug, serde::Serialize, Deserialize)]
struct TrackingConfig {
    applications: Vec<String>, // vector of strings representing process name which we have to track.
    interval: i64, // after every [interval] seconds send payload to server.
    server : String,
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

    /*
       Updates the end time & calculates duration using start_time and end_time.
       return 0 => marked end time,
              1 => marked end time, also send payload as user has worked for more than [reporting_interval] seconds
    */
    fn update_end_time(&mut self)  -> u8 {

        self.time_end = Some(Utc::now());
        let current = (self.time_end.unwrap() - self.time_start.unwrap()).num_seconds();
        self.duration = self.duration + current;

        if current > self.reporting_interval{
            return 1;
            send_payload(CONFIG.as_ref().unwrap().server.as_str())
        }
        println!("Duration: {}s ", &self.duration);
        return 0
    }
}

#[derive(Debug, Clone, serde::Serialize, Deserialize)]
struct WinRecord {
    last: Option<String>,
}
static CONFIG: Lazy<Option<TrackingConfig>> = Lazy::new(load_tracking_config);
static TIME_CELL_MAP: Lazy<Mutex<HashMap<String, TimeCell>>> =
    Lazy::new(|| Mutex::new(HashMap::new()));
static WIN_RECORD_INSTANCE: Lazy<Mutex<WinRecord>> =
    Lazy::new(|| Mutex::new(WinRecord { last: None }));
static PROCESS_NAME_CACHE: Lazy<Mutex<HashMap<u32, String>>> =
    Lazy::new(|| Mutex::new(HashMap::new()));

fn load_tracking_config() -> Option<TrackingConfig>{
    let proj_dirs =
        ProjectDirs::from("", "Neilsoft", "LicensemonTT").expect("Unable to get project dirs");
    let config_path = proj_dirs.config_dir().join("config.json");

    if !config_path.exists() {
        // let default = TrackingConfig {
        //     applications: vec!["notepad.exe".into(), "chrome.exe".into()],
        //     interval: 10,
        // };
        // fs::create_dir_all(proj_dirs.config_dir()).ok();
        // fs::write(
        //     &config_path,
        //     serde_json::to_string_pretty(&default).unwrap(),
        // )
        // .unwrap();
        // return default;
        /*
            If config file doesn't exist in server , return None
         */
        return None;
    }

    let data = fs::read_to_string(config_path).expect("Failed to read config.json");
    serde_json::from_str(&data).expect("Invalid JSON format in tracking.json")
}

fn save_tracking_log() {
    let proj_dirs =
        ProjectDirs::from("com", "example", "winwatch-tray").expect("Unable to get project dirs");
    let log_path = proj_dirs.data_dir().join("tracking_log.json");
    fs::create_dir_all(proj_dirs.data_dir()).ok();

    let data_map = TIME_CELL_MAP.lock().unwrap();
    let json = serde_json::to_string_pretty(&*data_map).unwrap();
    fs::write(&log_path, json).expect("Failed to write tracking_log.json");
}

fn get_process_name_from_hwnd(hwnd: HWND) -> Option<String> {
    unsafe {
        let mut pid: u32 = 0;

        GetWindowThreadProcessId(hwnd, Some(&mut pid));

        if pid == 0 {
            return None;
        }
        //check cache first
        {
            let cache = PROCESS_NAME_CACHE.lock().unwrap();
            if let Some(name) = cache.get(&pid) {
                println!("Returning cached app name ");
                return Some(name.clone());
            }
        }

        println!("Getting process handle");

        let h_process = OpenProcess(PROCESS_QUERY_LIMITED_INFORMATION, false, pid);

        match h_process {
            Err(error) => {
                println!("{error}");
                return None;
            }
            Ok(handle) => {
                let mut buffer = [0u16; 260];
                // let mut buffer_ptr= PWSTR::default();
                let mut size = buffer.len() as u32;

                let result = QueryFullProcessImageNameW(
                    handle,
                    windows::Win32::System::Threading::PROCESS_NAME_FORMAT(0),
                    PWSTR(buffer.as_mut_ptr()),
                    &mut size,
                );

                CloseHandle(handle).expect("Couldn't close handle");

                if result.is_err() {
                    return None;
                }

                let exe_path = String::from_utf16_lossy(&buffer[..size as usize]);
                let exe_path = exe_path.split('\\').last()?.to_string();
                {
                    // Insert pid and exe name in map
                    let mut cache = PROCESS_NAME_CACHE.lock().unwrap();
                    cache.insert(pid, exe_path.clone());
                }
                Some(exe_path)
            }
        }
    }
}

unsafe extern "system" fn win_event_proc(
    _hWinEventHook: HWINEVENTHOOK,
    event: u32,
    hwnd: HWND,
    _idObject: i32,
    _idChild: i32,
    _idEventThread: u32,
    _dwmsEventTime: u32,
) {
    if event == EVENT_SYSTEM_FOREGROUND {
        println!("=========NEW EVENT_SYSTEM_FOREGROUND=============");
        if let Some(app_name) = get_process_name_from_hwnd(hwnd) {
            let title_lower = app_name.to_lowercase();
            println!("Switched to: {}", title_lower.trim());


            /*
            Update last app time
             */
            let mut send_payload_to_server : u8 = 0;
            let mut last_app = WIN_RECORD_INSTANCE.lock().unwrap();
            let data = last_app.last.as_ref();
            match data {
                Some(app_str) => {
                    println!("last app name: {}", app_str);
                    let mut data_map = TIME_CELL_MAP.lock().unwrap();
                    let ref_data_last = data_map.get_mut(last_app.last.as_ref().unwrap());
                    match ref_data_last {
                        Some(data) => {
                            println!("Updating end time for {app_str}");
                            send_payload_to_server = data.update_end_time();
                        }
                        None => {
                            println!("Not a tracked last app.");
                        }
                    }
                }
                None => println!("No last app."),
            }
            if send_payload_to_server == 1{
                send_payload(CONFIG.as_ref().unwrap().server.as_str())
            }

            last_app.last = None; //clear the last app, if the app for which this current event run is generated is tracked last app will be set to that.

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


fn send_payload(server:&str){
    let payload : String ;
    {
        let map = TIME_CELL_MAP.lock().unwrap();
        /*
            derefrence the mutexguard and I get the hashmap so we do *map
            then we pass a reference to the hashmap in to_string() call, so we do &*map
         */
        payload = serde_json::to_string(&*map).unwrap();
    }
    let client = reqwest::blocking::Client::new();
    let res = client.post(server).json(&payload).send();
    match res{
        Ok(resp) => {
            if !resp.status().is_success(){
                eprintln!("Server responded with status code {}", resp.status());
            }
        },
        Err(error) => {
            eprintln!("Server responded with error {}", error);
        }
    }
    println!("{}", payload);
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
fn build_icon() -> Icon {
    let size = 16u32;
    let mut img: ImageBuffer<Rgba<u8>, Vec<u8>> = ImageBuffer::new(size, size);
    for (x, y, p) in img.enumerate_pixels_mut() {
        let on = ((x / 2 + y / 2) % 2) == 0;
        *p = if on {
            Rgba([0u8, 140u8, 255u8, 255u8])
        } else {
            Rgba([255, 255, 255, 255])
        };
    }
    let rgba = img.into_raw();
    Icon::from_rgba(rgba, size, size).expect("icon")
}
fn main() {

    if CONFIG.is_none(){
        println!("No config file detected .");
        return;
    }

    let config = CONFIG.as_ref();

    {
        /*
        Added scope to return the lock on TIME_CELL_MAP
         */

        let mut data_map = TIME_CELL_MAP.lock().unwrap();

        for x in &config.unwrap().applications{

            println!("{}", x);

            data_map.insert(
                x.clone(),
                TimeCell {
                    time_start: None,
                    time_end: None,
                    duration: 0,
                    reporting_interval: config.unwrap().interval //check if this call can be improved
                },
            );
        }
    }

    //let icon = load_icon("assets/ns.png");
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

        println!("Tray app running with custom icon. Listening for focus changes...");

        let mut msg = MSG::default();
        while GetMessageW(&mut msg, HWND(std::ptr::null_mut()), 0, 0).into() {
            TranslateMessage(&msg);
            DispatchMessageW(&msg);
        }

        UnhookWinEvent(hook);
    }
}

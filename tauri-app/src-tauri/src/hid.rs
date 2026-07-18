// Headset battery reading over raw USB/HID.
// Faithful port of the original C# HidApi.cs + HidApiProtocol.cs logic.
//
// IMPORTANT (macOS): every hidapi call runs on ONE dedicated, long-lived thread
// (see `sender` / `worker`). Enumerating the IOHIDManager from many different
// threads - which is what happens when each poll runs on the async blocking pool -
// corrupts CoreFoundation state and hard-crashes with SIGILL inside
// CFRunLoopAddSource. A single worker thread that reuses one HidApi keeps it stable.

use hidapi::{DeviceInfo, HidApi, HidDevice};
use serde::Serialize;
use std::sync::mpsc;
use std::sync::{Mutex, OnceLock};

// Supported (vendor_id, product_id) pairs, in priority order (matches the original).
const DEVICES: &[(u16, u16)] = &[
    (0x0951, 0x1718), // Kingston Cloud II Wireless
    (0x03F0, 0x018B), // HP Cloud II Wireless
    (0x03F0, 0x0995), // HP Cloud II Core Wireless
    (0x03F0, 0x098D), // HP Cloud Alpha Wireless
    (0x03F0, 0x0696), // HP Cloud II Wireless (variant B)
    (0x03F0, 0x0D93), // HP Cloud Stinger 2 Wireless
];

#[derive(Serialize, Clone)]
pub struct BatteryReading {
    pub success: bool,
    pub percent: i32,
    pub message: String,
    pub label: String,
}

impl BatteryReading {
    fn fail(message: &str, label: String) -> Self {
        BatteryReading {
            success: false,
            percent: 0,
            message: message.to_string(),
            label,
        }
    }
}

#[derive(Serialize, Clone)]
pub struct DeviceRow {
    pub vid: String,
    pub pid: String,
    pub usage: i32,
    pub usage_page: i32,
    pub manufacturer: String,
    pub product: String,
    pub path: String,
}

// ---- Dedicated HID thread ----

enum Request {
    Read(mpsc::Sender<BatteryReading>),
    List(mpsc::Sender<Vec<DeviceRow>>),
}

// Lazily starts the single HID worker thread on first use and hands back its channel.
fn sender() -> &'static Mutex<mpsc::Sender<Request>> {
    static SENDER: OnceLock<Mutex<mpsc::Sender<Request>>> = OnceLock::new();
    SENDER.get_or_init(|| {
        let (tx, rx) = mpsc::channel::<Request>();
        std::thread::Builder::new()
            .name("hid-worker".into())
            .spawn(move || worker(rx))
            .ok();
        Mutex::new(tx)
    })
}

// Owns one HidApi for the whole app life and serves a single request at a time.
fn worker(rx: mpsc::Receiver<Request>) {
    let mut api: Option<HidApi> = HidApi::new().ok();
    while let Ok(req) = rx.recv() {
        if api.is_none() {
            api = HidApi::new().ok();
        }
        match req {
            Request::Read(reply) => {
                let reading = match api.as_mut() {
                    Some(a) => read_battery_inner(a),
                    None => BatteryReading::fail("HID init failed.", String::new()),
                };
                let _ = reply.send(reading);
            }
            Request::List(reply) => {
                let rows = match api.as_mut() {
                    Some(a) => list_devices_inner(a),
                    None => Vec::new(),
                };
                let _ = reply.send(rows);
            }
        }
    }
}

pub fn read_battery() -> BatteryReading {
    let (tx, rx) = mpsc::channel();
    let sent = sender().lock().ok().map(|s| s.send(Request::Read(tx)));
    match sent {
        Some(Ok(())) => rx
            .recv()
            .unwrap_or_else(|_| BatteryReading::fail("HID worker stopped.", String::new())),
        _ => BatteryReading::fail("HID unavailable.", String::new()),
    }
}

pub fn list_devices() -> Vec<DeviceRow> {
    let (tx, rx) = mpsc::channel();
    let sent = sender().lock().ok().map(|s| s.send(Request::List(tx)));
    match sent {
        Some(Ok(())) => rx.recv().unwrap_or_default(),
        _ => Vec::new(),
    }
}

// ---- HID logic (only ever called on the worker thread) ----

// Pick the first supported (vid,pid) pair that has any device present, then the
// interface with the highest usage / usage_page (same selection as the original).
fn select_device(api: &HidApi) -> Option<&DeviceInfo> {
    for &(vid, pid) in DEVICES {
        let mut best: Option<&DeviceInfo> = None;
        for info in api.device_list() {
            if info.vendor_id() == vid && info.product_id() == pid {
                best = Some(match best {
                    None => info,
                    Some(b) => {
                        if info.usage() > b.usage()
                            || (info.usage() == b.usage() && info.usage_page() >= b.usage_page())
                        {
                            info
                        } else {
                            b
                        }
                    }
                });
            }
        }
        if best.is_some() {
            return best;
        }
    }
    None
}

fn read_battery_inner(api: &mut HidApi) -> BatteryReading {
    // Re-scan so a headset connected after launch is picked up.
    if api.refresh_devices().is_err() {
        return BatteryReading::fail("HID refresh failed.", String::new());
    }

    let info = match select_device(api) {
        Some(i) => i,
        None => return BatteryReading::fail("No headset device detected.", String::new()),
    };

    let manufacturer = info.manufacturer_string().unwrap_or("");
    let product = info.product_string().unwrap_or("");
    let label = format!("{manufacturer} {product}").trim().to_string();

    let dev = match api.open_path(info.path()) {
        Ok(d) => d,
        Err(_) => return BatteryReading::fail("Could not connect to headset.", label),
    };

    let pct = get_battery_level(&dev, manufacturer, product);

    if pct == 0 {
        return BatteryReading::fail("Headset found but not active.", label);
    }
    if !(0..=100).contains(&pct) {
        return BatteryReading::fail("Battery N/A.", label);
    }

    BatteryReading {
        success: true,
        percent: pct,
        message: String::new(),
        label,
    }
}

// Direct translation of the working protocol (buffer sizes, offsets, report priming).
fn get_battery_level(dev: &HidDevice, manufacturer: &str, product: &str) -> i32 {
    const WRITE_BUFFER_SIZE: usize = 52;
    const DATA_BUFFER_SIZE: usize = 20;

    let mut write_buffer = [0u8; WRITE_BUFFER_SIZE];
    let mut battery_index: usize = 7;

    let man = manufacturer.to_uppercase();
    let prod = product.to_uppercase();

    if man.contains("HP") {
        if prod.contains("CLOUD II CORE") {
            write_buffer[0] = 0x66;
            write_buffer[1] = 0x89;
            battery_index = 4;
        } else if prod.contains("CLOUD II WIRELESS") || prod.contains("CLOUD STINGER 2 WIRELESS") {
            write_buffer[0] = 0x06;
            write_buffer[1] = 0xFF;
            write_buffer[2] = 0xBB;
            write_buffer[3] = 0x02;
            battery_index = 7;
        } else if prod.contains("CLOUD ALPHA WIRELESS") {
            write_buffer[0] = 0x21;
            write_buffer[1] = 0xBB;
            write_buffer[2] = 0x0B;
            battery_index = 3;
        }
    } else {
        // Kingston Cloud II: prime input report before writes.
        const INPUT_BUFFER_SIZE: usize = 160;
        let mut buf = [0u8; INPUT_BUFFER_SIZE];
        buf[0] = 0x06; // report id
        let _ = dev.get_input_report(&mut buf);

        write_buffer[0] = 0x06;
        write_buffer[2] = 0x02;
        write_buffer[4] = 0x9A;
        write_buffer[7] = 0x68;
        write_buffer[8] = 0x4A;
        write_buffer[9] = 0x8E;
        write_buffer[10] = 0x0A;
        write_buffer[14] = 0xBB;
        write_buffer[15] = 0x02;
        battery_index = 7;
    }

    if dev.write(&write_buffer).is_err() {
        return -1;
    }

    let mut data_buffer = [0u8; DATA_BUFFER_SIZE];
    let _ = dev.read_timeout(&mut data_buffer, 1000);

    if battery_index >= data_buffer.len() {
        return -1;
    }
    data_buffer[battery_index] as i32
}

fn list_devices_inner(api: &mut HidApi) -> Vec<DeviceRow> {
    let _ = api.refresh_devices();

    let mut rows = Vec::new();
    for &(vid, pid) in DEVICES {
        for info in api.device_list() {
            if info.vendor_id() == vid && info.product_id() == pid {
                rows.push(DeviceRow {
                    vid: format!("0x{:04X}", info.vendor_id()),
                    pid: format!("0x{:04X}", info.product_id()),
                    usage: info.usage() as i32,
                    usage_page: info.usage_page() as i32,
                    manufacturer: info.manufacturer_string().unwrap_or("").to_string(),
                    product: info.product_string().unwrap_or("").to_string(),
                    path: info.path().to_string_lossy().to_string(),
                });
            }
        }
    }
    rows
}

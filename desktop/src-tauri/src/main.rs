#![cfg_attr(not(debug_assertions), windows_subsystem = "windows")]

mod adb;
mod geo;
mod ios;

use adb::{distance_m, kmh_to_mps, parse_host_port, parse_pairing_code, Adb, Device, LatLng, Status, MAX_JUMP_M};
use ios::IosSession;
use serde::Serialize;
use std::collections::HashMap;
use std::sync::{Arc, Mutex};
use std::time::Duration;
use tauri::{Manager, RunEvent};

/// アプリ終了時の cleanup に使う、現在操作中の端末。
#[derive(Default)]
struct Session {
    serial: Option<String>,
    platform: Option<String>,
    stop_on_exit: bool,
}

/// iOS 端末ごとのシミュレーション状態（iOS は端末側アプリがないので Mac 側で保持する）。
#[derive(Default)]
struct IosState {
    sessions: HashMap<String, IosSession>,
    versions: HashMap<String, String>,
}

type SharedIos = Arc<Mutex<IosState>>;

/// adb / pymobiledevice3 はブロッキングなので UI スレッドを止めないよう別スレッドで実行する。
async fn blocking<T: Send + 'static>(f: impl FnOnce() -> Result<T, String> + Send + 'static) -> Result<T, String> {
    tauri::async_runtime::spawn_blocking(f).await.map_err(|e| e.to_string())?
}

fn adb_for(serial: &str) -> Result<Adb, String> {
    Adb::new(Some(serial.to_string()))
}

fn is_ios(platform: &str) -> bool {
    platform == "ios"
}

/// iOS 端末のセッションを取得（なければ作成）して処理する。
fn with_ios<T>(ios: &SharedIos, udid: &str, f: impl FnOnce(&mut IosSession) -> Result<T, String>) -> Result<T, String> {
    let mut state = ios.lock().map_err(|_| "内部エラー: 状態をロックできません".to_string())?;
    if !state.sessions.contains_key(udid) {
        let version = state.versions.get(udid).cloned();
        let session = IosSession::new(ios::find_pmd3()?, udid, version.as_deref())?;
        state.sessions.insert(udid.to_string(), session);
    }
    f(state.sessions.get_mut(udid).unwrap())
}

#[derive(Serialize)]
struct DeviceList {
    devices: Vec<Device>,
    /// 片方（adb / pymobiledevice3）が使えないときの案内
    warnings: Vec<String>,
}

#[tauri::command]
async fn list_devices(ios_state: tauri::State<'_, SharedIos>) -> Result<DeviceList, String> {
    let ios_state = ios_state.inner().clone();
    blocking(move || {
        let mut devices = Vec::new();
        let mut warnings = Vec::new();
        match Adb::new(None).and_then(|adb| adb.devices()) {
            Ok(d) => devices.extend(d),
            Err(e) => warnings.push(format!("Android: {e}")),
        }
        match ios::list_devices() {
            Ok(d) => {
                let mut state = ios_state.lock().unwrap();
                for dev in &d {
                    if let Some(v) = &dev.os_version {
                        state.versions.insert(dev.serial.clone(), v.clone());
                    }
                }
                devices.extend(d);
            }
            Err(e) => warnings.push(format!("iOS: {e}")),
        }
        if warnings.len() == 2 {
            return Err(warnings.join("\n"));
        }
        Ok(DeviceList { devices, warnings })
    })
    .await
}

/// ワイヤレスデバッグのペア設定（初回のみ、Android）。
#[tauri::command]
async fn pair_device(host: String, code: String) -> Result<String, String> {
    blocking(move || {
        let host = parse_host_port(&host)?;
        let code = parse_pairing_code(&code)?;
        Adb::new(None)?.pair(&host, &code)
    })
    .await
}

/// ワイヤレスデバッグで接続し、端末一覧に出るシリアル（host:port）を返す（Android）。
#[tauri::command]
async fn connect_device(host: String) -> Result<String, String> {
    blocking(move || {
        let host = parse_host_port(&host)?;
        Adb::new(None)?.connect(&host)?;
        Ok(host)
    })
    .await
}

#[tauri::command]
async fn disconnect_device(host: String) -> Result<String, String> {
    blocking(move || {
        let host = parse_host_port(&host)?;
        Adb::new(None)?.disconnect(&host)
    })
    .await
}

#[tauri::command]
async fn get_status(ios_state: tauri::State<'_, SharedIos>, serial: String, platform: String) -> Result<Status, String> {
    let ios_state = ios_state.inner().clone();
    blocking(move || {
        if is_ios(&platform) {
            with_ios(&ios_state, &serial, |s| Ok(s.status()))
        } else {
            adb_for(&serial)?.status()
        }
    })
    .await
}

#[tauri::command]
async fn setup_device(serial: String, platform: String, apk: Option<String>) -> Result<String, String> {
    blocking(move || {
        if is_ios(&platform) {
            return ios::setup(&serial);
        }
        let adb = adb_for(&serial)?;
        let mut log = String::new();
        if let Some(apk) = apk.filter(|a| !a.trim().is_empty()) {
            log += &adb.install(apk.trim())?;
            log += "\n";
        }
        adb.grant_permissions()?;
        log += "位置情報の権限を付与し、「仮の現在地情報アプリ」に設定しました。";
        Ok(log)
    })
    .await
}

#[tauri::command]
async fn set_position(
    ios_state: tauri::State<'_, SharedIos>,
    serial: String,
    platform: String,
    point: LatLng,
    force: bool,
) -> Result<Status, String> {
    let ios_state = ios_state.inner().clone();
    blocking(move || {
        let point = point.validated()?;
        let current = if is_ios(&platform) {
            with_ios(&ios_state, &serial, |s| Ok(s.status().position))?
        } else {
            adb_for(&serial)?.status()?.position
        };
        if !force {
            if let Some(current) = current {
                let jump = distance_m(current, point);
                if jump > MAX_JUMP_M {
                    // フロントエンドが確認ダイアログを出すための識別子
                    return Err(format!("JUMP:{jump:.0}"));
                }
            }
        }
        if is_ios(&platform) {
            return with_ios(&ios_state, &serial, |s| s.hold(point));
        }
        let adb = adb_for(&serial)?;
        adb.send("SET", &[("point", point.format())])?;
        adb.wait_for(&["HOLDING"], Duration::from_secs(5))
    })
    .await
}

#[tauri::command]
async fn start_walk(
    ios_state: tauri::State<'_, SharedIos>,
    serial: String,
    platform: String,
    waypoints: Vec<LatLng>,
    from: Option<LatLng>,
    speed_kmh: f64,
) -> Result<Status, String> {
    let ios_state = ios_state.inner().clone();
    blocking(move || {
        if waypoints.is_empty() {
            return Err("目的地を地図上で指定してください".into());
        }
        let waypoints = waypoints.into_iter().map(LatLng::validated).collect::<Result<Vec<_>, _>>()?;
        let from = from.map(LatLng::validated).transpose()?;
        let speed = kmh_to_mps(speed_kmh)?;
        if is_ios(&platform) {
            return with_ios(&ios_state, &serial, |s| s.walk(waypoints, from, speed));
        }
        let mut extras = vec![
            ("to", waypoints.iter().map(LatLng::format).collect::<Vec<_>>().join(";")),
            ("speed", format!("{speed:.4}")),
        ];
        if let Some(from) = from {
            extras.push(("from", from.format()));
        }
        let adb = adb_for(&serial)?;
        adb.send("WALK", &extras)?;
        adb.wait_for(&["WALKING", "ARRIVED"], Duration::from_secs(5))
    })
    .await
}

#[tauri::command]
async fn pause_walk(ios_state: tauri::State<'_, SharedIos>, serial: String, platform: String) -> Result<Status, String> {
    let ios_state = ios_state.inner().clone();
    blocking(move || {
        if is_ios(&platform) {
            return with_ios(&ios_state, &serial, |s| s.pause());
        }
        let adb = adb_for(&serial)?;
        adb.send("PAUSE", &[])?;
        adb.wait_for(&["PAUSED", "ARRIVED", "HOLDING"], Duration::from_secs(5))
    })
    .await
}

#[tauri::command]
async fn resume_walk(ios_state: tauri::State<'_, SharedIos>, serial: String, platform: String) -> Result<Status, String> {
    let ios_state = ios_state.inner().clone();
    blocking(move || {
        if is_ios(&platform) {
            return with_ios(&ios_state, &serial, |s| s.resume());
        }
        let adb = adb_for(&serial)?;
        adb.send("RESUME", &[])?;
        adb.wait_for(&["WALKING", "ARRIVED", "HOLDING"], Duration::from_secs(5))
    })
    .await
}

#[tauri::command]
async fn set_speed(ios_state: tauri::State<'_, SharedIos>, serial: String, platform: String, speed_kmh: f64) -> Result<(), String> {
    let ios_state = ios_state.inner().clone();
    blocking(move || {
        let speed = kmh_to_mps(speed_kmh)?;
        if is_ios(&platform) {
            return with_ios(&ios_state, &serial, |s| s.set_speed(speed)).map(|_| ());
        }
        adb_for(&serial)?.send("SPEED", &[("speed", format!("{speed:.4}"))])
    })
    .await
}

#[tauri::command]
async fn stop_simulation(ios_state: tauri::State<'_, SharedIos>, serial: String, platform: String) -> Result<Status, String> {
    let ios_state = ios_state.inner().clone();
    blocking(move || {
        if is_ios(&platform) {
            return with_ios(&ios_state, &serial, |s| s.stop());
        }
        let adb = adb_for(&serial)?;
        adb.send("STOP", &[])?;
        // サービス終了（= テストプロバイダ解除）を確認できるまで完了扱いにしない
        adb.wait_for(&["IDLE", "NOT_RUNNING"], Duration::from_secs(5))
    })
    .await
}

#[tauri::command]
fn set_session(state: tauri::State<Mutex<Session>>, serial: Option<String>, platform: Option<String>, stop_on_exit: bool) {
    let mut s = state.lock().unwrap();
    s.serial = serial;
    s.platform = platform;
    s.stop_on_exit = stop_on_exit;
}

/// 終了時に、シミュレーション中なら停止して通常位置に戻す。
fn cleanup_on_exit(app: &tauri::AppHandle) {
    let (serial, platform, stop) = {
        let s = app.state::<Mutex<Session>>();
        let s = s.lock().unwrap();
        (s.serial.clone(), s.platform.clone(), s.stop_on_exit)
    };
    if !stop {
        return;
    }
    // iOS はこのアプリのプロセスが位置を保持しているので、操作したすべての端末を解除する
    if let Ok(mut state) = app.state::<SharedIos>().lock() {
        for (udid, session) in state.sessions.iter_mut() {
            if session.is_active() {
                if let Err(e) = session.stop() {
                    eprintln!("cleanup on exit failed for {udid}: {e}");
                }
            }
        }
    }
    let Some(serial) = serial.filter(|_| platform.as_deref() != Some("ios")) else { return };
    let Ok(adb) = adb_for(&serial) else { return };
    if adb.status().map(|s| s.is_active()).unwrap_or(false) {
        if let Err(e) = adb.send("STOP", &[]) {
            eprintln!("cleanup on exit failed: {e}");
        }
    }
}

fn main() {
    let app = tauri::Builder::default()
        .manage(Mutex::new(Session { serial: None, platform: None, stop_on_exit: true }))
        .manage(SharedIos::default())
        .invoke_handler(tauri::generate_handler![
            list_devices,
            pair_device,
            connect_device,
            disconnect_device,
            get_status,
            setup_device,
            set_position,
            start_walk,
            pause_walk,
            resume_walk,
            set_speed,
            stop_simulation,
            set_session,
        ])
        .build(tauri::generate_context!())
        .expect("error while building tauri application");

    app.run(|handle, event| {
        if let RunEvent::Exit = event {
            cleanup_on_exit(handle);
        }
    });
}

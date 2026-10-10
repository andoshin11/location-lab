#![cfg_attr(not(debug_assertions), windows_subsystem = "windows")]

mod adb;

use adb::{distance_m, kmh_to_mps, Adb, Device, LatLng, Status, MAX_JUMP_M};
use std::sync::Mutex;
use std::time::Duration;
use tauri::{Manager, RunEvent};

/// アプリ終了時の cleanup に使う、現在操作中の端末。
#[derive(Default)]
struct Session {
    serial: Option<String>,
    stop_on_exit: bool,
}

/// adb はブロッキングなので UI スレッドを止めないよう別スレッドで実行する。
async fn blocking<T: Send + 'static>(f: impl FnOnce() -> Result<T, String> + Send + 'static) -> Result<T, String> {
    tauri::async_runtime::spawn_blocking(f).await.map_err(|e| e.to_string())?
}

fn adb_for(serial: &str) -> Result<Adb, String> {
    Adb::new(Some(serial.to_string()))
}

#[tauri::command]
async fn list_devices() -> Result<Vec<Device>, String> {
    blocking(|| Adb::new(None)?.devices()).await
}

#[tauri::command]
async fn get_status(serial: String) -> Result<Status, String> {
    blocking(move || adb_for(&serial)?.status()).await
}

#[tauri::command]
async fn setup_device(serial: String, apk: Option<String>) -> Result<String, String> {
    blocking(move || {
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
async fn set_position(serial: String, point: LatLng, force: bool) -> Result<Status, String> {
    blocking(move || {
        let point = point.validated()?;
        let adb = adb_for(&serial)?;
        if !force {
            if let Some(current) = adb.status()?.position {
                let jump = distance_m(current, point);
                if jump > MAX_JUMP_M {
                    // フロントエンドが確認ダイアログを出すための識別子
                    return Err(format!("JUMP:{jump:.0}"));
                }
            }
        }
        adb.send("SET", &[("point", point.format())])?;
        adb.wait_for(&["HOLDING"], Duration::from_secs(5))
    })
    .await
}

#[tauri::command]
async fn start_walk(serial: String, waypoints: Vec<LatLng>, from: Option<LatLng>, speed_kmh: f64) -> Result<Status, String> {
    blocking(move || {
        if waypoints.is_empty() {
            return Err("目的地を地図上で指定してください".into());
        }
        let waypoints = waypoints.into_iter().map(LatLng::validated).collect::<Result<Vec<_>, _>>()?;
        let speed = kmh_to_mps(speed_kmh)?;
        let mut extras = vec![
            ("to", waypoints.iter().map(LatLng::format).collect::<Vec<_>>().join(";")),
            ("speed", format!("{speed:.4}")),
        ];
        if let Some(from) = from {
            extras.push(("from", from.validated()?.format()));
        }
        let adb = adb_for(&serial)?;
        adb.send("WALK", &extras)?;
        adb.wait_for(&["WALKING", "ARRIVED"], Duration::from_secs(5))
    })
    .await
}

#[tauri::command]
async fn pause_walk(serial: String) -> Result<Status, String> {
    blocking(move || {
        let adb = adb_for(&serial)?;
        adb.send("PAUSE", &[])?;
        adb.wait_for(&["PAUSED", "ARRIVED", "HOLDING"], Duration::from_secs(5))
    })
    .await
}

#[tauri::command]
async fn resume_walk(serial: String) -> Result<Status, String> {
    blocking(move || {
        let adb = adb_for(&serial)?;
        adb.send("RESUME", &[])?;
        adb.wait_for(&["WALKING", "ARRIVED", "HOLDING"], Duration::from_secs(5))
    })
    .await
}

#[tauri::command]
async fn set_speed(serial: String, speed_kmh: f64) -> Result<(), String> {
    blocking(move || {
        let speed = kmh_to_mps(speed_kmh)?;
        adb_for(&serial)?.send("SPEED", &[("speed", format!("{speed:.4}"))])
    })
    .await
}

#[tauri::command]
async fn stop_simulation(serial: String) -> Result<Status, String> {
    blocking(move || {
        let adb = adb_for(&serial)?;
        adb.send("STOP", &[])?;
        // サービス終了（= テストプロバイダ解除）を確認できるまで完了扱いにしない
        adb.wait_for(&["IDLE", "NOT_RUNNING"], Duration::from_secs(5))
    })
    .await
}

#[tauri::command]
fn set_session(state: tauri::State<Mutex<Session>>, serial: Option<String>, stop_on_exit: bool) {
    let mut s = state.lock().unwrap();
    s.serial = serial;
    s.stop_on_exit = stop_on_exit;
}

/// 終了時に、シミュレーション中なら停止して通常位置に戻す。
fn cleanup_on_exit(app: &tauri::AppHandle) {
    let (serial, stop) = {
        let s = app.state::<Mutex<Session>>();
        let s = s.lock().unwrap();
        (s.serial.clone(), s.stop_on_exit)
    };
    let Some(serial) = serial.filter(|_| stop) else { return };
    let Ok(adb) = adb_for(&serial) else { return };
    if adb.status().map(|s| s.is_active()).unwrap_or(false) {
        if let Err(e) = adb.send("STOP", &[]) {
            eprintln!("cleanup on exit failed: {e}");
        }
    }
}

fn main() {
    let app = tauri::Builder::default()
        .manage(Mutex::new(Session { serial: None, stop_on_exit: true }))
        .invoke_handler(tauri::generate_handler![
            list_devices,
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

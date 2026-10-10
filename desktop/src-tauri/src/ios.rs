//! iOS 端末の位置シミュレーション。
//!
//! Apple の開発者向け位置シミュレーション（iOS 17 以降は DVT LocationSimulation、
//! iOS 16 以下は com.apple.dt.simulatelocation）を、外部コマンドの pymobiledevice3 経由で使う。
//! pymobiledevice3 は GPL-3.0-or-later のため、ライブラリとして組み込まず別プロセスとして呼び出す。
//!
//! iOS には端末側アプリを入れないので、徒歩移動は Mac 側で 1 秒ごとの地点列（GPX）を作って再生する。
//! 再生中に pymobiledevice3 が出力する「set location to」ログを読み、実際に端末へ送った位置を状態として返す。

use crate::adb::{run_command, Device, LatLng, Status};
use crate::geo::{sample_route, to_gpx, Route, Sample};
use std::fs::{self, File};
use std::path::PathBuf;
use std::process::{Child, Command, Stdio};
use std::time::{Duration, Instant};

/// トンネル確立（iOS 17 以降）などで時間がかかるため長めに待つ。
const START_TIMEOUT: Duration = Duration::from_secs(45);

pub fn find_pmd3() -> Result<PathBuf, String> {
    if let Ok(p) = std::env::var("LOCATIONLAB_PMD3") {
        return Ok(PathBuf::from(p));
    }
    let mut candidates: Vec<PathBuf> = std::env::var_os("PATH")
        .map(|p| std::env::split_paths(&p).map(|d| d.join("pymobiledevice3")).collect())
        .unwrap_or_default();
    if let Some(home) = std::env::var_os("HOME") {
        candidates.push(PathBuf::from(home).join(".local/bin/pymobiledevice3"));
    }
    candidates.push("/opt/homebrew/bin/pymobiledevice3".into());
    candidates.push("/usr/local/bin/pymobiledevice3".into());
    candidates.into_iter().find(|p| p.is_file()).ok_or_else(|| {
        "iOS 端末を使うには pymobiledevice3 が必要です（`uv tool install pymobiledevice3` 等でインストールしてください）。".into()
    })
}

pub fn major_version(version: &str) -> u32 {
    version.split('.').next().and_then(|v| v.parse().ok()).unwrap_or(0)
}

/// `pymobiledevice3 usbmux list` の JSON を端末一覧に変換する。
pub fn parse_usbmux_list(json: &str) -> Result<Vec<Device>, String> {
    let start = json.find('[').ok_or("pymobiledevice3 の出力を解釈できません")?;
    let items: Vec<serde_json::Value> =
        serde_json::from_str(&json[start..]).map_err(|e| format!("pymobiledevice3 の出力を解釈できません: {e}"))?;
    let mut devices: Vec<Device> = Vec::new();
    for item in items {
        let get = |k: &str| item.get(k).and_then(|v| v.as_str()).map(String::from);
        let Some(udid) = get("UniqueDeviceID").or_else(|| get("Identifier")) else { continue };
        // USB と Wi-Fi の両方で見えている端末は 1 件にまとめる
        if devices.iter().any(|d| d.serial == udid) {
            continue;
        }
        devices.push(Device {
            serial: udid,
            state: "device".into(),
            model: get("DeviceName").or_else(|| get("ProductType")),
            platform: "ios".into(),
            os_version: get("ProductVersion"),
        });
    }
    Ok(devices)
}

/// 再生ログから「set location to LAT LNG」の回数と最後の座標を取り出す。
pub fn parse_play_log(log: &str) -> (usize, Option<LatLng>) {
    let mut count = 0;
    let mut last = None;
    for line in log.lines() {
        if let Some(idx) = line.find("set location to ") {
            count += 1;
            let mut it = line[idx + "set location to ".len()..].split_whitespace();
            let lat = it.next().and_then(|v| v.parse().ok());
            let lng = it.next().and_then(|v| v.trim_end_matches(|c: char| !c.is_ascii_digit()).parse().ok());
            if let (Some(lat), Some(lng)) = (lat, lng) {
                last = LatLng::new(lat, lng).ok();
            }
        }
    }
    (count, last)
}

/// pymobiledevice3 のエラー出力を、利用者向けの案内に変換する。
pub fn explain_error(log: &str) -> String {
    let lower = log.to_ascii_lowercase();
    let hint = if lower.contains("developermode") || lower.contains("developer mode") {
        Some("端末で「設定 → プライバシーとセキュリティ → デベロッパモード」を ON にしてください。")
    } else if lower.contains("passwordrequired") || lower.contains("passcode") || lower.contains("locked") {
        Some("端末のロックを解除してから、もう一度実行してください。")
    } else if lower.contains("pairing") || lower.contains("notpaired") || lower.contains("trust") {
        Some("端末に表示される「このコンピュータを信頼しますか？」で「信頼」を選んでください。")
    } else if lower.contains("diskimage") || lower.contains("mount") || lower.contains("invalidservice") {
        Some("「端末セットアップ」を実行して、デベロッパディスクイメージをマウントしてください。")
    } else if lower.contains("nodeviceconnected") || lower.contains("device not found") || lower.contains("no device") {
        Some("端末が接続されていません。USB ケーブルを確認してください。")
    } else {
        None
    };
    let tail: Vec<&str> = log.lines().map(str::trim).filter(|l| !l.is_empty()).collect();
    let tail = tail[tail.len().saturating_sub(3)..].join("\n");
    match hint {
        Some(h) => format!("{h}\n（{tail}）"),
        None if tail.is_empty() => "pymobiledevice3 が異常終了しました".into(),
        None => tail,
    }
}

enum Kind {
    Hold,
    Walk { samples: Vec<Sample> },
}

struct Running {
    child: Child,
    log: PathBuf,
    kind: Kind,
}

pub struct IosSession {
    pmd3: PathBuf,
    udid: String,
    /// iOS 16 以下（DVT ではなく com.apple.dt.simulatelocation を使う）
    legacy: bool,
    dir: PathBuf,
    running: Option<Running>,
    route: Option<Route>,
    speed_mps: f64,
    traveled_m: f64,
    position: Option<LatLng>,
    bearing: f64,
    paused: bool,
    arrived: bool,
    error: Option<String>,
}

impl IosSession {
    pub fn new(pmd3: PathBuf, udid: &str, os_version: Option<&str>) -> Result<Self, String> {
        let dir = std::env::temp_dir().join("location-lab-ios").join(udid);
        fs::create_dir_all(&dir).map_err(|e| e.to_string())?;
        Ok(Self {
            pmd3,
            udid: udid.to_string(),
            legacy: os_version.map(major_version).is_some_and(|m| m > 0 && m < 17),
            dir,
            running: None,
            route: None,
            speed_mps: 1.4,
            traveled_m: 0.0,
            position: None,
            bearing: 0.0,
            paused: false,
            arrived: false,
            error: None,
        })
    }

    fn simulate_args(&self, sub: &str) -> Vec<String> {
        let mut args = vec!["developer".to_string()];
        if !self.legacy {
            args.push("dvt".into());
        }
        args.extend(["simulate-location".to_string(), sub.to_string(), "--udid".into(), self.udid.clone()]);
        args
    }

    fn run(&self, args: &[String], timeout: Duration) -> Result<String, String> {
        let refs: Vec<&str> = args.iter().map(String::as_str).collect();
        run_command(&self.pmd3, &refs, timeout).map_err(|e| explain_error(&format!("pymobiledevice3 {e}")))
    }

    /// 長時間動くプロセス（set の保持 / play の再生）を起動する。出力はログファイルに書く。
    fn spawn(&mut self, args: Vec<String>, kind: Kind) -> Result<(), String> {
        self.kill_running();
        self.kill_orphan();
        let log = self.dir.join(match kind {
            Kind::Hold => "hold.log",
            Kind::Walk { .. } => "walk.log",
        });
        let file = File::create(&log).map_err(|e| e.to_string())?;
        let child = Command::new(&self.pmd3)
            .args(&args)
            .env("NO_COLOR", "1")
            .env("PYTHONUNBUFFERED", "1")
            .stdin(Stdio::null())
            .stdout(Stdio::from(file.try_clone().map_err(|e| e.to_string())?))
            .stderr(Stdio::from(file))
            .spawn()
            .map_err(|e| format!("pymobiledevice3 を起動できません: {e}"))?;
        // アプリが強制終了されても次回の停止で片付けられるよう PID を残す
        let _ = fs::write(self.dir.join("pid"), child.id().to_string());
        self.running = Some(Running { child, log, kind });
        Ok(())
    }

    /// 前回アプリが強制終了した際に残った pymobiledevice3 プロセスを終了させる。
    fn kill_orphan(&self) {
        let pid_file = self.dir.join("pid");
        let Ok(pid) = fs::read_to_string(&pid_file) else { return };
        let pid = pid.trim();
        let is_ours = Command::new("ps")
            .args(["-p", pid, "-o", "command="])
            .output()
            .map(|o| {
                let cmd = String::from_utf8_lossy(&o.stdout);
                cmd.contains("simulate-location") && cmd.contains(&self.udid)
            })
            .unwrap_or(false);
        if is_ours {
            let _ = Command::new("kill").args(["-TERM", pid]).status();
        }
        let _ = fs::remove_file(pid_file);
    }

    /// 実行中プロセスを SIGTERM で終了させる（pymobiledevice3 が接続を閉じられるように）。
    fn kill_running(&mut self) {
        if let Some(mut r) = self.running.take() {
            if matches!(r.child.try_wait(), Ok(None)) {
                let _ = Command::new("kill").args(["-TERM", &r.child.id().to_string()]).status();
                let deadline = Instant::now() + Duration::from_secs(3);
                while matches!(r.child.try_wait(), Ok(None)) && Instant::now() < deadline {
                    std::thread::sleep(Duration::from_millis(50));
                }
                let _ = r.child.kill();
            }
            let _ = r.child.wait();
            let _ = fs::remove_file(self.dir.join("pid"));
        }
    }

    fn read_log(&self) -> String {
        self.running.as_ref().and_then(|r| fs::read_to_string(&r.log).ok()).unwrap_or_default()
    }

    /// 起動したプロセスが端末への設定に成功したことを確認できるまで待つ。
    fn wait_started(&mut self) -> Result<(), String> {
        let deadline = Instant::now() + START_TIMEOUT;
        loop {
            let log = self.read_log();
            let running = self.running.as_mut().ok_or("内部エラー: プロセスがありません")?;
            let exited = running.child.try_wait().map_err(|e| e.to_string())?;
            let confirmed = match &running.kind {
                // iOS 17+ の set は設定後にシグナル待ちの案内を出す。iOS 16 以下は設定後すぐ終了する
                Kind::Hold => log.contains("Press Ctrl+C") || (self.legacy && exited.is_some_and(|s| s.success())),
                Kind::Walk { .. } => parse_play_log(&log).0 > 0,
            };
            if confirmed {
                return Ok(());
            }
            if let Some(status) = exited {
                self.running = None;
                return Err(if log.trim().is_empty() {
                    format!("pymobiledevice3 が終了しました ({status})")
                } else {
                    explain_error(&log)
                });
            }
            if Instant::now() >= deadline {
                self.kill_running();
                return Err("端末からの応答がありません（タイムアウト）。端末のロックを解除し、接続を確認してください。".into());
            }
            std::thread::sleep(Duration::from_millis(200));
        }
    }

    fn fail<T>(&mut self, message: String) -> Result<T, String> {
        self.kill_running();
        self.error = Some(message.clone());
        Err(message)
    }

    pub fn hold(&mut self, point: LatLng) -> Result<Status, String> {
        self.route = None;
        self.paused = false;
        self.arrived = false;
        self.error = None;
        let mut args = self.simulate_args("set");
        args.extend(["--".into(), format!("{:.7}", point.lat), format!("{:.7}", point.lng)]);
        self.spawn(args, Kind::Hold)?;
        if let Err(e) = self.wait_started() {
            return self.fail(e);
        }
        self.position = Some(point);
        Ok(self.status())
    }

    pub fn walk(&mut self, waypoints: Vec<LatLng>, from: Option<LatLng>, speed_mps: f64) -> Result<Status, String> {
        self.refresh();
        let start = from.or(self.position).ok_or(
            "iOS では端末の実際の現在地を取得できないため、出発地を指定してください（「最初の点を出発地にする」）。",
        )?;
        let mut points = vec![start];
        points.extend(waypoints);
        self.route = Some(Route::new(points)?);
        self.speed_mps = speed_mps;
        self.traveled_m = 0.0;
        self.position = Some(start);
        self.paused = false;
        self.arrived = false;
        self.error = None;
        self.play_from_current()?;
        Ok(self.status())
    }

    fn play_from_current(&mut self) -> Result<(), String> {
        let route = self.route.as_ref().ok_or("経路がありません")?;
        let samples = sample_route(route, self.traveled_m, self.speed_mps);
        let gpx = self.dir.join("route.gpx");
        fs::write(&gpx, to_gpx(&samples)).map_err(|e| e.to_string())?;
        let mut args = self.simulate_args("play");
        args.push(gpx.to_string_lossy().into_owned());
        if self.legacy {
            args.push("0".into()); // iOS 16 以下の play は timing_randomness_range が必須の位置引数
        }
        self.spawn(args, Kind::Walk { samples })?;
        if let Err(e) = self.wait_started() {
            return self.fail(e);
        }
        self.refresh();
        Ok(())
    }

    pub fn pause(&mut self) -> Result<Status, String> {
        self.refresh();
        if self.route.is_none() || self.paused || self.arrived {
            return Ok(self.status());
        }
        let point = self.position.ok_or("現在位置が不明です")?;
        let mut args = self.simulate_args("set");
        args.extend(["--".into(), format!("{:.7}", point.lat), format!("{:.7}", point.lng)]);
        self.spawn(args, Kind::Hold)?;
        if let Err(e) = self.wait_started() {
            return self.fail(e);
        }
        self.paused = true;
        Ok(self.status())
    }

    pub fn resume(&mut self) -> Result<Status, String> {
        if self.route.is_some() && self.paused {
            self.paused = false;
            self.play_from_current()?;
        }
        Ok(self.status())
    }

    pub fn set_speed(&mut self, speed_mps: f64) -> Result<Status, String> {
        self.refresh();
        self.speed_mps = speed_mps;
        if self.route.is_some() && !self.paused && !self.arrived {
            self.play_from_current()?;
        }
        Ok(self.status())
    }

    /// シミュレーションを解除して実際の位置に戻す。解除を確認できなければエラー。
    pub fn stop(&mut self) -> Result<Status, String> {
        self.kill_running();
        self.kill_orphan();
        self.run(&self.simulate_args("clear"), START_TIMEOUT)?;
        self.route = None;
        self.position = None;
        self.paused = false;
        self.arrived = false;
        self.error = None;
        Ok(self.status())
    }

    pub fn is_active(&mut self) -> bool {
        self.refresh();
        self.position.is_some() || self.running.is_some()
    }

    /// 再生ログとプロセスの状態から現在位置を更新する。
    fn refresh(&mut self) {
        let log = self.read_log();
        let Some(running) = self.running.as_mut() else { return };
        if let Kind::Walk { samples } = &running.kind {
            let (count, _) = parse_play_log(&log);
            if count > 0 {
                let sample = &samples[(count - 1).min(samples.len() - 1)];
                self.position = Some(sample.point);
                self.bearing = sample.bearing;
                self.traveled_m = sample.traveled_m;
                self.arrived = count >= samples.len();
            }
        }
        if let Ok(Some(status)) = running.child.try_wait() {
            let finished_ok = status.success() && (self.legacy || self.arrived);
            self.running = None;
            if !finished_ok {
                self.error = Some(explain_error(&log));
                self.position = None;
                self.route = None;
            }
        }
    }

    pub fn status(&mut self) -> Status {
        self.refresh();
        let state = if self.error.is_some() {
            "ERROR"
        } else if self.position.is_none() {
            "IDLE"
        } else if self.route.is_none() {
            "HOLDING"
        } else if self.arrived {
            "ARRIVED"
        } else if self.paused {
            "PAUSED"
        } else {
            "WALKING"
        };
        let total = self.route.as_ref().map(Route::total_m).unwrap_or(0.0);
        let remaining = (total - self.traveled_m).max(0.0);
        let moving = state == "WALKING";
        Status {
            state: state.into(),
            position: self.position,
            bearing: self.bearing,
            speed_kmh: if moving { self.speed_mps * 3.6 } else { 0.0 },
            remaining_m: if self.route.is_some() { remaining } else { 0.0 },
            remaining_s: if self.route.is_some() { remaining / self.speed_mps } else { 0.0 },
            providers: vec![if self.legacy { "simulatelocation" } else { "dvt" }.into()],
            error: self.error.clone(),
        }
    }
}

impl Drop for IosSession {
    fn drop(&mut self) {
        self.kill_running();
    }
}

/// 端末一覧（pymobiledevice3 が見つからなければエラー）。
pub fn list_devices() -> Result<Vec<Device>, String> {
    let pmd3 = find_pmd3()?;
    let out = run_command(&pmd3, &["usbmux", "list"], Duration::from_secs(15))
        .map_err(|e| explain_error(&format!("pymobiledevice3 {e}")))?;
    parse_usbmux_list(&out)
}

/// 初回セットアップ: デベロッパモードの確認と、デベロッパディスクイメージのマウント。
pub fn setup(udid: &str) -> Result<String, String> {
    let pmd3 = find_pmd3()?;
    let run = |args: &[&str], secs| {
        let mut full = args.to_vec();
        full.extend_from_slice(&["--udid", udid]);
        run_command(&pmd3, &full, Duration::from_secs(secs)).map_err(|e| explain_error(&format!("pymobiledevice3 {e}")))
    };
    let mode = run(&["amfi", "developer-mode-status"], 30)?;
    if mode.lines().any(|l| l.trim().eq_ignore_ascii_case("false")) {
        let _ = run(&["amfi", "reveal-developer-mode"], 30);
        return Err(
            "デベロッパモードが OFF です。端末の「設定 → プライバシーとセキュリティ → デベロッパモード」を ON にして再起動し、もう一度セットアップを実行してください。".into(),
        );
    }
    run(&["mounter", "auto-mount"], 300)?;
    Ok("デベロッパモードを確認し、デベロッパディスクイメージをマウントしました。".into())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_usbmux_list() {
        let json = r#"[
          {"BuildVersion":"22A3354","ConnectionType":"USB","DeviceClass":"iPhone","DeviceName":"My iPhone",
           "Identifier":"00008110-000A","ProductType":"iPhone15,2","ProductVersion":"18.0","UniqueDeviceID":"00008110-000A"},
          {"ConnectionType":"Network","DeviceName":"My iPhone","Identifier":"00008110-000A","ProductVersion":"18.0","UniqueDeviceID":"00008110-000A"},
          {"ConnectionType":"USB","DeviceName":null,"Identifier":"abc","ProductType":"iPad8,1","ProductVersion":"16.7.2","UniqueDeviceID":"abc"}
        ]"#;
        let devices = parse_usbmux_list(json).unwrap();
        assert_eq!(devices.len(), 2);
        assert_eq!(devices[0].serial, "00008110-000A");
        assert_eq!(devices[0].model.as_deref(), Some("My iPhone"));
        assert_eq!(devices[0].platform, "ios");
        assert_eq!(devices[1].model.as_deref(), Some("iPad8,1"));
        assert_eq!(parse_usbmux_list("[]").unwrap(), vec![]);
        assert!(parse_usbmux_list("oops").is_err());
    }

    #[test]
    fn versions() {
        assert_eq!(major_version("17.4.1"), 17);
        assert_eq!(major_version("16.7"), 16);
        assert_eq!(major_version(""), 0);
    }

    #[test]
    fn parses_play_log() {
        let log = "2026-10-10 15:26:30 host LocationSimulation[1] INFO set location to 35.1 139.2\n\
                   2026-10-10 15:26:31 host LocationSimulation[1] INFO waiting for 1.000s\n\
                   2026-10-10 15:26:31 host LocationSimulation[1] INFO set location to 35.10001 139.20002\n";
        let (count, last) = parse_play_log(log);
        assert_eq!(count, 2);
        assert_eq!(last, Some(LatLng { lat: 35.10001, lng: 139.20002 }));
        assert_eq!(parse_play_log(""), (0, None));
    }

    /// pymobiledevice3 の出力形式を模したフェイク（sh スクリプト）でセッションの流れを確認する。
    fn fake_pmd3(dir: &std::path::Path, body: &str) -> PathBuf {
        use std::os::unix::fs::PermissionsExt;
        let path = dir.join("fake-pmd3");
        fs::write(&path, format!("#!/bin/sh\necho \"$@\" >> \"{}/calls.log\"\n{body}\n", dir.display())).unwrap();
        fs::set_permissions(&path, fs::Permissions::from_mode(0o755)).unwrap();
        path
    }

    fn temp_dir(name: &str) -> PathBuf {
        let dir = std::env::temp_dir().join(format!("location-lab-test-{name}-{}", std::process::id()));
        let _ = fs::remove_dir_all(&dir);
        fs::create_dir_all(&dir).unwrap();
        dir
    }

    #[test]
    fn session_hold_walk_and_stop_with_fake_pmd3() {
        let dir = temp_dir("ok");
        let pmd3 = fake_pmd3(
            &dir,
            r#"case "$*" in
  *"simulate-location set"*) echo "Press Ctrl+C to send a SIGINT"; trap 'exit 0' TERM; while :; do sleep 0.05; done ;;
  *"simulate-location play"*)
    for last; do :; done
    grep -o 'lat="[^"]*" lon="[^"]*"' "$last" | head -3 | sed 's/lat="\([^"]*\)" lon="\([^"]*\)"/INFO set location to  /'
    trap 'exit 0' TERM; while :; do sleep 0.05; done ;;
  *"simulate-location clear"*) exit 0 ;;
esac"#,
        );
        let mut s = IosSession::new(pmd3, "UDID-1", Some("18.0")).unwrap();
        let start = LatLng { lat: 35.0, lng: 139.0 };

        let st = s.hold(start).unwrap();
        assert_eq!(st.state, "HOLDING");
        assert_eq!(st.position, Some(start));

        // 約 111 m を 1.4 m/s で → フェイクは 3 点目まで再生した状態
        let st = s.walk(vec![LatLng { lat: 35.001, lng: 139.0 }], None, 1.4).unwrap();
        assert_eq!(st.state, "WALKING");
        assert!((st.remaining_m - (111.2 - 2.0 * 1.4)).abs() < 1.0, "{}", st.remaining_m);

        let st = s.stop().unwrap();
        assert_eq!(st.state, "IDLE");
        let calls = fs::read_to_string(dir.join("calls.log")).unwrap();
        assert!(calls.contains("developer dvt simulate-location set --udid UDID-1 -- 35.0000000 139.0000000"));
        assert!(calls.contains("developer dvt simulate-location play --udid UDID-1"));
        assert!(calls.contains("developer dvt simulate-location clear --udid UDID-1"));
        let _ = fs::remove_dir_all(&dir);
    }

    #[test]
    fn stop_cleans_up_orphan_from_previous_run() {
        let dir = temp_dir("orphan");
        let pmd3 = fake_pmd3(
            &dir,
            r#"case "$*" in
  *"simulate-location set"*) echo "Press Ctrl+C"; trap 'exit 0' TERM; while :; do sleep 0.05; done ;;
esac"#,
        );
        let mut first = IosSession::new(pmd3.clone(), "UDID-ORPHAN", Some("18.0")).unwrap();
        first.hold(LatLng { lat: 35.0, lng: 139.0 }).unwrap();
        let pid = first.running.as_ref().unwrap().child.id();
        // アプリの強制終了を模して、プロセスを残したままセッションを捨てる
        std::mem::forget(first.running.take());
        drop(first);

        let mut second = IosSession::new(pmd3, "UDID-ORPHAN", Some("18.0")).unwrap();
        second.stop().unwrap();
        std::thread::sleep(Duration::from_millis(300));
        // テストプロセスが親なので終了後はゾンビ (Z) として残る。それ以外なら生存とみなす
        let stat = Command::new("ps").args(["-p", &pid.to_string(), "-o", "stat="]).output().unwrap();
        let stat = String::from_utf8_lossy(&stat.stdout).trim().to_string();
        let alive = !stat.is_empty() && !stat.starts_with('Z');
        assert!(!alive, "orphan process {pid} is still running");
        let _ = fs::remove_dir_all(&dir);
    }

    #[test]
    fn session_reports_errors_and_legacy_commands() {
        let dir = temp_dir("err");
        let pmd3 = fake_pmd3(&dir, r#"echo "pymobiledevice3.exceptions.DeveloperModeIsNotEnabledError" >&2; exit 1"#);
        let mut s = IosSession::new(pmd3, "UDID-2", Some("16.7")).unwrap();
        let err = s.hold(LatLng { lat: 35.0, lng: 139.0 }).unwrap_err();
        assert!(err.contains("デベロッパモード"), "{err}");
        assert_eq!(s.status().state, "ERROR");
        // iOS 16 以下は DVT ではないコマンドを使う
        let calls = fs::read_to_string(dir.join("calls.log")).unwrap();
        assert!(calls.contains("developer simulate-location set --udid UDID-2"));
        // iOS では実際の現在地が取れないので、出発地なしの移動は拒否する
        assert!(s.walk(vec![LatLng { lat: 35.0, lng: 139.0 }], None, 1.4).is_err());
        let _ = fs::remove_dir_all(&dir);
    }

    #[test]
    fn explains_errors() {
        assert!(explain_error("pymobiledevice3.exceptions.DeveloperModeIsNotEnabledError").contains("デベロッパモード"));
        assert!(explain_error("PasswordRequiredError: device is locked").contains("ロックを解除"));
        assert!(explain_error("NoDeviceConnectedError").contains("接続されていません"));
        assert_eq!(explain_error("something\nelse"), "something\nelse");
    }
}

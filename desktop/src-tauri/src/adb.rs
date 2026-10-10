//! adb 経由で端末側補助アプリ（dev.locationlab.helper）を操作する。
//! プロトコルは mac/locationlab.py と同じ（am start-foreground-service + dumpsys）。

use serde::Serialize;
use std::collections::BTreeMap;
use std::io::Read;
use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};
use std::time::{Duration, Instant};

pub const PACKAGE: &str = "dev.locationlab.helper";
pub const SERVICE: &str = "dev.locationlab.helper/.MockLocationService";
const ACTION_PREFIX: &str = "dev.locationlab.action.";

pub const MIN_SPEED_MPS: f64 = 0.1;
pub const MAX_SPEED_MPS: f64 = 50.0;
/// これを超える瞬間移動は明示的な許可がない限り拒否する。
pub const MAX_JUMP_M: f64 = 100.0;
const EARTH_RADIUS_M: f64 = 6_371_008.8;

#[derive(Debug, Clone, Copy, PartialEq, serde::Deserialize, Serialize)]
pub struct LatLng {
    pub lat: f64,
    pub lng: f64,
}

impl LatLng {
    pub fn new(lat: f64, lng: f64) -> Result<Self, String> {
        if !(lat.is_finite() && (-90.0..=90.0).contains(&lat)) {
            return Err(format!("緯度が範囲外です: {lat}"));
        }
        if !(lng.is_finite() && (-180.0..=180.0).contains(&lng)) {
            return Err(format!("経度が範囲外です: {lng}"));
        }
        Ok(Self { lat, lng })
    }

    pub fn validated(self) -> Result<Self, String> {
        Self::new(self.lat, self.lng)
    }

    pub fn parse(text: &str) -> Result<Self, String> {
        let parts: Vec<&str> = text.split(',').map(str::trim).collect();
        if parts.len() != 2 {
            return Err(format!("'緯度,経度' 形式ではありません: {text}"));
        }
        let lat = parts[0].parse::<f64>().map_err(|_| format!("緯度が不正です: {}", parts[0]))?;
        let lng = parts[1].parse::<f64>().map_err(|_| format!("経度が不正です: {}", parts[1]))?;
        Self::new(lat, lng)
    }

    pub fn format(&self) -> String {
        format!("{:.7},{:.7}", self.lat, self.lng)
    }
}

pub fn distance_m(a: LatLng, b: LatLng) -> f64 {
    let (lat1, lat2) = (a.lat.to_radians(), b.lat.to_radians());
    let dlat = lat2 - lat1;
    let dlng = (b.lng - a.lng).to_radians();
    let h = (dlat / 2.0).sin().powi(2) + lat1.cos() * lat2.cos() * (dlng / 2.0).sin().powi(2);
    2.0 * EARTH_RADIUS_M * h.clamp(0.0, 1.0).sqrt().asin()
}

pub fn kmh_to_mps(kmh: f64) -> Result<f64, String> {
    let mps = kmh / 3.6;
    if !(mps.is_finite() && (MIN_SPEED_MPS..=MAX_SPEED_MPS).contains(&mps)) {
        return Err(format!(
            "速度は {:.1}〜{:.0} km/h で指定してください: {kmh}",
            MIN_SPEED_MPS * 3.6,
            MAX_SPEED_MPS * 3.6
        ));
    }
    Ok(mps)
}

/// dumpsys の出力から key=value 行を取り出す。
pub fn parse_status(text: &str) -> BTreeMap<String, String> {
    let mut map = BTreeMap::new();
    for line in text.lines() {
        if let Some((k, v)) = line.trim().split_once('=') {
            if !k.is_empty() && k.chars().all(|c| c.is_ascii_alphanumeric() || c == '_') {
                map.insert(k.to_string(), v.to_string());
            }
        }
    }
    map
}

#[derive(Debug, Clone, Serialize, Default, PartialEq)]
pub struct Status {
    pub state: String,
    pub position: Option<LatLng>,
    pub bearing: f64,
    pub speed_kmh: f64,
    pub remaining_m: f64,
    pub remaining_s: f64,
    pub providers: Vec<String>,
    pub error: Option<String>,
}

impl Status {
    pub fn from_dumpsys(text: &str) -> Self {
        let map = parse_status(text);
        let num = |k: &str| map.get(k).and_then(|v| v.parse::<f64>().ok()).unwrap_or(0.0);
        Status {
            state: map.get("state").cloned().unwrap_or_else(|| "NOT_RUNNING".into()),
            position: map.get("position").and_then(|p| LatLng::parse(p).ok()),
            bearing: num("bearing"),
            speed_kmh: num("speed_mps") * 3.6,
            remaining_m: num("remaining_m"),
            remaining_s: num("remaining_s"),
            providers: map
                .get("providers")
                .map(|p| p.split(',').filter(|s| !s.is_empty()).map(String::from).collect())
                .unwrap_or_default(),
            error: map.get("error").cloned(),
        }
    }

    pub fn is_active(&self) -> bool {
        !matches!(self.state.as_str(), "NOT_RUNNING" | "IDLE")
    }
}

#[derive(Debug, Clone, Serialize, PartialEq)]
pub struct Device {
    pub serial: String,
    pub state: String,
    pub model: Option<String>,
    /// "android" | "ios"
    pub platform: String,
    pub os_version: Option<String>,
}

/// `adb devices -l` の出力をパースする。
pub fn parse_devices(text: &str) -> Vec<Device> {
    text.lines()
        .skip_while(|l| !l.starts_with("List of devices"))
        .skip(1)
        .filter_map(|line| {
            let mut it = line.split_whitespace();
            let serial = it.next()?.to_string();
            let state = it.next()?.to_string();
            let model = it.find_map(|kv| kv.strip_prefix("model:").map(|m| m.replace('_', " ")));
            Some(Device { serial, state, model, platform: "android".into(), os_version: None })
        })
        .collect()
}

/// ワイヤレスデバッグの接続先 "host:port" を検証して正規化する。
pub fn parse_host_port(text: &str) -> Result<String, String> {
    let text = text.trim();
    let (host, port) = text
        .rsplit_once(':')
        .ok_or_else(|| format!("「IP アドレス:ポート」の形式で入力してください: {text}"))?;
    let valid_host = !host.is_empty()
        && host.len() <= 253
        && host.chars().all(|c| c.is_ascii_alphanumeric() || c == '.' || c == '-');
    if !valid_host {
        return Err(format!("IP アドレスが不正です: {host}"));
    }
    match port.parse::<u16>() {
        Ok(p) if p > 0 => Ok(format!("{host}:{p}")),
        _ => Err(format!("ポート番号が不正です: {port}")),
    }
}

/// ペア設定コード（6 桁の数字）を検証する。
pub fn parse_pairing_code(text: &str) -> Result<String, String> {
    let code = text.trim();
    if code.len() == 6 && code.chars().all(|c| c.is_ascii_digit()) {
        Ok(code.to_string())
    } else {
        Err("ペア設定コードは 6 桁の数字です".into())
    }
}

/// `adb connect` は失敗しても終了コード 0 を返すことがあるため、出力で判定する。
pub fn connect_succeeded(output: &str) -> bool {
    let out = output.to_ascii_lowercase();
    (out.contains("connected to") || out.contains("already connected"))
        && !out.contains("failed")
        && !out.contains("cannot")
        && !out.contains("unable")
}

/// GUI アプリは Finder から起動するとシェルの PATH を引き継がないため、よくある場所も探す。
pub fn find_adb() -> Result<PathBuf, String> {
    if let Ok(p) = std::env::var("LOCATIONLAB_ADB") {
        return Ok(PathBuf::from(p));
    }
    let mut candidates: Vec<PathBuf> = std::env::var_os("PATH")
        .map(|p| std::env::split_paths(&p).map(|d| d.join("adb")).collect())
        .unwrap_or_default();
    for var in ["ANDROID_HOME", "ANDROID_SDK_ROOT"] {
        if let Some(sdk) = std::env::var_os(var) {
            candidates.push(PathBuf::from(sdk).join("platform-tools/adb"));
        }
    }
    if let Some(home) = std::env::var_os("HOME") {
        candidates.push(PathBuf::from(home).join("Library/Android/sdk/platform-tools/adb"));
    }
    candidates.push("/opt/homebrew/bin/adb".into());
    candidates.push("/usr/local/bin/adb".into());
    candidates
        .into_iter()
        .find(|p| p.is_file())
        .ok_or_else(|| {
            "adb が見つかりません。`brew install --cask android-platform-tools` でインストールするか、環境変数 LOCATIONLAB_ADB に adb のパスを設定してください。".into()
        })
}

/// 外部コマンドをタイムアウト付きで実行し、stdout+stderr を返す。失敗時は出力をエラーにする。
pub fn run_command(exe: &Path, args: &[&str], timeout: Duration) -> Result<String, String> {
    let mut child = Command::new(exe)
        .args(args)
        .stdin(Stdio::null())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .map_err(|e| format!("を起動できません: {e}"))?;
    // 出力が多いとパイプが詰まるので別スレッドで読む
    let mut stdout = child.stdout.take().unwrap();
    let mut stderr = child.stderr.take().unwrap();
    let out_t = std::thread::spawn(move || {
        let mut s = String::new();
        let _ = stdout.read_to_string(&mut s);
        s
    });
    let err_t = std::thread::spawn(move || {
        let mut s = String::new();
        let _ = stderr.read_to_string(&mut s);
        s
    });
    let deadline = Instant::now() + timeout;
    let status = loop {
        if let Some(st) = child.try_wait().map_err(|e| e.to_string())? {
            break st;
        }
        if Instant::now() >= deadline {
            let _ = child.kill();
            let _ = child.wait();
            return Err(format!("がタイムアウトしました: {}", args.join(" ")));
        }
        std::thread::sleep(Duration::from_millis(20));
    };
    let out = format!("{}{}", out_t.join().unwrap_or_default(), err_t.join().unwrap_or_default());
    let out = out.trim().to_string();
    if !status.success() {
        return Err(format!("エラー: {out}"));
    }
    Ok(out)
}

pub struct Adb {
    exe: PathBuf,
    serial: Option<String>,
}

impl Adb {
    pub fn new(serial: Option<String>) -> Result<Self, String> {
        Ok(Self { exe: find_adb()?, serial })
    }

    pub fn run(&self, args: &[&str], timeout: Duration) -> Result<String, String> {
        let mut full: Vec<&str> = Vec::new();
        if let Some(s) = &self.serial {
            full.extend_from_slice(&["-s", s]);
        }
        full.extend_from_slice(args);
        run_command(&self.exe, &full, timeout).map_err(|e| format!("adb {e}"))
    }

    pub fn shell(&self, args: &[&str]) -> Result<String, String> {
        let mut full = vec!["shell"];
        full.extend_from_slice(args);
        self.run(&full, Duration::from_secs(20))
    }

    pub fn devices(&self) -> Result<Vec<Device>, String> {
        Ok(parse_devices(&self.run(&["devices", "-l"], Duration::from_secs(10))?))
    }

    pub fn send(&self, action: &str, extras: &[(&str, String)]) -> Result<(), String> {
        let full_action = format!("{ACTION_PREFIX}{action}");
        let mut args: Vec<&str> = vec!["am", "start-foreground-service", "-n", SERVICE, "-a", &full_action];
        for (k, v) in extras {
            args.extend_from_slice(&["--es", k, v]);
        }
        let out = self.shell(&args)?;
        if out.contains("Error") || out.contains("Exception") {
            return Err(if out.contains("does not exist") || out.contains("not found") {
                "端末に Location Lab アプリがインストールされていません（「端末セットアップ」を実行してください）".into()
            } else {
                out
            });
        }
        Ok(())
    }

    pub fn status(&self) -> Result<Status, String> {
        Ok(Status::from_dumpsys(&self.shell(&["dumpsys", "activity", "service", SERVICE])?))
    }

    /// コマンド送信後、期待する状態になるまで待つ。ERROR ならその内容をエラーとして返す。
    pub fn wait_for(&self, expected: &[&str], timeout: Duration) -> Result<Status, String> {
        let deadline = Instant::now() + timeout;
        std::thread::sleep(Duration::from_millis(300)); // 送信は非同期なので前の状態を読まないよう待つ
        loop {
            let st = self.status()?;
            if st.state == "ERROR" {
                return Err(st.error.unwrap_or_else(|| "端末側でエラーが発生しました".into()));
            }
            if expected.contains(&st.state.as_str()) {
                return Ok(st);
            }
            if Instant::now() >= deadline {
                return Err(format!("状態が {expected:?} になりませんでした（現在: {}）", st.state));
            }
            std::thread::sleep(Duration::from_millis(300));
        }
    }

    pub fn pair(&self, host: &str, code: &str) -> Result<String, String> {
        let out = self.run(&["pair", host, code], Duration::from_secs(20))?;
        if out.contains("Successfully paired") {
            Ok(out)
        } else {
            Err(format!("ペア設定に失敗しました: {out}"))
        }
    }

    pub fn connect(&self, host: &str) -> Result<String, String> {
        let out = self.run(&["connect", host], Duration::from_secs(15))?;
        if connect_succeeded(&out) {
            Ok(out)
        } else {
            Err(format!("接続できませんでした: {out}"))
        }
    }

    pub fn disconnect(&self, host: &str) -> Result<String, String> {
        self.run(&["disconnect", host], Duration::from_secs(10))
    }

    pub fn install(&self, apk: &str) -> Result<String, String> {
        self.run(&["install", "-r", apk], Duration::from_secs(180))
    }

    pub fn grant_permissions(&self) -> Result<(), String> {
        for perm in ["android.permission.ACCESS_FINE_LOCATION", "android.permission.ACCESS_COARSE_LOCATION"] {
            self.shell(&["pm", "grant", PACKAGE, perm])?;
        }
        self.shell(&["appops", "set", PACKAGE, "android:mock_location", "allow"])?;
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn validates_coordinates() {
        assert!(LatLng::new(35.0, 139.0).is_ok());
        assert!(LatLng::new(90.1, 0.0).is_err());
        assert!(LatLng::new(0.0, -180.1).is_err());
        assert!(LatLng::new(f64::NAN, 0.0).is_err());
        assert_eq!(LatLng::parse(" 35.5 , 139.25 ").unwrap(), LatLng { lat: 35.5, lng: 139.25 });
        assert!(LatLng::parse("35.5").is_err());
        assert!(LatLng::parse("a,1").is_err());
    }

    #[test]
    fn distance_tokyo_shinjuku() {
        let d = distance_m(LatLng { lat: 35.681236, lng: 139.767125 }, LatLng { lat: 35.690921, lng: 139.700258 });
        assert!((d - 6130.0).abs() < 50.0, "{d}");
    }

    #[test]
    fn speed_range() {
        assert!((kmh_to_mps(3.6).unwrap() - 1.0).abs() < 1e-9);
        assert!(kmh_to_mps(0.0).is_err());
        assert!(kmh_to_mps(200.0).is_err());
        assert!(kmh_to_mps(f64::INFINITY).is_err());
    }

    #[test]
    fn parses_dumpsys_status() {
        let text = "SERVICE dev.locationlab.helper/.MockLocationService 1a2b pid=123\n  state=WALKING\n  position=35.6812360,139.7671250\n  bearing=90.0\n  speed_mps=1.40\n  remaining_m=120.5\n  remaining_s=86\n  providers=gps,network\n";
        let s = Status::from_dumpsys(text);
        assert_eq!(s.state, "WALKING");
        assert_eq!(s.position, Some(LatLng { lat: 35.681236, lng: 139.767125 }));
        assert!((s.speed_kmh - 5.04).abs() < 1e-9);
        assert_eq!(s.providers, vec!["gps", "network"]);
        assert!(s.is_active());

        let empty = Status::from_dumpsys("");
        assert_eq!(empty.state, "NOT_RUNNING");
        assert!(!empty.is_active());

        let err = Status::from_dumpsys("state=ERROR\nerror=not selected=x\n");
        assert_eq!(err.error.as_deref(), Some("not selected=x"));
    }

    #[test]
    fn validates_wireless_inputs() {
        assert_eq!(parse_host_port(" 192.168.1.20:37099 ").unwrap(), "192.168.1.20:37099");
        assert_eq!(parse_host_port("pixel.local:5555").unwrap(), "pixel.local:5555");
        assert!(parse_host_port("192.168.1.20").is_err());
        assert!(parse_host_port("192.168.1.20:0").is_err());
        assert!(parse_host_port("192.168.1.20:70000").is_err());
        assert!(parse_host_port(":5555").is_err());
        assert!(parse_host_port("1.2.3.4;rm -rf:5555").is_err());

        assert_eq!(parse_pairing_code(" 123456 ").unwrap(), "123456");
        assert!(parse_pairing_code("12345").is_err());
        assert!(parse_pairing_code("12a456").is_err());
    }

    #[test]
    fn interprets_connect_output() {
        assert!(connect_succeeded("connected to 192.168.1.20:37099"));
        assert!(connect_succeeded("already connected to 192.168.1.20:37099"));
        assert!(!connect_succeeded("failed to connect to '192.168.1.20:37099': Connection refused"));
        assert!(!connect_succeeded("cannot connect to 192.168.1.20:37099: No route to host"));
    }

    #[test]
    fn parses_device_list() {
        let text = "* daemon started successfully\nList of devices attached\nemulator-5554          device product:sdk_gphone64_arm64 model:sdk_gphone64_arm64 device:emu64a transport_id:1\nR5CT1234   unauthorized usb:1-1 transport_id:2\n\n";
        let devices = parse_devices(text);
        assert_eq!(devices.len(), 2);
        assert_eq!(devices[0].serial, "emulator-5554");
        assert_eq!(devices[0].model.as_deref(), Some("sdk gphone64 arm64"));
        assert_eq!(devices[1].state, "unauthorized");
        assert_eq!(devices[1].model, None);
    }
}

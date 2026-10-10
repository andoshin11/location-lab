import { formatDistance, formatDuration, parseGpx, parseLatLng, routeLengthM } from "./geo.js";

const { invoke } = window.__TAURI__.core;
const $ = (id) => document.getElementById(id);

const DEFAULT_CENTER = [35.681236, 139.767125];
const POLL_MS = 1000;
const POLL_HIDDEN_MS = 5000; // ウィンドウ非表示中は負荷を下げる

const state = {
  serial: null,
  devices: [],
  status: null,
  waypoints: [],
  centeredOnce: false,
  busy: false,
};

// ---- 地図 ----

const map = L.map("map", { zoomControl: true }).setView(DEFAULT_CENTER, 15);
L.tileLayer("https://tile.openstreetmap.org/{z}/{x}/{y}.png", {
  maxZoom: 19,
  attribution: '&copy; <a href="https://www.openstreetmap.org/copyright">OpenStreetMap</a> contributors',
}).addTo(map);

const simMarker = L.marker(DEFAULT_CENTER, {
  icon: L.divIcon({ className: "", html: '<div class="sim-marker"></div>', iconSize: [18, 18], iconAnchor: [9, 9] }),
  zIndexOffset: 1000,
});
const routeLine = L.polyline([], { color: "#2463eb", weight: 4, opacity: 0.7, dashArray: "8 6" }).addTo(map);
const waypointLayer = L.layerGroup().addTo(map);

map.on("click", (e) => {
  addWaypoint({ lat: e.latlng.lat, lng: wrapLng(e.latlng.lng) });
});

function wrapLng(lng) {
  return ((((lng + 180) % 360) + 360) % 360) - 180;
}

// ---- 経路 ----

function addWaypoint(p) {
  state.waypoints.push(p);
  renderRoute();
}

function routeStart() {
  if ($("first-is-start").checked) return state.waypoints[0] ?? null;
  return state.status?.position ?? null;
}

function routeTargets() {
  return $("first-is-start").checked ? state.waypoints.slice(1) : state.waypoints;
}

function renderRoute() {
  const list = $("waypoints");
  list.replaceChildren();
  waypointLayer.clearLayers();
  const firstIsStart = $("first-is-start").checked;

  state.waypoints.forEach((p, i) => {
    const li = document.createElement("li");
    if (firstIsStart && i === 0) {
      const s = document.createElement("span");
      s.className = "start-label";
      s.textContent = "出発 ";
      li.append(s);
    }
    li.append(`${p.lat.toFixed(6)}, ${p.lng.toFixed(6)}`);
    const del = document.createElement("button");
    del.textContent = "×";
    del.title = "削除";
    del.onclick = () => {
      state.waypoints.splice(i, 1);
      renderRoute();
    };
    li.append(del);
    list.append(li);

    L.circleMarker([p.lat, p.lng], {
      radius: 6,
      color: firstIsStart && i === 0 ? "#1f8a4c" : "#2463eb",
      fillOpacity: 0.9,
    })
      .bindTooltip(firstIsStart && i === 0 ? "出発" : `${firstIsStart ? i : i + 1}`)
      .addTo(waypointLayer);
  });

  const start = routeStart();
  const points = [...(start ? [start] : []), ...routeTargets()];
  routeLine.setLatLngs(points.map((p) => [p.lat, p.lng]));
  updateSummary();
  updateButtons();
}

function updateSummary() {
  const start = routeStart();
  const targets = routeTargets();
  if (targets.length === 0) {
    $("route-summary").textContent = "目的地が未指定です";
    return;
  }
  if (!start) {
    $("route-summary").textContent = isIos()
      ? "iOS では出発地の指定が必要です（「最初の点を出発地にする」にチェック）"
      : "出発地: 端末の実際の位置（距離は開始後に表示）";
    return;
  }
  const length = routeLengthM([start, ...targets]);
  const kmh = Number($("speed").value);
  const eta = kmh > 0 ? formatDuration(length / (kmh / 3.6)) : "–";
  $("route-summary").textContent = `距離 ${formatDistance(length)} ／ ${kmh} km/h で ${eta}`;
}

$("add-point-btn").onclick = () => {
  try {
    addWaypoint(parseLatLng($("add-point").value));
    $("add-point").value = "";
    const p = state.waypoints.at(-1);
    map.panTo([p.lat, p.lng]);
  } catch (e) {
    showMessage(e.message, true);
  }
};
$("add-point").addEventListener("keydown", (e) => e.key === "Enter" && $("add-point-btn").click());
$("first-is-start").onchange = renderRoute;
$("clear-route").onclick = () => {
  state.waypoints = [];
  renderRoute();
};
$("gpx").onchange = async (e) => {
  const file = e.target.files[0];
  e.target.value = "";
  if (!file) return;
  try {
    state.waypoints = parseGpx(await file.text());
    renderRoute();
    map.fitBounds(routeLine.getBounds(), { padding: [40, 40] });
    showMessage(`GPX から ${state.waypoints.length} 点を読み込みました`);
  } catch (err) {
    showMessage(err.message, true);
  }
};

// ---- 速度 ----

function speedKmh() {
  const v = Number($("speed").value);
  if (!Number.isFinite(v) || v <= 0) throw new Error("速度を正しく入力してください");
  return v;
}

$("speed").oninput = updateSummary;
document.querySelectorAll("[data-speed]").forEach((b) => {
  b.onclick = () => {
    $("speed").value = b.dataset.speed;
    updateSummary();
  };
});
$("apply-speed").onclick = () => run(() => invoke("set_speed", { ...target(), speedKmh: speedKmh() }), "速度を変更しました");

// ---- 操作 ----

$("walk").onclick = () =>
  run(async () => {
    const targets = routeTargets();
    if (targets.length === 0) throw new Error("目的地を地図上で指定してください");
    const from = $("first-is-start").checked ? state.waypoints[0] : null;
    const status = await invoke("start_walk", { ...target(), waypoints: targets, from, speedKmh: speedKmh() });
    applyStatus(status);
  }, "移動を開始しました");

$("pause").onclick = () => run(async () => applyStatus(await invoke("pause_walk", target())), "一時停止しました");
$("resume").onclick = () => run(async () => applyStatus(await invoke("resume_walk", target())), "再開しました");

$("hold").onclick = () =>
  run(async () => {
    const point = state.waypoints.at(-1);
    if (!point) throw new Error("固定する地点を地図上で指定してください");
    try {
      applyStatus(await invoke("set_position", { ...target(), point, force: false }));
    } catch (e) {
      const m = String(e).match(/^JUMP:(\d+)/);
      if (!m) throw e;
      const ok = await confirmDialog(
        `現在のシミュレーション位置から ${formatDistance(Number(m[1]))} 瞬間移動します。` +
          "徐々に移動するには「徒歩で移動開始」を使ってください。それでも固定しますか？",
      );
      if (!ok) return "cancel";
      applyStatus(await invoke("set_position", { ...target(), point, force: true }));
    }
  }, "指定地点に固定しました");

$("stop").onclick = () =>
  run(async () => applyStatus(await invoke("stop_simulation", target())), "停止しました。端末は通常の位置情報に戻りました。");

$("setup-device").onclick = () =>
  run(async () => {
    const log = await invoke("setup_device", { ...target(), apk: $("apk-path").value || null });
    showMessage(log);
    return "keep-message";
  });

$("stop-on-exit").onchange = syncSession;

// ---- ワイヤレス接続 ----

const LAST_HOST_KEY = "locationlab.lastWirelessHost";

function isWirelessSerial(serial) {
  return /^[\w.-]+:\d+$/.test(serial ?? "");
}

function rememberHost(host) {
  try {
    localStorage.setItem(LAST_HOST_KEY, host);
  } catch {
    // 保存できなくても動作に影響しない
  }
}

try {
  $("connect-host").value = localStorage.getItem(LAST_HOST_KEY) ?? "";
} catch {
  // 同上
}

$("pair-btn").onclick = () =>
  run(async () => {
    const out = await invoke("pair_device", { host: $("pair-host").value, code: $("pair-code").value });
    $("pair-code").value = "";
    // ペア設定の IP を接続欄に流用（ポートは別なので入力を促す）
    const ip = $("pair-host").value.trim().split(":")[0];
    if (ip && !$("connect-host").value.startsWith(ip + ":")) $("connect-host").value = ip + ":";
    showMessage(`${out}\n続けて「2. 接続」で接続用のポートを入力してください。`);
    $("connect-host").focus();
    return "keep-message";
  });

$("connect-btn").onclick = () =>
  run(async () => {
    const serial = await invoke("connect_device", { host: $("connect-host").value });
    rememberHost(serial);
    state.serial = serial;
    state.centeredOnce = false;
    // 接続直後は offline のことがあるので少し待ってから一覧を取り直す
    for (let i = 0; i < 5; i++) {
      await new Promise((r) => setTimeout(r, 600));
      const { devices } = await invoke("list_devices");
      if (devices.some((d) => d.serial === serial && d.state === "device")) break;
    }
    await refreshDevices();
    if (state.serial !== serial) throw new Error(`${serial} に接続しましたが、端末が応答しません。端末の画面を確認してください。`);
  }, "ワイヤレスで接続しました");

$("disconnect-btn").onclick = () =>
  run(async () => {
    await invoke("disconnect_device", { host: state.serial });
    await refreshDevices();
  }, "切断しました。端末上のシミュレーションは継続しています（停止するには先に「停止して通常位置に戻す」）。");

$("pair-code").addEventListener("keydown", (e) => e.key === "Enter" && $("pair-btn").click());
$("connect-host").addEventListener("keydown", (e) => e.key === "Enter" && $("connect-btn").click());

async function run(fn, successMessage) {
  if (state.busy) return;
  state.busy = true;
  updateButtons();
  // iOS はトンネル確立などで数十秒かかることがある
  if (isIos()) showMessage("iPhone / iPad と通信しています…");
  try {
    const result = await fn();
    if (result === "cancel" || result === "keep-message") return;
    if (successMessage) showMessage(successMessage);
  } catch (e) {
    showMessage(String(e.message ?? e), true);
  } finally {
    state.busy = false;
    updateButtons();
  }
}

function updateButtons() {
  const connected = !!state.serial && !state.busy;
  const s = state.status?.state;
  const has = routeTargets().length > 0;
  // iOS は実際の現在地を取得できないので、出発地（現在のシミュレーション位置か指定）が必要
  $("walk").disabled = !connected || !has || (isIos() && !routeStart());
  $("hold").disabled = !connected || state.waypoints.length === 0;
  $("pause").disabled = !connected || s !== "WALKING";
  $("resume").disabled = !connected || s !== "PAUSED";
  $("apply-speed").disabled = !connected || !["WALKING", "PAUSED"].includes(s);
  $("stop").disabled = !connected || !s || s === "NOT_RUNNING" || s === "IDLE";
  $("setup-device").disabled = !connected;
  $("disconnect-btn").hidden = !isWirelessSerial(state.serial);
  $("disconnect-btn").disabled = state.busy;
  $("pair-btn").disabled = state.busy;
  $("connect-btn").disabled = state.busy;
}

/** WebView 標準の confirm() に頼らないアプリ内確認ダイアログ。 */
function confirmDialog(text) {
  const dialog = $("confirm");
  $("confirm-text").textContent = text;
  dialog.showModal();
  return new Promise((resolve) => {
    dialog.addEventListener("close", () => resolve(dialog.returnValue === "ok"), { once: true });
  });
}

function showMessage(text, isError = false) {
  const el = $("message");
  el.hidden = false;
  el.textContent = text;
  el.classList.toggle("error", isError);
}

// ---- 端末と状態 ----

function currentDevice() {
  return state.devices.find((d) => d.serial === state.serial) ?? null;
}

function currentPlatform() {
  return currentDevice()?.platform ?? null;
}

function isIos() {
  return currentPlatform() === "ios";
}

/** コマンドの対象端末（バックエンドは platform で Android / iOS を切り替える）。 */
function target() {
  return { serial: state.serial, platform: currentPlatform() ?? "android" };
}

/** 端末の種類に応じて、セットアップ欄などの表示を切り替える。 */
function renderPlatform() {
  const ios = isIos();
  $("apk-path").hidden = ios;
  $("setup-hint").textContent = ios
    ? "デベロッパモードを確認し、位置シミュレーションに必要なデベロッパディスクイメージをマウントします（初回は数分かかることがあります）。"
    : "APK のパスを指定するとインストールも行います。空欄なら権限付与と「仮の現在地情報アプリ」設定のみ。";
  $("wireless").hidden = ios;
}

async function refreshDevices() {
  const select = $("device");
  try {
    const { devices, warnings } = await invoke("list_devices");
    state.devices = devices;
    select.replaceChildren();
    for (const d of devices) {
      const o = document.createElement("option");
      o.value = d.serial;
      o.disabled = d.state !== "device";
      const via = isWirelessSerial(d.serial) || d.serial.includes("._adb-tls-connect.") ? " · Wi-Fi" : "";
      const os = d.platform === "ios" ? ` · iOS ${d.os_version ?? ""}` : "";
      o.textContent = `${d.model ?? d.serial}${os}${via}${d.state !== "device" ? `（${d.state}）` : ""}`;
      select.append(o);
    }
    const usable = devices.filter((d) => d.state === "device");
    if (usable.length === 0) {
      select.append(new Option(devices.length ? "使用可能な端末がありません" : "端末が接続されていません", ""));
      state.serial = null;
    } else if (!usable.some((d) => d.serial === state.serial)) {
      state.serial = usable[0].serial;
    }
    select.value = state.serial ?? "";
    if (devices.some((d) => d.state === "unauthorized")) {
      showMessage("端末で「USB デバッグを許可」を承認してください", true);
    } else if (usable.length === 0 && warnings.length > 0) {
      showMessage(warnings.join("\n"), true);
    }
  } catch (e) {
    state.devices = [];
    select.replaceChildren(new Option("adb / pymobiledevice3 に接続できません", ""));
    state.serial = null;
    showMessage(String(e), true);
  }
  renderPlatform();
  await syncSession();
  updateButtons();
}

async function syncSession() {
  await invoke("set_session", { serial: state.serial, platform: currentPlatform(), stopOnExit: $("stop-on-exit").checked });
}

$("device").onchange = (e) => {
  state.serial = e.target.value || null;
  state.centeredOnce = false;
  state.status = null;
  renderPlatform();
  syncSession();
  updateButtons();
};
$("refresh-devices").onclick = refreshDevices;

function applyStatus(status) {
  const prevState = state.status?.state;
  state.status = status;
  const badge = $("state-badge");
  const labels = {
    NOT_RUNNING: "停止中",
    IDLE: "停止中",
    HOLDING: "固定中",
    WALKING: "移動中",
    PAUSED: "一時停止",
    ARRIVED: "到着",
    ERROR: "エラー",
  };
  badge.textContent = labels[status.state] ?? status.state;
  badge.className = `badge ${status.state}`;

  const p = status.position;
  $("st-position").textContent = p ? `${p.lat.toFixed(6)}, ${p.lng.toFixed(6)}` : "–";
  const moving = ["WALKING", "PAUSED"].includes(status.state);
  $("st-speed").textContent = moving ? `${status.speed_kmh.toFixed(1)} km/h` : "–";
  $("st-remaining").textContent = moving
    ? `${formatDistance(status.remaining_m)} ／ ${formatDuration(status.remaining_s)}`
    : "–";
  $("st-error").hidden = !status.error;
  $("st-error").textContent = status.error ?? "";

  if (p) {
    simMarker.setLatLng([p.lat, p.lng]);
    if (!map.hasLayer(simMarker)) simMarker.addTo(map);
    if (!state.centeredOnce) {
      map.setView([p.lat, p.lng], Math.max(map.getZoom(), 16));
      state.centeredOnce = true;
    }
  } else if (map.hasLayer(simMarker)) {
    simMarker.remove();
  }
  if (prevState === "WALKING" && status.state === "ARRIVED") showMessage("目的地に到着しました");
  if (!$("first-is-start").checked) renderRoute();
  else updateButtons();
}

async function poll() {
  if (state.serial && !state.busy) {
    try {
      applyStatus(await invoke("get_status", target()));
    } catch (e) {
      // 切断された可能性があるので端末一覧を取り直す
      applyStatus({ state: "NOT_RUNNING", position: null, speed_kmh: 0, remaining_m: 0, remaining_s: 0, error: null });
      await refreshDevices();
    }
  }
  setTimeout(poll, document.hidden ? POLL_HIDDEN_MS : POLL_MS);
}

renderRoute();
await refreshDevices();
poll();

// 座標計算・入力パース（DOM 非依存。node --test でテストする）

const EARTH_RADIUS_M = 6_371_008.8;

export function isValidLatLng(p) {
  return (
    p != null &&
    Number.isFinite(p.lat) &&
    Number.isFinite(p.lng) &&
    p.lat >= -90 &&
    p.lat <= 90 &&
    p.lng >= -180 &&
    p.lng <= 180
  );
}

/** "lat,lng" をパースする。不正なら Error。 */
export function parseLatLng(text) {
  const parts = String(text).split(",").map((s) => s.trim());
  if (parts.length !== 2 || parts.some((s) => s === "")) {
    throw new Error(`「緯度,経度」の形式で入力してください: ${text}`);
  }
  const p = { lat: Number(parts[0]), lng: Number(parts[1]) };
  if (!isValidLatLng(p)) throw new Error(`座標が範囲外または不正です: ${text}`);
  return p;
}

export function distanceM(a, b) {
  const rad = (d) => (d * Math.PI) / 180;
  const lat1 = rad(a.lat);
  const lat2 = rad(b.lat);
  const dLat = lat2 - lat1;
  const dLng = rad(b.lng - a.lng);
  const h = Math.sin(dLat / 2) ** 2 + Math.cos(lat1) * Math.cos(lat2) * Math.sin(dLng / 2) ** 2;
  return 2 * EARTH_RADIUS_M * Math.asin(Math.sqrt(Math.min(1, Math.max(0, h))));
}

export function routeLengthM(points) {
  let total = 0;
  for (let i = 1; i < points.length; i++) total += distanceM(points[i - 1], points[i]);
  return total;
}

export function formatDuration(seconds) {
  const s = Math.max(0, Math.floor(seconds));
  const h = Math.floor(s / 3600);
  const m = Math.floor((s % 3600) / 60);
  return `${h}:${String(m).padStart(2, "0")}:${String(s % 60).padStart(2, "0")}`;
}

export function formatDistance(m) {
  return m >= 1000 ? `${(m / 1000).toFixed(2)} km` : `${Math.round(m)} m`;
}

/** GPX の trkpt → rtept → wpt の順で最初に見つかった点列を返す。 */
export function parseGpx(xml) {
  for (const tag of ["trkpt", "rtept", "wpt"]) {
    const re = new RegExp(`<(?:[\\w-]+:)?${tag}\\b([^>]*)>`, "g");
    const points = [];
    let match;
    let index = 0;
    while ((match = re.exec(xml)) !== null) {
      index++;
      const attr = (name) => {
        const m = match[1].match(new RegExp(`\\b${name}\\s*=\\s*["']([^"']*)["']`));
        return m ? Number(m[1]) : NaN;
      };
      const p = { lat: attr("lat"), lng: attr("lon") };
      if (!isValidLatLng(p)) throw new Error(`GPX の ${tag} #${index} の lat/lon が不正です`);
      points.push(p);
    }
    if (points.length > 0) return points;
  }
  throw new Error("GPX に trkpt / rtept / wpt が見つかりません");
}

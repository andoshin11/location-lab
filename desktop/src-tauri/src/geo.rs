//! 経路上の位置計算（android/ の Geo.kt と同じ考え方）。iOS では Mac 側で移動を計算する。

use crate::adb::{distance_m, LatLng};

const EARTH_RADIUS_M: f64 = 6_371_008.8;

pub fn bearing_deg(a: LatLng, b: LatLng) -> f64 {
    let (lat1, lat2) = (a.lat.to_radians(), b.lat.to_radians());
    let dlng = (b.lng - a.lng).to_radians();
    let y = dlng.sin() * lat2.cos();
    let x = lat1.cos() * lat2.sin() - lat1.sin() * lat2.cos() * dlng.cos();
    (y.atan2(x).to_degrees() + 360.0) % 360.0
}

/// 大円上で a から b へ fraction (0..1) 進んだ点。
pub fn interpolate(a: LatLng, b: LatLng, fraction: f64) -> LatLng {
    let f = fraction.clamp(0.0, 1.0);
    let delta = distance_m(a, b) / EARTH_RADIUS_M;
    if delta < 1e-12 {
        return a;
    }
    let (lat1, lng1, lat2, lng2) = (a.lat.to_radians(), a.lng.to_radians(), b.lat.to_radians(), b.lng.to_radians());
    let ka = ((1.0 - f) * delta).sin() / delta.sin();
    let kb = (f * delta).sin() / delta.sin();
    let x = ka * lat1.cos() * lng1.cos() + kb * lat2.cos() * lng2.cos();
    let y = ka * lat1.cos() * lng1.sin() + kb * lat2.cos() * lng2.sin();
    let z = ka * lat1.sin() + kb * lat2.sin();
    LatLng {
        lat: z.atan2((x * x + y * y).sqrt()).to_degrees().clamp(-90.0, 90.0),
        lng: y.atan2(x).to_degrees().clamp(-180.0, 180.0),
    }
}

#[derive(Debug, Clone)]
pub struct Route {
    points: Vec<LatLng>,
    cumulative: Vec<f64>,
}

impl Route {
    pub fn new(points: Vec<LatLng>) -> Result<Self, String> {
        if points.len() < 2 {
            return Err("経路には 2 点以上が必要です".into());
        }
        let mut cumulative = vec![0.0; points.len()];
        for i in 1..points.len() {
            cumulative[i] = cumulative[i - 1] + distance_m(points[i - 1], points[i]);
        }
        Ok(Self { points, cumulative })
    }

    pub fn total_m(&self) -> f64 {
        *self.cumulative.last().unwrap()
    }

    /// 進んだ距離に対応する位置と進行方向。
    pub fn position_at(&self, traveled_m: f64) -> (LatLng, f64) {
        let d = traveled_m.clamp(0.0, self.total_m());
        let mut i = 1;
        while i < self.points.len() - 1 && self.cumulative[i] < d {
            i += 1;
        }
        let seg_len = self.cumulative[i] - self.cumulative[i - 1];
        let fraction = if seg_len <= 0.0 { 1.0 } else { (d - self.cumulative[i - 1]) / seg_len };
        let point = interpolate(self.points[i - 1], self.points[i], fraction);
        let bearing = (1..=i)
            .rev()
            .find(|&s| self.cumulative[s] - self.cumulative[s - 1] > 0.0)
            .map(|s| bearing_deg(self.points[s - 1], self.points[s]))
            .unwrap_or(0.0);
        (point, bearing)
    }
}

/// 1 秒ごとの通過地点（GPX 再生用）。最初の要素は from_m の地点、最後は終点。
#[derive(Debug, Clone, PartialEq)]
pub struct Sample {
    pub point: LatLng,
    pub traveled_m: f64,
    pub bearing: f64,
}

pub fn sample_route(route: &Route, from_m: f64, speed_mps: f64) -> Vec<Sample> {
    let total = route.total_m();
    let mut samples = Vec::new();
    let mut d = from_m.clamp(0.0, total);
    loop {
        let (point, bearing) = route.position_at(d);
        samples.push(Sample { point, traveled_m: d, bearing });
        if d >= total {
            break;
        }
        d = (d + speed_mps).min(total);
    }
    samples
}

/// pymobiledevice3 の `simulate-location play` に渡す GPX（1 秒間隔のタイムスタンプ付き）。
pub fn to_gpx(samples: &[Sample]) -> String {
    let mut s = String::from(
        "<?xml version=\"1.0\" encoding=\"UTF-8\"?>\n<gpx version=\"1.1\" creator=\"location-lab\" xmlns=\"http://www.topografix.com/GPX/1/1\">\n<trk><trkseg>\n",
    );
    for (i, sample) in samples.iter().enumerate() {
        // 再生では時刻の差分だけが使われるので、固定の基準時刻からの経過秒で十分
        let t = i as u64;
        s += &format!(
            "<trkpt lat=\"{:.7}\" lon=\"{:.7}\"><time>2000-01-01T{:02}:{:02}:{:02}Z</time></trkpt>\n",
            sample.point.lat,
            sample.point.lng,
            (t / 3600) % 24,
            (t / 60) % 60,
            t % 60
        );
    }
    s += "</trkseg></trk>\n</gpx>\n";
    s
}

#[cfg(test)]
mod tests {
    use super::*;

    fn p(lat: f64, lng: f64) -> LatLng {
        LatLng { lat, lng }
    }

    #[test]
    fn route_positions_and_bearing() {
        let a = p(0.0, 0.0);
        let b = p(0.0, 0.01);
        let c = p(0.01, 0.01);
        let route = Route::new(vec![a, b, b, c]).unwrap();
        let ab = distance_m(a, b);
        assert!((route.total_m() - (ab + distance_m(b, c))).abs() < 1e-6);
        let (mid, bearing) = route.position_at(ab / 2.0);
        assert!((bearing - 90.0).abs() < 1e-6);
        assert!((distance_m(a, mid) - ab / 2.0).abs() < 0.5);
        let (_, bearing2) = route.position_at(ab + 10.0);
        assert!(bearing2.abs() < 1e-3);
        assert!(distance_m(route.position_at(1e9).0, c) < 0.01);
        assert!(Route::new(vec![a]).is_err());
    }

    #[test]
    fn samples_are_one_second_apart_and_end_at_destination() {
        let route = Route::new(vec![p(35.0, 139.0), p(35.001, 139.0)]).unwrap(); // 約 111 m
        let samples = sample_route(&route, 0.0, 1.4);
        assert_eq!(samples.len(), (route.total_m() / 1.4).ceil() as usize + 1);
        for w in samples.windows(2) {
            assert!(distance_m(w[0].point, w[1].point) <= 1.4 + 0.01);
        }
        assert!(distance_m(samples.last().unwrap().point, p(35.001, 139.0)) < 0.01);

        // 途中から再開
        let resumed = sample_route(&route, 100.0, 1.4);
        assert!((resumed[0].traveled_m - 100.0).abs() < 1e-9);
        assert_eq!(resumed.len(), 1 + ((route.total_m() - 100.0) / 1.4).ceil() as usize);
    }

    #[test]
    fn gpx_has_increasing_timestamps() {
        let route = Route::new(vec![p(35.0, 139.0), p(35.0, 139.001)]).unwrap();
        let gpx = to_gpx(&sample_route(&route, 0.0, 1.4));
        assert!(gpx.contains("<trkpt lat=\"35.0000000\" lon=\"139.0000000\"><time>2000-01-01T00:00:00Z</time>"));
        assert!(gpx.contains("<time>2000-01-01T00:00:01Z</time>"));
        assert_eq!(gpx.matches("<trkpt").count(), 1 + (route.total_m() / 1.4).ceil() as usize);
    }
}

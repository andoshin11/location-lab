import { test } from "node:test";
import assert from "node:assert/strict";
import { distanceM, formatDistance, formatDuration, parseGpx, parseLatLng, routeLengthM } from "../ui/geo.js";

const tokyo = { lat: 35.681236, lng: 139.767125 };
const shinjuku = { lat: 35.690921, lng: 139.700258 };

test("parseLatLng validates input", () => {
  assert.deepEqual(parseLatLng(" 35.5 , 139.25 "), { lat: 35.5, lng: 139.25 });
  for (const bad of ["35.5", "a,b", "91,0", "0,181", ",139", "35,"]) {
    assert.throws(() => parseLatLng(bad), undefined, bad);
  }
});

test("distance and route length", () => {
  assert.ok(Math.abs(distanceM(tokyo, shinjuku) - 6130) < 50);
  assert.equal(routeLengthM([tokyo]), 0);
  assert.ok(Math.abs(routeLengthM([tokyo, shinjuku, tokyo]) - 2 * distanceM(tokyo, shinjuku)) < 1e-6);
});

test("formatting", () => {
  assert.equal(formatDuration(3725.9), "1:02:05");
  assert.equal(formatDuration(-1), "0:00:00");
  assert.equal(formatDistance(999.4), "999 m");
  assert.equal(formatDistance(6130), "6.13 km");
});

test("parseGpx prefers track points", () => {
  const xml = `<?xml version="1.0"?><gpx xmlns="http://www.topografix.com/GPX/1/1">
    <wpt lat="1" lon="1"/><trk><trkseg>
    <trkpt lat="35.0" lon="139.0"><ele>3</ele></trkpt>
    <trkpt lon='139.001' lat='35.001'/>
    </trkseg></trk></gpx>`;
  assert.deepEqual(parseGpx(xml), [
    { lat: 35.0, lng: 139.0 },
    { lat: 35.001, lng: 139.001 },
  ]);
});

test("parseGpx rejects invalid data", () => {
  assert.throws(() => parseGpx("<gpx><trk/></gpx>"), /見つかりません/);
  assert.throws(() => parseGpx('<gpx><wpt lat="95" lon="0"/></gpx>'), /#1/);
  assert.throws(() => parseGpx('<gpx><rtept lat="35"/></gpx>'), /rtept/);
});

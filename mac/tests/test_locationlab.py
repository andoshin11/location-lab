import os
import sys
import tempfile
import unittest
from unittest import mock

sys.path.insert(0, os.path.join(os.path.dirname(__file__), ".."))

import locationlab as ll  # noqa: E402


class FakeAdb:
    """am start-foreground-service / dumpsys を模倣する。"""

    def __init__(self, states):
        self.sent = []
        self.states = list(states)

    def send(self, action, extras=None):
        self.sent.append((action, extras or {}))

    def status(self):
        return self.states.pop(0) if len(self.states) > 1 else self.states[0]


class CoordinateTest(unittest.TestCase):
    def test_parse(self):
        self.assertEqual(ll.parse_latlng(" 35.68 , 139.76 "), ll.LatLng(35.68, 139.76))
        for bad in ("35.6", "a,b", "91,0", "0,181", "nan,0"):
            with self.assertRaises(ll.CliError, msg=bad):
                ll.parse_latlng(bad)

    def test_distance(self):
        tokyo = ll.LatLng(35.681236, 139.767125)
        shinjuku = ll.LatLng(35.690921, 139.700258)
        self.assertAlmostEqual(ll.distance_m(tokyo, shinjuku), 6130, delta=50)
        self.assertAlmostEqual(ll.route_length_m([tokyo, shinjuku, tokyo]), 2 * ll.distance_m(tokyo, shinjuku))

    def test_speed(self):
        self.assertAlmostEqual(ll.kmh_to_mps(5.0), 1.3889, places=3)
        for bad in (0.0, -1.0, 200.0, float("inf")):
            with self.assertRaises(ll.CliError):
                ll.kmh_to_mps(bad)

    def test_duration(self):
        self.assertEqual(ll.format_duration(3725.9), "1:02:05")
        self.assertEqual(ll.format_duration(-3), "0:00:00")


class GpxTest(unittest.TestCase):
    def write(self, body):
        f = tempfile.NamedTemporaryFile("w", suffix=".gpx", delete=False)
        f.write(body)
        f.close()
        self.addCleanup(os.unlink, f.name)
        return f.name

    def test_track_points(self):
        path = self.write(
            '<gpx xmlns="http://www.topografix.com/GPX/1/1"><trk><trkseg>'
            '<trkpt lat="35.0" lon="139.0"/><trkpt lat="35.001" lon="139.001"/>'
            "</trkseg></trk><wpt lat=\"1\" lon=\"1\"/></gpx>"
        )
        self.assertEqual(ll.load_gpx(path), [ll.LatLng(35.0, 139.0), ll.LatLng(35.001, 139.001)])

    def test_invalid(self):
        with self.assertRaises(ll.CliError):
            ll.load_gpx(self.write("<gpx><trk/></gpx>"))
        with self.assertRaises(ll.CliError):
            ll.load_gpx(self.write('<gpx><wpt lat="95" lon="0"/></gpx>'))
        with self.assertRaises(ll.CliError):
            ll.load_gpx(self.write("<gpx><broken"))
        with self.assertRaises(ll.CliError):
            ll.load_gpx("/nonexistent.gpx")


class StatusTest(unittest.TestCase):
    def test_parse_dumpsys(self):
        out = "SERVICE dev.locationlab.helper/.MockLocationService 1234 pid=1\n  state=WALKING\n  position=35.0,139.0\n  error=x=y\n"
        status = ll.parse_status(out)
        self.assertEqual(status["state"], "WALKING")
        self.assertEqual(status["position"], "35.0,139.0")
        self.assertEqual(status["error"], "x=y")


@mock.patch("time.sleep", lambda _: None)
class CommandTest(unittest.TestCase):
    def test_walk_sends_waypoints_and_speed(self):
        adb = FakeAdb([{"state": "WALKING", "position": "35.0,139.0"}])
        args = ll.build_parser().parse_args(["walk", "35.001,139.0", "35.002,139.0", "--speed", "3.6"])
        args.func(adb, args)
        action, extras = adb.sent[0]
        self.assertEqual(action, "WALK")
        self.assertEqual(extras["to"], "35.0010000,139.0000000;35.0020000,139.0000000")
        self.assertEqual(extras["speed"], "1.0000")
        self.assertNotIn("from", extras)

    def test_set_refuses_large_jump(self):
        adb = FakeAdb([{"state": "HOLDING", "position": "35.0,139.0"}])
        args = ll.build_parser().parse_args(["set", "36.0,139.0"])
        with self.assertRaises(ll.CliError):
            args.func(adb, args)
        self.assertEqual(adb.sent, [])

    def test_set_force(self):
        adb = FakeAdb([{"state": "HOLDING", "position": "35.0,139.0"}])
        args = ll.build_parser().parse_args(["set", "36.0,139.0", "--force"])
        args.func(adb, args)
        self.assertEqual(adb.sent, [("SET", {"point": "36.0000000,139.0000000"})])

    def test_error_state_is_not_reported_as_success(self):
        adb = FakeAdb([{"state": "ERROR", "error": "not selected as mock app"}])
        args = ll.build_parser().parse_args(["walk", "35.001,139.0", "--from", "35.0,139.0"])
        with self.assertRaisesRegex(ll.CliError, "not selected"):
            args.func(adb, args)

    def test_stop_waits_until_service_ends(self):
        adb = FakeAdb([{"state": "HOLDING"}, {"state": "NOT_RUNNING"}])
        args = ll.build_parser().parse_args(["stop"])
        args.func(adb, args)
        self.assertEqual(adb.sent, [("STOP", {})])


if __name__ == "__main__":
    unittest.main()

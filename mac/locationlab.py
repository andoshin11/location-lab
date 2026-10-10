#!/usr/bin/env python3
"""Location Lab: Mac から adb 経由で Android 端末の位置シミュレーションを操作する CLI。

端末側には android/ の補助アプリをインストールし、開発者向けオプションで
「仮の現在地情報アプリ」として選択しておく必要がある。
"""

from __future__ import annotations

import argparse
import math
import shutil
import subprocess
import sys
import time
import xml.etree.ElementTree as ET
from dataclasses import dataclass

PACKAGE = "dev.locationlab.helper"
SERVICE = f"{PACKAGE}/.MockLocationService"
ACTION_PREFIX = "dev.locationlab.action."

DEFAULT_WALK_KMH = 5.0
MIN_SPEED_MPS = 0.1
MAX_SPEED_MPS = 50.0
EARTH_RADIUS_M = 6_371_008.8


class CliError(Exception):
    pass


@dataclass(frozen=True)
class LatLng:
    lat: float
    lng: float

    def __post_init__(self) -> None:
        if not (math.isfinite(self.lat) and -90.0 <= self.lat <= 90.0):
            raise CliError(f"latitude out of range: {self.lat}")
        if not (math.isfinite(self.lng) and -180.0 <= self.lng <= 180.0):
            raise CliError(f"longitude out of range: {self.lng}")

    def format(self) -> str:
        return f"{self.lat:.7f},{self.lng:.7f}"


def parse_latlng(text: str) -> LatLng:
    parts = [p.strip() for p in text.split(",")]
    if len(parts) != 2:
        raise CliError(f"expected 'lat,lng': {text!r}")
    try:
        return LatLng(float(parts[0]), float(parts[1]))
    except ValueError:
        raise CliError(f"invalid coordinate: {text!r}") from None


def distance_m(a: LatLng, b: LatLng) -> float:
    lat1, lat2 = math.radians(a.lat), math.radians(b.lat)
    dlat = lat2 - lat1
    dlng = math.radians(b.lng - a.lng)
    h = math.sin(dlat / 2) ** 2 + math.cos(lat1) * math.cos(lat2) * math.sin(dlng / 2) ** 2
    return 2 * EARTH_RADIUS_M * math.asin(math.sqrt(min(1.0, max(0.0, h))))


def route_length_m(points: list[LatLng]) -> float:
    return sum(distance_m(a, b) for a, b in zip(points, points[1:]))


def kmh_to_mps(kmh: float) -> float:
    mps = kmh / 3.6
    if not (math.isfinite(mps) and MIN_SPEED_MPS <= mps <= MAX_SPEED_MPS):
        raise CliError(f"speed must be {MIN_SPEED_MPS * 3.6:.2f}..{MAX_SPEED_MPS * 3.6:.0f} km/h: {kmh}")
    return mps


def load_gpx(path: str) -> list[LatLng]:
    """GPX の trkpt → rtept → wpt の順で最初に見つかった点列を返す。"""
    try:
        root = ET.parse(path).getroot()
    except (OSError, ET.ParseError) as e:
        raise CliError(f"cannot read GPX {path}: {e}") from None
    for tag in ("trkpt", "rtept", "wpt"):
        nodes = [n for n in root.iter() if n.tag == tag or n.tag.endswith("}" + tag)]
        if nodes:
            points = []
            for i, n in enumerate(nodes):
                try:
                    points.append(LatLng(float(n.attrib["lat"]), float(n.attrib["lon"])))
                except (KeyError, ValueError):
                    raise CliError(f"GPX {tag} #{i + 1} has invalid lat/lon") from None
            return points
    raise CliError(f"GPX {path} contains no trkpt/rtept/wpt")


def parse_status(text: str) -> dict[str, str]:
    status: dict[str, str] = {}
    for line in text.splitlines():
        key, sep, value = line.strip().partition("=")
        if sep and key.isidentifier():
            status[key] = value
    return status


def format_duration(seconds: float) -> str:
    s = max(0, int(seconds))
    return f"{s // 3600}:{s % 3600 // 60:02d}:{s % 60:02d}"


# ---- adb ----


class Adb:
    def __init__(self, serial: str | None) -> None:
        exe = shutil.which("adb")
        if not exe:
            raise CliError("adb not found. Install Android platform-tools (e.g. `brew install --cask android-platform-tools`).")
        self.base = [exe] + (["-s", serial] if serial else [])

    def run(self, *args: str, timeout: float = 20) -> str:
        try:
            proc = subprocess.run(self.base + list(args), capture_output=True, text=True, timeout=timeout)
        except subprocess.TimeoutExpired:
            raise CliError(f"adb timed out: {' '.join(args)}") from None
        out = (proc.stdout + proc.stderr).strip()
        if proc.returncode != 0:
            raise CliError(f"adb failed ({proc.returncode}): {out}")
        return out

    def shell(self, *args: str) -> str:
        return self.run("shell", *args)

    def ensure_device(self) -> None:
        state = self.run("get-state").strip()
        if state != "device":
            raise CliError(f"device not ready: {state} (check USB debugging authorization)")

    def send(self, action: str, extras: dict[str, str] | None = None) -> None:
        args = ["am", "start-foreground-service", "-n", SERVICE, "-a", ACTION_PREFIX + action]
        for k, v in (extras or {}).items():
            args += ["--es", k, v]
        out = self.shell(*args)
        if "Error" in out or "Exception" in out:
            raise CliError(out)

    def status(self) -> dict[str, str]:
        out = self.shell("dumpsys", "activity", "service", SERVICE)
        status = parse_status(out)
        return status or {"state": "NOT_RUNNING"}


def wait_for_state(adb: Adb, expected: set[str], timeout: float = 5.0) -> dict[str, str]:
    """コマンド送信後、サービスが期待する状態になるまで待つ。ERROR なら例外。"""
    deadline = time.monotonic() + timeout
    status: dict[str, str] = {}
    time.sleep(0.3)  # am start-foreground-service は非同期なので、前の状態を読まないよう少し待つ
    while time.monotonic() < deadline:
        status = adb.status()
        if status.get("state") == "ERROR":
            raise CliError(f"device reported error: {status.get('error', 'unknown')}")
        if status.get("state") in expected:
            return status
        time.sleep(0.3)
    raise CliError(f"timed out waiting for {sorted(expected)} (last state: {status.get('state')})")


def print_status(status: dict[str, str]) -> None:
    state = status.get("state", "UNKNOWN")
    print(f"state:     {state}")
    if "position" in status:
        print(f"position:  {status['position']}")
    if state in ("WALKING", "PAUSED"):
        speed = float(status.get("speed_mps", "0"))
        print(f"speed:     {speed * 3.6:.1f} km/h")
        print(f"remaining: {float(status.get('remaining_m', '0')):.0f} m, {format_duration(float(status.get('remaining_s', '0')))}")
    if status.get("providers"):
        print(f"providers: {status['providers']}")
    if "error" in status:
        print(f"error:     {status['error']}")


# ---- commands ----


def cmd_setup(adb: Adb, args: argparse.Namespace) -> None:
    if args.apk:
        print(adb.run("install", "-r", args.apk, timeout=120))
    for perm in ("android.permission.ACCESS_FINE_LOCATION", "android.permission.ACCESS_COARSE_LOCATION"):
        adb.shell("pm", "grant", PACKAGE, perm)
    # 開発者向けオプションの「仮の現在地情報アプリ」に相当
    adb.shell("appops", "set", PACKAGE, "android:mock_location", "allow")
    print("setup done: location permission granted, mock location app op allowed.")


def cmd_status(adb: Adb, args: argparse.Namespace) -> None:
    print_status(adb.status())


def cmd_set(adb: Adb, args: argparse.Namespace) -> None:
    point = parse_latlng(args.point)
    current = adb.status()
    if current.get("position") and not args.force:
        jump = distance_m(parse_latlng(current["position"]), point)
        if jump > 100:
            raise CliError(
                f"this would jump {jump:.0f} m instantly. Use `walk` to move gradually, or pass --force."
            )
    adb.send("SET", {"point": point.format()})
    print_status(wait_for_state(adb, {"HOLDING"}))


def cmd_walk(adb: Adb, args: argparse.Namespace) -> None:
    if args.gpx:
        waypoints = load_gpx(args.gpx)
    elif args.to:
        waypoints = [parse_latlng(p) for p in args.to]
    else:
        raise CliError("specify destination(s) or --gpx")
    speed = kmh_to_mps(args.speed)
    extras = {"to": ";".join(p.format() for p in waypoints), "speed": f"{speed:.4f}"}
    start = None
    if args.start:
        start = parse_latlng(args.start)
        extras["from"] = start.format()
    adb.send("WALK", extras)
    status = wait_for_state(adb, {"WALKING", "ARRIVED"})
    if start is None and "position" in status:
        start = parse_latlng(status["position"])
    if start is not None:
        length = route_length_m([start] + waypoints)
        print(f"route: {length:.0f} m at {args.speed:.1f} km/h, ETA {format_duration(length / speed)}")
    print_status(status)
    if args.follow:
        follow(adb)


def follow(adb: Adb) -> None:
    try:
        while True:
            status = adb.status()
            state = status.get("state")
            line = f"\r{state:<8} {status.get('position', '-'):<24}"
            if state in ("WALKING", "PAUSED"):
                line += f" {float(status.get('remaining_m', '0')):>7.0f} m  {format_duration(float(status.get('remaining_s', '0')))}"
            print(line.ljust(70), end="", flush=True)
            if state in ("ARRIVED", "ERROR", "IDLE", "NOT_RUNNING", "HOLDING"):
                print()
                if state == "ERROR":
                    raise CliError(status.get("error", "unknown error"))
                return
            time.sleep(1)
    except KeyboardInterrupt:
        print("\n(stopped following; simulation continues on the device. Use `stop` to restore.)")


def cmd_pause(adb: Adb, args: argparse.Namespace) -> None:
    adb.send("PAUSE")
    print_status(wait_for_state(adb, {"PAUSED", "ARRIVED", "HOLDING"}))


def cmd_resume(adb: Adb, args: argparse.Namespace) -> None:
    adb.send("RESUME")
    print_status(wait_for_state(adb, {"WALKING", "ARRIVED", "HOLDING"}))


def cmd_speed(adb: Adb, args: argparse.Namespace) -> None:
    adb.send("SPEED", {"speed": f"{kmh_to_mps(args.kmh):.4f}"})
    time.sleep(0.5)
    print_status(adb.status())


def cmd_stop(adb: Adb, args: argparse.Namespace) -> None:
    adb.send("STOP")
    # サービス終了（= テストプロバイダ解除）を確認できるまで完了扱いにしない
    wait_for_state(adb, {"IDLE", "NOT_RUNNING"})
    print("stopped: mock location removed, device is back to its real location.")


def build_parser() -> argparse.ArgumentParser:
    p = argparse.ArgumentParser(prog="locationlab", description=__doc__)
    p.add_argument("-s", "--serial", help="adb device serial (when multiple devices are connected)")
    sub = p.add_subparsers(dest="command", required=True)

    s = sub.add_parser("setup", help="install APK (optional) and grant permissions / mock location")
    s.add_argument("--apk", help="path to app-debug.apk to install")
    s.set_defaults(func=cmd_setup)

    s = sub.add_parser("status", help="show simulation status")
    s.set_defaults(func=cmd_status)

    s = sub.add_parser("set", help="hold a fixed position")
    s.add_argument("point", help="lat,lng")
    s.add_argument("--force", action="store_true", help="allow an instant jump of more than 100 m")
    s.set_defaults(func=cmd_set)

    s = sub.add_parser("walk", help="walk from the current position to the destination at a realistic speed")
    s.add_argument("to", nargs="*", help="lat,lng (multiple = waypoints in order)")
    s.add_argument("--gpx", help="follow the points in a GPX file instead")
    s.add_argument("--from", dest="start", help="start lat,lng (default: current simulated or real last known position)")
    s.add_argument("--speed", type=float, default=DEFAULT_WALK_KMH, help=f"km/h (default {DEFAULT_WALK_KMH})")
    s.add_argument("-f", "--follow", action="store_true", help="show progress until arrival")
    s.set_defaults(func=cmd_walk)

    s = sub.add_parser("pause", help="pause walking (keeps the current position)")
    s.set_defaults(func=cmd_pause)
    s = sub.add_parser("resume", help="resume walking")
    s.set_defaults(func=cmd_resume)
    s = sub.add_parser("speed", help="change walking speed")
    s.add_argument("kmh", type=float)
    s.set_defaults(func=cmd_speed)
    s = sub.add_parser("stop", help="stop simulation and restore the real location")
    s.set_defaults(func=cmd_stop)
    return p


def main(argv: list[str] | None = None) -> int:
    args = build_parser().parse_args(argv)
    try:
        adb = Adb(args.serial)
        adb.ensure_device()
        args.func(adb, args)
    except CliError as e:
        print(f"error: {e}", file=sys.stderr)
        return 1
    return 0


if __name__ == "__main__":
    sys.exit(main())

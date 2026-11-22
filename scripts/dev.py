#!/usr/bin/env python3
"""Watch Minc's macOS build and refresh the same isolated development app."""

import argparse
import fcntl
import filecmp
import json
import os
from pathlib import Path
import plistlib
import shutil
import subprocess
import sys
import time

ROOT = Path(__file__).resolve().parents[1]
TARGET = ROOT / "target"
BUILD = TARGET / "debug/winc"
DEV = TARGET / "minc-dev"
LIBRARY = DEV / "Quiet.md 基准笔记库"
HOME = DEV / "home"
STATE = HOME / "Library/Application Support/winc/state.json"
BUNDLE = DEV / "Minc Dev.app"
EXECUTABLE = BUNDLE / "Contents/MacOS/minc-dev"
BUNDLE_ID = "local.minc.dev"


def message(text):
    print(f"[Minc Dev] {text}", flush=True)


def source_stamp():
    paths = [ROOT / name for name in ("Cargo.toml", "Cargo.lock", "rust-toolchain.toml", "VERSION")]
    for folder in ("crates", ".cargo"):
        for directory, _, files in os.walk(ROOT / folder):
            paths.extend(Path(directory) / name for name in files if not name.startswith("."))
    stamp = {}
    for path in paths:
        try:
            st = path.stat()
            stamp[path] = (st.st_mtime_ns, st.st_size)
        except FileNotFoundError:  # Atomic editor saves may briefly remove a file.
            pass
    return stamp


def stop_preview():
    # Ask the native app to quit normally. Never force-quit through a save prompt,
    # and never touch an app with a different bundle path.
    return subprocess.run([
        "/usr/bin/swift", str(ROOT / "scripts/quit-dev.swift"), BUNDLE_ID, str(BUNDLE),
    ]).returncode == 0


def deploy(activate_existing):
    changed = not EXECUTABLE.is_file() or not filecmp.cmp(BUILD, EXECUTABLE, shallow=False)
    if changed and EXECUTABLE.is_file() and not stop_preview():
        message("暂不更新：请在 Minc Dev 处理退出提示，下次保存代码或重跑脚本即可重试。")
        return False
    if not changed and not activate_existing:
        return True

    if changed:
        EXECUTABLE.parent.mkdir(parents=True, exist_ok=True)
        with (BUNDLE / "Contents/Info.plist").open("wb") as manifest:
            plistlib.dump({
                "CFBundleName": "Minc Dev",
                "CFBundleIdentifier": BUNDLE_ID,
                "CFBundleExecutable": EXECUTABLE.name,
                "CFBundlePackageType": "APPL",
            }, manifest)
        staging = EXECUTABLE.with_suffix(".new")
        shutil.copy2(BUILD, staging)
        staging.replace(EXECUTABLE)

    try:
        restore = bool(json.loads(STATE.read_text()).get("last"))
    except (OSError, ValueError):
        restore = False
    command = ["/usr/bin/open", "-a", str(BUNDLE), "--env", f"HOME={HOME}", "--args", "-n"]
    if not restore and LIBRARY.is_dir():
        command.append(str(LIBRARY))
    subprocess.run(command, check=True)
    message(f"{'已刷新' if changed else '已打开'} {BUNDLE.name}")
    return True


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--once", action="store_true", help="build and open once, without watching")
    args = parser.parse_args()
    if sys.platform != "darwin":
        parser.error("this development launcher is for macOS")

    DEV.mkdir(exist_ok=True, parents=True)
    with (DEV / "watch.lock").open("a") as lock:
        try:
            fcntl.flock(lock, fcntl.LOCK_EX | fcntl.LOCK_NB)
        except BlockingIOError:
            message("已有监听进程，不重复打开窗口。")
            return 0
        HOME.mkdir(exist_ok=True)
        first = True
        while True:
            stamp = source_stamp()
            message("正在编译…")
            built = subprocess.run([
                "cargo", "build", "--quiet", "-p", "winc", "--target-dir", str(TARGET),
            ], cwd=ROOT).returncode == 0
            ok = built and deploy(first)
            if not built:
                message("编译未通过，保留当前窗口。")
            if args.once:
                return 0 if ok else 1
            first = False
            message("监听代码保存（Ctrl+C 停止监听，保留测试窗口）。")
            while True:
                time.sleep(0.5)
                updated = source_stamp()
                if stamp == updated:
                    continue
                # Debounce a group of saves, e.g. from cargo fmt.
                time.sleep(0.5)
                if updated == source_stamp():
                    break


if __name__ == "__main__":
    try:
        sys.exit(main())
    except KeyboardInterrupt:
        sys.exit(0)
    except (OSError, subprocess.CalledProcessError) as error:
        message(str(error))
        sys.exit(1)

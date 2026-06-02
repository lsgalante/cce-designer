import subprocess
import time
import re
import sys
import os
import math

def run_clearctl(cmd_str):
    full_cmd = f"env WAYLAND_DISPLAY=wayland-0 XDG_RUNTIME_DIR=/run/user/1000 /home/lsgalante/.local/bin/clearctl {cmd_str}"
    res = subprocess.run(full_cmd, shell=True, capture_output=True, text=True)
    return res.stdout.strip()

def get_window_rect(title):
    stdout = run_clearctl("windows")
    for line in stdout.splitlines():
        if title in line:
            m = re.search(r'\bx=(\d+)\s+y=(\d+)\s+w=(\d+)\s+h=(\d+)', line)
            if m:
                return int(m.group(1)), int(m.group(2)), int(m.group(3)), int(m.group(4))
    return None

def reset_pointer(window_x, window_y, window_w, window_h):
    cx = window_x + window_w // 2
    cy = window_y + window_h // 2
    print(f"Focusing window at ({cx}, {cy})...")
    run_clearctl(f"pointer-move-to {cx} {cy}")
    time.sleep(0.2)
    run_clearctl("pointer-click left")
    time.sleep(0.3)
    run_clearctl("key-press Escape")
    time.sleep(0.05)
    run_clearctl("key-release Escape")
    time.sleep(0.2)

def get_curved_circle_params():
    debug_path = "/home/lsgalante/Dropbox/Clear/debug.txt"
    if not os.path.exists(debug_path):
        return None
    with open(debug_path, "r") as f:
        lines = f.readlines()
    for line in reversed(lines):
        # Match curved=Some((298.9297, 279.10547, 180.0))
        m = re.search(r'curved=Some\(\(([\d\.-]+),\s*([\d\.-]+),\s*([\d\.-]+)\)\)', line)
        if m:
            return float(m.group(1)), float(m.group(2)), float(m.group(3))
    return None

title = "Clear Design Interface"
rect = get_window_rect(title)
if not rect:
    print(f"Error: Could not find window '{title}'", file=sys.stderr)
    sys.exit(1)

window_x, window_y, window_w, window_h = rect
print(f"Found window '{title}' at x={window_x}, y={window_y}, w={window_w}, h={window_h}")

reset_pointer(window_x, window_y, window_w, window_h)

circle_params = get_curved_circle_params()
if not circle_params:
    print("Error: Could not read curved circle parameters from debug.txt. Using defaults.", file=sys.stderr)
    circle_params = (298.9297, 279.10547, 180.0)

cx, cy, r = circle_params
print(f"Curved circle center from debug.txt: cx={cx}, cy={cy}, r={r}")

# View menu midpoint angle:
theta = (5.1339273 + 5.417004) / 2.0
r_mid = r - 17.5

local_x = cx + r_mid * math.cos(theta)
local_y = cy + r_mid * math.sin(theta)

border_offset_x = 0
border_offset_y = 0

scale = 2.0
screen_x = int(window_x + border_offset_x + local_x * scale)
screen_y = int(window_y + border_offset_y + local_y * scale)

print(f"Targeting client local ({local_x:.2f}, {local_y:.2f}) -> screen ({screen_x}, {screen_y})")

log_path = "/home/lsgalante/Dropbox/Clear/design-interface.log"
initial_size = 0
if os.path.exists(log_path):
    initial_size = os.path.getsize(log_path)

print(f"Moving pointer to ({screen_x}, {screen_y})...")
print("Move result:", run_clearctl(f"pointer-move-to {screen_x} {screen_y}"))
time.sleep(0.5)

print("Clicking left...")
print("Click result:", run_clearctl("pointer-click left"))
time.sleep(1.0)

# Check design-interface.log for new DEBUG output
new_logs = ""
if os.path.exists(log_path):
    with open(log_path, "r", errors="ignore") as f:
        f.seek(initial_size)
        new_logs = f.read()

print("--- New Application Logs ---")
print(new_logs)
print("----------------------------")

screenshot_path = "/home/lsgalante/.gemini/antigravity/brain/9048aec8-21a4-458f-94d4-efe127095c7f/screenshot_circular_view_dropdown.png"
print(f"Taking screenshot: {screenshot_path}")
subprocess.run(f"env WAYLAND_DISPLAY=wayland-0 XDG_RUNTIME_DIR=/run/user/1000 grim {screenshot_path}", shell=True)
print("Done.")

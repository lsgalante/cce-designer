import subprocess
import time
import re
import sys
import os

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
    # Click at window center to focus it
    cx = window_x + window_w // 2
    cy = window_y + window_h // 2
    print(f"Focusing window at ({cx}, {cy})...")
    run_clearctl(f"pointer-move-to {cx} {cy}")
    time.sleep(0.2)
    run_clearctl("pointer-click left")
    time.sleep(0.3)
    # Press Escape to dismiss any open menus
    run_clearctl("key-press Escape")
    time.sleep(0.05)
    run_clearctl("key-release Escape")
    time.sleep(0.2)

title = "Clear Design Interface"
rect = get_window_rect(title)
if not rect:
    print(f"Error: Could not find window '{title}'", file=sys.stderr)
    sys.exit(1)

window_x, window_y, window_w, window_h = rect
print(f"Window: x={window_x}, y={window_y}, w={window_w}, h={window_h}")

# File menu local coordinates
file_local_x = 250.5
file_local_y = 137.5

# Test offset = 0
reset_pointer(window_x, window_y, window_w, window_h)
x0 = int(window_x + file_local_x)
y0 = int(window_y + file_local_y)
print(f"Testing Offset 0: screen ({x0}, {y0})")
run_clearctl(f"pointer-move-to {x0} {y0}")
time.sleep(0.3)
run_clearctl("pointer-click left")
time.sleep(1.0)
subprocess.run("env WAYLAND_DISPLAY=wayland-0 XDG_RUNTIME_DIR=/run/user/1000 grim /home/lsgalante/.gemini/antigravity/brain/9048aec8-21a4-458f-94d4-efe127095c7f/screenshot_offset_0.png", shell=True)

# Test offset = 24
reset_pointer(window_x, window_y, window_w, window_h)
x24 = int(window_x + 24 + file_local_x)
y24 = int(window_y + 24 + file_local_y)
print(f"Testing Offset 24: screen ({x24}, {y24})")
run_clearctl(f"pointer-move-to {x24} {y24}")
time.sleep(0.3)
run_clearctl("pointer-click left")
time.sleep(1.0)
subprocess.run("env WAYLAND_DISPLAY=wayland-0 XDG_RUNTIME_DIR=/run/user/1000 grim /home/lsgalante/.gemini/antigravity/brain/9048aec8-21a4-458f-94d4-efe127095c7f/screenshot_offset_24.png", shell=True)

print("Done testing offsets.")

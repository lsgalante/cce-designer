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

log_path = "/home/lsgalante/Dropbox/Clear/design-interface.log"

def clear_log():
    if os.path.exists(log_path):
        with open(log_path, "w") as f:
            f.write("")

def check_log_for_press():
    if not os.path.exists(log_path):
        return False
    with open(log_path, "r", errors="ignore") as f:
        content = f.read()
    return "DEBUG" in content

title = "Clear Design Interface"
rect = get_window_rect(title)
if not rect:
    print(f"Error: Could not find window '{title}'", file=sys.stderr)
    sys.exit(1)

window_x, window_y, window_w, window_h = rect
print(f"Window rect: x={window_x}, y={window_y}, w={window_w}, h={window_h}")

# Focus the window first
run_clearctl(f"focus-window \"{title}\"")
time.sleep(0.5)

# We will sweep dx from 50 to 200, dy from 0 to 100, in steps of 20
found = False
for dy in range(0, 150, 10):
    for dx in range(50, 250, 20):
        clear_log()
        target_x = window_x + dx
        target_y = window_y + dy
        print(f"Testing client offset dx={dx}, dy={dy} -> screen ({target_x}, {target_y})")
        
        run_clearctl(f"pointer-move-to {target_x} {target_y}")
        time.sleep(0.1)
        run_clearctl("pointer-click left")
        time.sleep(0.2)
        
        if check_log_for_press():
            print(f"SUCCESS! Click registered at offset dx={dx}, dy={dy}")
            found = True
            break
    if found:
        break

if not found:
    print("Failed to register any clicks inside the window.")

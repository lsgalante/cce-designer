import subprocess
import time

def run_clearctl(cmd_str):
    full_cmd = f"env WAYLAND_DISPLAY=wayland-0 XDG_RUNTIME_DIR=/run/user/1000 /home/lsgalante/.local/bin/clearctl {cmd_str}"
    res = subprocess.run(full_cmd, shell=True, capture_output=True, text=True)
    return res.stdout.strip()

def click_at(x, y):
    print(f"Moving to {x}, {y}...")
    run_clearctl(f"pointer-move-to {x} {y}")
    time.sleep(0.2)
    print(f"Clicking at {x}, {y}...")
    run_clearctl("pointer-click left")
    time.sleep(0.3)

# Focus the design interface window
run_clearctl("focus-window cce-design-interface")
time.sleep(0.5)

# Click on Box 1 (x=238, y=389)
click_at(238, 389)
subprocess.run("env WAYLAND_DISPLAY=wayland-0 XDG_RUNTIME_DIR=/run/user/1000 grim /home/lsgalante/.gemini/antigravity/brain/ab8b2bd9-c7ba-4730-96bb-8766ee711f3a/screenshot_click_box.png", shell=True)

# Click on Scatter 1 (x=238, y=469)
click_at(238, 469)
subprocess.run("env WAYLAND_DISPLAY=wayland-0 XDG_RUNTIME_DIR=/run/user/1000 grim /home/lsgalante/.gemini/antigravity/brain/ab8b2bd9-c7ba-4730-96bb-8766ee711f3a/screenshot_click_scatter.png", shell=True)

print("Done.")

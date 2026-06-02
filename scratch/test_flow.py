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

# Reset cursor & close any menus
run_clearctl("key-press Escape")
time.sleep(0.05)
run_clearctl("key-release Escape")
time.sleep(0.2)

# Focus the window
click_at(1100, 150)

# Move to View menu and click
# View menu is around local x=230 (global 984+230 = 1214), local y=39 (global 48+39 = 87)
click_at(1214, 87)

# Move to "Circular Pane" dropdown and click
# Dropdown is at x=207 to 344. Let's use local x=270 (global 984+270 = 1254).
# Y center is local y=107 (global 48+107 = 155).
click_at(1254, 155)

print("Taking screenshot...")
subprocess.run("env WAYLAND_DISPLAY=wayland-0 XDG_RUNTIME_DIR=/run/user/1000 grim /home/lsgalante/.gemini/antigravity/brain/9048aec8-21a4-458f-94d4-efe127095c7f/screenshot_test.png", shell=True)
print("Done.")

# kompact desktop

A small Wayland desktop for Linux that looks and works like macOS: a menu bar, a dock,
Spotlight search, System Settings, its own lock screen and (optionally) its own login screen.

## What you need

- **Ubuntu 24.04 LTS or newer**, or another Debian-based system of the same age, 64-bit PC
  (x86_64). The installer checks this and stops if the system is too old.
- A real machine, or a virtual machine with a virtual graphics card (VirtualBox: VMSVGA;
  VMware: SVGA; QEMU/virt-manager: virtio-gpu). Turning on 3D acceleration in the VM's settings
  makes it smoother.
- An internet connection during install (it installs the packages kompact needs with apt).

## Install

```sh
tar -xzf kompact-desktop-VERSION-x86_64.tar.gz
cd kompact-desktop-VERSION-x86_64
sudo ./install.sh
```

Then log out, pick **kompact** on the login screen (on Ubuntu's: choose your name, then the
gear button at the bottom right), and log in. Your current desktop stays installed and is
still there on the login screen.

To use kompact's own login screen at boot instead of the current one:

```sh
sudo ./install.sh --login-screen       # or later: sudo kompact-login-screen enable
sudo kompact-login-screen disable      # back to the old login screen (from the next boot)
```

## First steps

| | |
|---|---|
| Super+Space | Spotlight: find and open apps, calculate |
| Super+Return | Terminal (foot) |
| Super+Q | Close the window |
| Super+Ctrl+Q | Lock the screen |
| Logo menu (top left) | System Settings, Lock Screen, Log Out |

Every shortcut can be changed (System Settings → Keyboard Shortcuts, with macOS, Windows 11 and
classic presets).

## Siva, the assistant

The ring in the menu bar (or Ctrl+Super+V): click it and speak, or type. "Volume 30", "dark
mode", "put Firefox and Files side by side", "set up for coding", "why is my computer slow?",
"what's the weather?", "open Text Editor, write a short poem and save it as poem.txt".
Hearing, speaking and the small AI model run on this PC; what Siva hears is never kept. It goes
online only for look-ups (weather, news, facts, searches, from free sources) as Settings →
Assistant → Internet allows (Always, Ask or Off), and to a cloud AI only if you turn one on with
your own key. What can't be undone (closing windows, Wi-Fi, logging out, power, typing into a
terminal) asks first, and only a click says yes. Settings → Assistant → Hearing picks Accurate or
Fast speech recognition.

Siva also does real tasks: "write a python program that prints the primes below 50, save it and
run it", "list the files in my Documents folder", "open Google News and read the headlines",
"read this page aloud". Files are written in your home folder (with Undo), and commands run in
Siva's own terminal window, where you see them; deleting, sudo, sending files out and the like
ask first. A task works step by step, seeing each result, on the cloud AI when one is on (a
free Groq key works well), else on a coding model on this PC (`kompact-assistant-setup work`,
2.1 GB, slow on older CPUs; Qwen2.5-Coder 3B is under the Qwen Research License: non-commercial
use only). Siva reads inside windows through accessibility (Settings → Assistant → Permissions →
Read inside apps; restart Firefox once after turning it on); password fields are never read, and
what's in your windows is read on this PC and never sent to the cloud AI.

Typing works right away. For speech, set it up once, as yourself:

```sh
kompact-assistant-setup            # hearing, voice and understanding (~1.4 GB, once)
kompact-assistant-setup work       # optional: the coding model for tasks without a cloud AI
kompact-assistant-setup --check    # what's set up
kompact-assistant-setup --remove   # delete the downloaded models again
```

(or `sudo ./install.sh --assistant` does it while installing). Hearing and understanding use
Ubuntu's `whisper.cpp` and `llama.cpp-tools` packages, which Ubuntu 26.04 has and 24.04
doesn't; there, Siva hears nothing but still speaks and understands typed requests.

## If something goes wrong

- **Black screen or back at the login screen:** press Ctrl+Alt+F3, log in on the text console,
  and read `~/.local/state/kompact/kompact.log` (and `shell.log` next to it). Pick another
  session at the login screen to get back to your usual desktop.
- **kompact's login screen doesn't come up at boot:** Ctrl+Alt+F3, log in,
  `sudo kompact-login-screen disable`, reboot.

## Uninstall

```sh
sudo kompact-uninstall
```

This puts the old login screen back if kompact's was on and removes kompact's programs. The
packages it installed stay (`sudo apt autoremove` tidies them), and your home folder is
untouched: `~/.config/kompact` holds your kompact settings if you want to delete them, and
`kompact-assistant-setup --remove` deletes the assistant's models (run it before uninstalling).

## What's in here

- `bin/`: `kompact` (the compositor), `kompact-shell` (menu bar, dock, Spotlight, Settings),
  `kompact-lock` (lock screen), `kompact-greeter` (login screen), and the scripts that start a
  session and the login screen.
- `install.sh`, `uninstall.sh`, `login-screen.sh`: the installer and the tools it installs.
- `packages-required.txt`, `packages-optional.txt`: what the installer gets with apt.
- `VERSION`, `GLIBC` (the oldest glibc it runs on), `BUILT-ON`.

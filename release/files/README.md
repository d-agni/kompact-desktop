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
untouched: `~/.config/kompact` holds your kompact settings if you want to delete them.

## What's in here

- `bin/`: `kompact` (the compositor), `kompact-shell` (menu bar, dock, Spotlight, Settings),
  `kompact-lock` (lock screen), `kompact-greeter` (login screen), and the scripts that start a
  session and the login screen.
- `install.sh`, `uninstall.sh`, `login-screen.sh`: the installer and the tools it installs.
- `packages-required.txt`, `packages-optional.txt`: what the installer gets with apt.
- `VERSION`, `GLIBC` (the oldest glibc it runs on), `BUILT-ON`.

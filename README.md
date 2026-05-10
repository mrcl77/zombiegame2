# zombiegame2

Top-down 2D zombie survival shooter. Single player and LAN co-op for up to 4 players.

[![Release](https://github.com/mrcl77/zombiegame2/actions/workflows/release.yml/badge.svg?branch=main)](https://github.com/mrcl77/zombiegame2/actions/workflows/release.yml)

## Download

Pick your platform — links always point to the latest build:

| Platform                  | Download                                                                                                                            |
| ------------------------- | ----------------------------------------------------------------------------------------------------------------------------------- |
| **Linux** (x86_64)        | [zombiegame2-linux-x86_64.tar.gz](https://github.com/mrcl77/zombiegame2/releases/latest/download/zombiegame2-linux-x86_64.tar.gz)   |
| **macOS** (Apple Silicon) | [zombiegame2-macos-aarch64.tar.gz](https://github.com/mrcl77/zombiegame2/releases/latest/download/zombiegame2-macos-aarch64.tar.gz) |
| **Windows** (x86_64)      | [zombiegame2-windows-x86_64.zip](https://github.com/mrcl77/zombiegame2/releases/latest/download/zombiegame2-windows-x86_64.zip)     |

Unpack the archive and run `zombiegame2` (`zombiegame2.exe` on Windows). Keep the `assets/` folder next to the binary.

> **macOS:** the binary is unsigned. If Gatekeeper blocks it, run once: `xattr -dr com.apple.quarantine zombiegame2-macos-aarch64`.

## Controls

| Key            | Action                       |
| -------------- | ---------------------------- |
| `WASD` / arrows | Move                        |
| Mouse          | Aim                          |
| Left click     | Shoot                        |
| `R`            | Reload                       |
| `1` / `2` / `3` | Switch weapon               |
| `Esc`          | Pause / quit                 |

## Multiplayer (LAN)

Host opens a game on TCP port **7777**. Other players pick **JOIN LAN** and enter the host's local IP. Up to 4 players.

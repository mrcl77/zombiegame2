# zombiegame2

Top-down 2D zombie survival shooter napisany w Rust + [Bevy 0.14](https://bevyengine.org/).
Tryb single-player oraz kooperacja LAN do **4 graczy**, fale przeciwników, 11 broni
i proceduralna pikselowa grafika generowana w runtime (brak zewnętrznych assetów graficznych).

[![Release](https://github.com/mrcl77/zombiegame2/actions/workflows/release.yml/badge.svg?branch=main)](https://github.com/mrcl77/zombiegame2/actions/workflows/release.yml)

> **Status:** wczesna wersja rozwojowa (`0.1.0`). Mechaniki gameplay są stabilne,
> mapa i ekonomia broni nadal się zmieniają.

---

## Pobierz binarkę

Każdy push do `main` automatycznie buduje paczki dla trzech systemów i publikuje
je jako rolling release [`latest`](https://github.com/mrcl77/zombiegame2/releases/latest).
Linki poniżej zawsze wskazują **najnowszy** build:

| System                       | Pobierz                                                                                                                                       |
| ---------------------------- | --------------------------------------------------------------------------------------------------------------------------------------------- |
| **Linux** (x86_64)           | [zombiegame2-linux-x86_64.tar.gz](https://github.com/mrcl77/zombiegame2/releases/latest/download/zombiegame2-linux-x86_64.tar.gz)             |
| **macOS** (Apple Silicon)    | [zombiegame2-macos-aarch64.tar.gz](https://github.com/mrcl77/zombiegame2/releases/latest/download/zombiegame2-macos-aarch64.tar.gz)           |
| **Windows** (x86_64)         | [zombiegame2-windows-x86_64.zip](https://github.com/mrcl77/zombiegame2/releases/latest/download/zombiegame2-windows-x86_64.zip)               |

Po pobraniu rozpakuj archiwum i uruchom `zombiegame2` (lub `zombiegame2.exe`).
Katalog `assets/` musi pozostać obok binarki — gra ładuje z niego font HUD oraz
specyfikację mapy.

> **macOS:** binarka nie jest podpisana — przy pierwszym uruchomieniu Gatekeeper
> może ją zablokować. Odblokuj ją w *System Settings → Privacy & Security* lub
> w terminalu: `xattr -dr com.apple.quarantine zombiegame2-macos-aarch64`.

---

## Spis treści

- [Pobierz binarkę](#pobierz-binarkę)
- [Funkcjonalności](#funkcjonalności)
- [Wymagania](#wymagania)
- [Budowanie i uruchamianie](#budowanie-i-uruchamianie)
- [Sterowanie](#sterowanie)
- [Multiplayer (LAN)](#multiplayer-lan)
- [Architektura](#architektura)
- [Struktura repozytorium](#struktura-repozytorium)
- [Licencja](#licencja)

---

## Funkcjonalności

- **Tryby gry:** Single Player, Host LAN, Join LAN (do 4 graczy).
- **System fal:** rosnąca trudność, 5 typów zombie (Normal, Fast, Exploder, Burning, Giant).
- **11 broni:** Pistol, SMG, Shotgun, Rifle, Rocket Launcher, Minigun, Flamethrower,
  Sniper, Uzi, Auto Shotgun, Marksman Rifle — każda z indywidualnym cooldownem,
  obrażeniami i prędkością pocisków.
- **Pickupy:** apteczki, kamizelki (armor), mnożniki kasy (2× / 3×).
- **Achievementy** zapisywane lokalnie.
- **Proceduralna grafika 2D** (pixel art generowany przez `src/pixelart.rs`)
  oraz **proceduralne SFX** przez `bevy::audio::Pitch` (brak plików audio).
- **Konfigurowalna grafika:** rozdzielczość, tryb okna, VSync, limit FPS, jakość, licznik FPS.
- Symulacja deterministyczna w `FixedUpdate` przy **60 Hz**.

## Mapa

Wiejska mapa post-apokaliptyczna 64×48 kafli (2048×1536 px świata):

- Pojedyncza droga asfaltowa wschód-zachód przez środek mapy.
- 4 drewniane chaty z umeblowanym wnętrzem (łóżko, kuchnia, salon).
- Gęsty las wokół granic mapy (sosny, brzozy, suche drzewa).
- Wraki samochodów, autobus, ogrodzenia, studnie, paleniska, gruz.
- 6 punktów respawnu na krawędziach mapy.

---

## Wymagania

- **Rust** 1.75+ (edycja 2021)
- System operacyjny obsługujący Bevy 0.14: Linux, macOS, Windows
- GPU ze wsparciem dla `wgpu` (Vulkan / Metal / DX12)
- Sieć LAN dla trybu multiplayer (port TCP **7777**)

### Zależności (Cargo)

```toml
bevy       = "0.14"
rand       = "0.8"
serde      = { version = "1", features = ["derive"] }
bincode    = "1.3"
serde_json = "1"
```

---

## Budowanie i uruchamianie

```bash
# klon i wejście do katalogu
git clone <repo-url> zombiegame2
cd zombiegame2

# tryb developerski (szybka kompilacja, opt-level 1)
cargo run

# tryb release (zalecany do gry)
cargo run --release
```

Pierwsza kompilacja Bevy potrafi trwać kilka minut — kolejne są inkrementalne.

---

## Sterowanie

### Menu

| Klawisz             | Akcja                          |
| ------------------- | ------------------------------ |
| `W` / `S` / strzałki | Nawigacja                     |
| `Enter` / `Space`   | Zatwierdź                      |
| `Esc`               | Powrót / wyjście               |

### Rozgrywka

| Klawisz / przycisk  | Akcja                                  |
| ------------------- | -------------------------------------- |
| `WASD` / strzałki   | Ruch                                   |
| Mysz                | Celowanie                              |
| LPM                 | Strzał                                 |
| `R`                 | Przeładuj                              |
| `1` / `2` / `3`     | Wybór broni z ekwipunku                |
| `Esc`               | Pauza (single-player) / wyjście (LAN)  |
| `Q` / `M` (w pauzie) | Powrót do menu                        |

---

## Multiplayer (LAN)

- **Port:** TCP `7777` (hardcoded — `src/net.rs::NET_PORT`)
- **Maks. graczy:** 4 (`MAX_PLAYERS`)
- **Model:** server-authoritative — host prowadzi pełną symulację i co tick
  rozsyła pełne snapshoty stanu; klienci wysyłają tylko inputy.
- **Protokół:** surowe TCP + length-prefixed `bincode`, bez zewnętrznych
  bibliotek netcode.

### Hostowanie gry

1. W menu wybierz **HOST LAN**.
2. Upewnij się, że port `7777` jest otwarty / nie blokowany przez firewall.
3. Podaj graczom swój adres IP w sieci lokalnej.

### Dołączanie

1. W menu wybierz **JOIN LAN**.
2. Wpisz adres IP hosta (domyślnie `127.0.0.1`).
3. `Enter` potwierdza i przechodzi do lobby.

---

## Architektura

Gra zbudowana jest jako zestaw pluginów Bevy. Wejściem aplikacji jest `src/main.rs`.

| Moduł                | Odpowiedzialność                                              |
| -------------------- | ------------------------------------------------------------- |
| `main.rs`            | Bootstrap aplikacji, kamera, stany gry                        |
| `menu.rs` / `lobby.rs` | UI menu głównego, ustawień, lobby, ekranu „How to play”      |
| `pause.rs`           | Pauza w trybie single-player                                  |
| `player.rs`          | Sterowanie graczem, HP, kolizje, animacja postaci             |
| `weapon.rs`          | Definicje broni, pickupy, ekonomia, przeładowanie             |
| `bullet.rs`          | Pociski, hitscan, eksplozje, ogień                            |
| `zombie.rs`          | AI, pathfinding, typy zombie, spawn                           |
| `wave.rs`            | System fal, dobieranie typów, rosnąca trudność                |
| `map.rs` / `map_data.rs` | Generacja mapy, tilemap, kolizje, dekoracje, meble        |
| `net.rs`             | TCP transport, `NetMode`, alokacja `NetId`                    |
| `sync.rs`            | Snapshoty serwera, aplikacja stanu po stronie klienta         |
| `audio.rs`           | Proceduralne SFX (`bevy::audio::Pitch`)                       |
| `pixelart.rs`        | `Canvas` do generowania spritów (`put`/`fill_rect`/`fill_circle`) |
| `ui.rs`              | HUD: HP, armor, amunicja, fala, wynik                         |
| `settings.rs`        | Persystencja ustawień graficznych                             |
| `achievements.rs`    | Achievementy + zapis na dysk                                  |
| `zones.rs` / `elevator.rs` | Shimy kompatybilności po pivotie mapy                   |

### Stany gry

```
Menu → Settings | Achievements | Guide
     → JoinPrompt → Lobby → Playing → GameOver
                          ↑           ↓
                          └───────────┘
```

### Pętla symulacji

- Logika gry wykonywana jest w `FixedUpdate` z częstotliwością `TICK_HZ = 60.0`.
- Systemy gameplay są gateowane przez:
  - `is_authoritative` (host + single-player) — symulacja
  - `gameplay_active` — wstrzymanie podczas pauzy w SP

---

## Struktura repozytorium

```
.
├── Cargo.toml
├── README.md
├── assets/
│   └── fonts/
│       └── PressStart2P.ttf
└── src/
    ├── main.rs
    ├── menu.rs / lobby.rs / pause.rs / ui.rs
    ├── player.rs / weapon.rs / bullet.rs
    ├── zombie.rs / wave.rs
    ├── map.rs / map_data.rs / zones.rs / elevator.rs
    ├── net.rs / sync.rs
    ├── audio.rs / pixelart.rs
    ├── settings.rs
    └── achievements.rs
```

---

## Licencja

Projekt prywatny — licencja nieokreślona. Skontaktuj się z autorem
przed użyciem komercyjnym lub redystrybucją.

<p align="center">
  <img src="assets/logo.png" alt="Purrch cat logo" width="120">
</p>

<h1 align="center">Purrch</h1>

<p align="center">A cat that lives in your notch.</p>

<p align="center">
  <a href="https://github.com/Shaiyon69/purrch/releases/latest"><img src="https://img.shields.io/github/v/release/Shaiyon69/purrch" alt="Latest release"></a>
  <a href="LICENSE"><img src="https://img.shields.io/github/license/Shaiyon69/purrch" alt="MIT license"></a>
  <img src="https://img.shields.io/badge/platform-Windows%2010%20%7C%2011-0078D4" alt="Windows 10 | 11">
</p>

<p align="center">
  <a href="#install">Install</a> · <a href="#build">Build</a> · <a href="#your-data">Your data</a>
</p>

Purrch sits at the top of your screen as a small pill and opens into a panel when you hover over it.

## Features

- **Water**: spreads your daily goal into small sips across your working hours, nudges you when one is due, and tells you when your bottle is probably empty. Snooze, a bottle tracker and an hourly chart are included.
- **Focus**: a Pomodoro timer. 25 minutes of focus, 5-minute breaks, and a 15-minute break after every fourth session.
- **To-do**: a quick task list. Add `@14:30` to a task to get a reminder at that time.
- **Shelf**: drop files, links or text on the notch to keep them handy, then drag them back out into any app.
- **Music**: shows what's playing in any app that reports to Windows media controls, with album art, controls and synced lyrics from [LRCLIB](https://lrclib.net).
- **Pop-ups**: charger plugged in or out, low battery, Caps Lock, internet lost or back, Bluetooth devices connecting.
- **The cat**: follows your cursor with its eyes and falls asleep when you step away.
- **Stays out of the way**: the notch hides itself while a full-screen app is in front.

### Tray

Right-click the tray icon for *Start with Windows*, *Show the cat*, *Compact notch* and *Quit*.

## Install

Download `PurrchSetup.exe` from the [latest release](https://github.com/Shaiyon69/purrch/releases/latest) and run it. It installs for your user only, with no admin prompt, and adds Purrch to the Start menu. Windows 10 or 11, 64-bit.

> [!NOTE]
> The installer is not code-signed yet, so Windows SmartScreen may warn you. Click *More info*, then *Run anyway*.

## Build

Requires Windows 10 or 11 and Rust with the MSVC toolchain (including the Windows SDK, which embeds the icon).

```powershell
cargo build --release
cargo test
```

The app is `target\release\purrch.exe`. To build the installer, install [Inno Setup 6](https://jrsoftware.org/isinfo.php) and run:

```powershell
iscc installer.iss
```

It writes `dist\PurrchSetup.exe`.

## Your data

Everything stays on your machine, as plain text in `%APPDATA%\Purrch\`:

| File | Contents |
|---|---|
| `settings.txt` | cat and compact-mode toggles |
| `water.txt` | bottle, goal, hours and today's progress |
| `todo.txt` | tasks, one per line |
| `shelf.txt` | paths of shelved items |

Dropped links and text snippets are saved as files in `%LOCALAPPDATA%\Purrch\Shelf\`.

> [!TIP]
> The only network request Purrch makes is the lyrics lookup to lrclib.net, which sends the song title and artist.

## License

MIT. See [LICENSE](LICENSE).

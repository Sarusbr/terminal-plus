# Terminal Plus

Lightweight SSH + local terminal with visual panels (files, Docker, system, processes,
ports, logs, services). Tauri v2 (Rust) + Vue 3 + xterm.js. Nothing is installed on the
server: panels type standard commands (`ls`, `docker`, `ps`, …) into your own shell.

## İndir

| Platform | Dosya |
| --- | --- |
| macOS — Apple Silicon (M1/M2/M3/M4) | [TerminalPlus-macos-apple-silicon.dmg](https://github.com/Sarusbr/terminal-plus/releases/latest/download/TerminalPlus-macos-apple-silicon.dmg) |
| macOS — Intel | [TerminalPlus-macos-intel.dmg](https://github.com/Sarusbr/terminal-plus/releases/latest/download/TerminalPlus-macos-intel.dmg) |
| Windows (kurulum) | [TerminalPlus-windows-x64-setup.exe](https://github.com/Sarusbr/terminal-plus/releases/latest/download/TerminalPlus-windows-x64-setup.exe) |
| Windows (MSI) | [TerminalPlus-windows-x64.msi](https://github.com/Sarusbr/terminal-plus/releases/latest/download/TerminalPlus-windows-x64.msi) |
| Linux — AppImage | [TerminalPlus-linux-x64.AppImage](https://github.com/Sarusbr/terminal-plus/releases/latest/download/TerminalPlus-linux-x64.AppImage) |
| Linux — Debian/Ubuntu | [TerminalPlus-linux-x64.deb](https://github.com/Sarusbr/terminal-plus/releases/latest/download/TerminalPlus-linux-x64.deb) |

Tüm sürümler: [Releases](https://github.com/Sarusbr/terminal-plus/releases)

### İlk açılış (imzasız uygulama)

Uygulama henüz Apple / Microsoft sertifikasıyla imzalı değil, bu yüzden ilk açılışta uyarı çıkar:

- **macOS:** `.dmg`'yi açıp uygulamayı *Applications*'a sürükle. İlk açılışta uyarı gelirse
  *Sistem Ayarları → Gizlilik ve Güvenlik → "Yine de Aç"*. Alternatif olarak Terminal'de:
  `xattr -dr com.apple.quarantine "/Applications/Terminal Plus.app"`
- **Windows:** SmartScreen "Windows bilgisayarınızı korudu" derse *Ek bilgi → Yine de çalıştır*.

### Yeni sürüm yayınlama

```bash
git tag v0.1.1
git push origin v0.1.1
```

GitHub Actions tüm platformları derler ve dosyaları release'e yükler; yukarıdaki linkler
otomatik olarak en yeni sürümü indirir.

## Develop

```bash
npm install
npm run tauri dev
```

Requires Rust (stable) and Node 20+. On Linux also the Tauri system packages
(`libwebkit2gtk-4.1-dev`, `libssl-dev`, `librsvg2-dev`, …).

## Build installers

Each platform is built on that platform:

```bash
npm run tauri build -- --bundles app,dmg      # macOS
npm run tauri build -- --bundles msi,nsis     # Windows
npm run tauri build -- --bundles appimage,deb # Linux
```

## Tests

```bash
cd src-tauri && cargo test --lib
```

The SSH integration test needs a reachable sshd with password auth:

```bash
cd src-tauri && TP_TEST_SSH=127.0.0.1:2222:user:password cargo test --lib -- --include-ignored
```

## How panels talk to the shell

`src/lib/runner.ts` is the only place panel commands are sent from. Each command is
wrapped in unique markers, its output is captured and hidden from xterm, and the prompt is
redrawn afterwards. Commands never enter shell history:

- **zsh**: a key sequence switches ZLE to a private keymap that collects the command into
  a variable and `eval`s it — no line is accepted, so nothing is recorded.
- **bash/sh**: a space-prefixed line with `HISTCONTROL=ignorespace`.

Local shells get this setup at startup through a wrapper `ZDOTDIR` (zsh) or `--rcfile`
(bash) that sources your normal startup files first. Remote shells get a one-line setup
on first panel use (never on connect); it removes itself from history.

Panels are disabled while a full-screen program (vim, htop, less …) is running, and for
local tabs also while any foreground program runs.

## Layout

| Path | What |
| --- | --- |
| `src-tauri/src/ssh.rs` | SSH connections, PTY channels, known_hosts |
| `src-tauri/src/local.rs` | Local shell PTY + shell integration |
| `src-tauri/src/transfer.rs` | Upload/download (SFTP channel only during transfer) |
| `src-tauri/src/fsops.rs` | Editor file read/write |
| `src-tauri/src/secrets.rs` | OS keychain |
| `src/lib/runner.ts`, `src/lib/shell.ts` | Panel command service |
| `src/pages/registry.ts` | Page modules — add a page here |

## Known limits

- Panels don't work in fish or in Windows' local PowerShell.
- Dragging files from the list out to the desktop isn't supported; use the download button.
- zsh prompts that depend on `precmd` (e.g. git branch) refresh after your next command.

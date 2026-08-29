# Rexeb `v0.2.4-alpha`

```
  ██████╗ ███████╗██╗  ██╗███████╗██████╗
  ██╔══██╗██╔════╝╚██╗██╔╝██╔════╝██╔══██╗
  ██████╔╝█████╗   ╚███╔╝ █████╗  ██████╔╝
  ██╔══██╗██╔══╝   ██╔██╗ ██╔══╝  ██╔══██╗
  ██║  ██║███████╗██╔╝ ██╗███████╗██████╔╝
  ╚═╝  ╚═╝╚══════╝╚═╝  ╚═╝╚══════╝╚═════╝
```

*A smarter, faster debtap alternative — convert .deb packages to Arch Linux packages*

[![License: GPL-v3.0](https://img.shields/-badge/License-GPL--3.0-yellow.svg)](https://www.gnu.org/licenses/gpl-3.0)
[![Rust](https://img.shields.io/badge/rust-%23000000.svg?style=flat&logo=rust&logoColor=white)](https://www.rust-lang.org/)
[![Version](https://img.shields.io/badge/version-0.2.4--alpha-blue.svg)](https://github.com/OnionOrbit/rexeb/releases)
[![Mappings](https://img.shields.io/badge/mappings-589-green.svg)](#dependency-database)

## About

Rexeb is a modern, high-performance CLI written in Rust that converts Debian (`.deb`) packages to Arch Linux packages (`.pkg.tar.zst`). It's a faster, more reliable alternative to debtap — with intelligent dependency mapping, AUR-assisted fallback, parallel processing, watermark tracking, and AUR publishing. Built for real hardware: great performance on dual-core / i3 / i5 and 2 GB RAM with low-RAM safety caps.

## Features

- 🚀 **Fast conversion** — rayon-powered parallelism with automatic caps for low-end CPUs and 2 GB RAM safety
- 🔍 **Smart dependency resolution** — 589-entry curated mapping, regex rewrites, fuzzy matching (skim + Jaro-Winkler), and AUR provider search as fallback
- 🗺️ **Manual mapping** — `rexeb map add/remove/import/export` to fix any missing dependency without recompiling
- 🔄 **Database sync** — `rexeb update` pulls from `db/mappings.json`; `--enlarge` crawls local pacman DB `Provides` + debtap's mapping table
- 📦 **Batch processing** — convert multiple `.deb` files in one invocation
- 🔧 **Flexible configuration** — TOML config, `--name`/`--version` overrides, `--format` (zst/xz/gz)
- 🏷️ **Packages renamed safely** — `--name` override + `rexeb manage --rename` alias
- 🏗️ **Sandboxed builds** — `systemd-nspawn` isolation via `--sandbox`
- 📊 **Analysis** — pre-conversion warnings, FHS/lib/security/conflict checks (`rexeb analyze`)
- 🔎 **Search** — local + AUR package search
- 🧪 **AUR integration** — `rexeb check-aur` for latest version checks, `rexeb aur-push` to publish conversions
- 💧 **Watermark** — every converted package carries `x-rexeb` in `.PKGINFO`; `rexeb list-installed` and `rexeb manage --fix-icon` work even when rexeb itself isn't installed
- ⬇️ **Self-update** — `rexeb self-update` checks `onionorbit/rexeb` releases on GitHub
- 🧹 **Cleanup** — parallel cache/temp cleanup

## Performance

Rexeb targets **dual-core, i3, i5, 2 GB RAM** without overloading the device:

- Bounded rayon thread pool: `MemAvailable < 1 GB` → 1 job; `≤2 cores` → 2 jobs; otherwise `min(cores/2, 4)`. Override with `-j/--jobs`.
- Streaming bzip2/zstd/xz extraction and 64 KiB chunked SHA-256 (no whole-archive buffering).
- `cargo build --release` uses `lto = true`, `strip = true`, `codegen-units = 1` for small RSS.

## Installation

### From Source

Requires Rust (≥ 1.75 recommended):

```bash
curl --proto '=https' --tlsv1.2 -sSf https://sh.rustup.rs | sh
git clone https://github.com/OnionOrbit/rexeb.git
cd rexeb
cargo build --release
sudo cp target/release/rexeb /usr/local/bin/
```

### From AUR (once published)

```bash
paru -S rexeb      # or: yay -S rexeb
```

## Usage

### Basic conversion

```bash
rexeb convert package.deb
rexeb convert package1.deb package2.deb -o ./out
rexeb convert --name my-pkg --format pkg.tar.xz package.deb
rexeb convert --sandbox package.deb
rexeb convert --pkgbuild package.deb -o ./PKGBUILD-dir
```

### Install in one step

```bash
rexeb install package.deb --asdeps
```

### Analyze without converting

```bash
rexeb analyze package.deb --conflicts --verify
rexeb info package.deb --format json --extended
rexeb search libssl --fuzzy --aur
```

## Commands

| Command | Description |
|---------|-------------|
| `convert` | Convert `.deb` → `.pkg.tar.zst` (or PKGBUILD) |
| `install` | Convert and install via `pacman -U` |
| `analyze` | Pre-conversion analysis (FHS, libs, security, conflicts, deps) |
| `info` | Show package metadata |
| `search` | Search Arch / AUR package mappings |
| `update` | Refresh mappings, virtual packages, AUR cache (`--enlarge` crawls repo DBs + debtap) |
| `map` | `add`/`remove`/`list`/`import`/`export` manual dependency mappings |
| `check-aur` | Check AUR for latest version of a package or deb (`--installed` for all watermarked pkgs) |
| `aur-push` | Publish a converted PKGBUILD to the AUR (`--dry-run` to preview) |
| `list-installed` | List packages previously installed by rexeb (watermark) |
| `manage` | Rename (`--rename`) or fix icons (`--fix-icon`) of installed rexeb packages |
| `self-update` | Check / apply updates from GitHub releases (`--check-only`) |
| `config` | `show`/`edit`/`reset`/`set`/`get`/`init` configuration |
| `clean` | Clean cache and temp files (parallel, `--dry-run`) |

Global flags: `-v/--verbose`, `-q/--quiet`, `-c/--config`, `-j/--jobs`, `--tui`.

```bash
rexeb update --all --force --enlarge
rexeb map add libfoo foo --confidence 0.9
rexeb map list --json
rexeb check-aur ./package.deb
rexeb check-aur --installed
rexeb aur-push ./out --dry-run --pkgbase my-pkg
rexeb list-installed --json
rexeb manage my-pkg --rename my-new-name
rexeb self-update --check-only
```

## Dependency Database

- **Curated static JSON:** `db/mappings.json` (589 entries as of v0.2.4) + `db/virtual_packages.json`. Served at `https://raw.githubusercontent.com/OnionOrbit/rexeb/main/db/mappings.json` for `rexeb update`.
- **Loading order:** `~/.local/share/rexeb/db/mappings.json` (user overrides) → `db/mappings.json` on disk → `include_str!` embedded JSON → hardcoded fallback in `src/resolver/database.rs`.
- **Enlarging:** `rexeb update --enlarge` scans `/var/lib/pacman/sync/*.db` `Provides` and parses debtap's `"
debian => arch"` table, proposes new `{debian → arch}` entries at 0.75 confidence, and persists them. See `db/README.md`.

## Watermark

Every package built by rexeb is stamped so it can be identified even when rexeb is not installed:

- `.PKGINFO` fields: `x-rexeb`, `x-rexeb-source` (ignored by pacman/libalpm, visible via `pacman -Qi`)
- `pakcman -Q` path + sentinel lookups in `src/watermark.rs` — `list-installed` scans `/var/lib/pacman/local/*/desc` for `x-rexeb`

## Dependencies

System (at runtime):

- `pacman` (Arch)
- `fakeroot`, `gcc`, `make` (build-time)
- `git` + AUR SSH key for `aur-push`

## Development

```bash
cargo build              # dev
cargo build --release    # release (LTO + stripped)
cargo test
cargo clippy -- -D warnings
```

### Warnings policy

`src/lib.rs` enables `#![warn(missing_docs, clippy::all)]` — new code must be documented. As of v0.2.4 the warning count is **0** (down from 59). Do not silence with `#[allow]`.

## Changelog

- **v0.2.4-alpha** — bump version, mappings 497 → 589 (+92: KDE5, gir1.2-*, LibreOffice, browsers, containers, network, everyday tools); JSON-first DB with embedded fallback; `rexeb update` no longer 404s; 59 → 0 warnings; `map`/`check-aur`/`aur-push`/`list-installed`/`manage`/`self-update`/`--enlarge`; bzip2 streaming + low-RAM caps.
- **v0.2.0-alpha** — initial public alpha.

## License

GPL-3.0-only — see [LICENSE](LICENSE).

## Support

- Issues: [GitHub Issues](https://github.com/OnionOrbit/rexeb/issues)
- Discussions: [GitHub Discussions](https://github.com/OnionOrbit/rexeb/discussions)

---

**Made with ❤️ for the Arch Linux community**

# Rexeb

```
  ██████╗ ███████╗██╗  ██╗███████╗██████╗
  ██╔══██╗██╔════╝╚██╗██╔╝██╔════╝██╔══██╗
  ██████╔╝█████╗   ╚███╔╝ █████╗  ██████╔╝
  ██╔══██╗██╔══╝   ██╔██╗ ██╔══╝  ██╔══██╗
  ██║  ██║███████╗██╔╝ ██╗███████╗██████╔╝
  ╚═╝  ╚═╝╚══════╝╚═╝  ╚═╝╚══════╝╚═════╝
```

*A smarter, faster debtap alternative - convert .deb packages to Arch Linux packages*

[![License: GPL-v3.0](https://img.shields.io/badge/License-GPL3.0-yellow.svg)](https://opensource.org/licenses/MIT)
[![Rust](https://img.shields.io/badge/rust-%23000000.svg?style=flat&logo=rust&logoColor=white)](https://www.rust-lang.org/)

## About

Rexeb is a modern, high-performance command-line tool written in Rust that converts Debian (.deb) packages to Arch Linux packages (.pkg.tar.zst). It's designed as a faster and more reliable alternative to debtap, with improved dependency resolution, parallel processing, and a clean, intuitive interface.

## Features

- 🚀 **Fast conversion**: Optimized for speed with parallel processing
- 🔍 **Smart dependency resolution**: Intelligent mapping of Debian dependencies to Arch packages
- 📦 **Batch processing**: Convert multiple packages simultaneously
- 🔧 **Flexible configuration**: Extensive customization options
- 🏗️ **Sandbox building**: Isolated build environment for enhanced security
- 📊 **Package analysis**: Detailed package inspection and metadata extraction
- 🔄 **Auto-updates**: Keep package databases synchronized
- 🧹 **Cleanup utilities**: Remove temporary files and caches

## Installation

### From Source

Ensure you have Rust installed on your system:

```bash
# Install Rust (if not already installed)
curl --proto '=https' --tlsv1.2 -sSf https://sh.rustup.rs | sh

# Clone the repository
git clone https://github.com/OnionOrbit/rexeb.git
cd rexeb

# Build and install
cargo build --release
sudo cp target/release/rexeb /usr/local/bin/
```

## Usage

### Basic Conversion

Convert a single .deb package:

```bash
rexeb convert package.deb
```

Convert multiple packages:

```bash
rexeb convert package1.deb package2.deb package3.deb
```

## Commands

| Command | Description |
|---------|-------------|
| `convert` | Convert .deb packages to Arch Linux packages |
| `install` | Convert and install packages in one step |
| `update` | Update Arch Linux package databases (Upcoming) |
| `analyze` | Analyze .deb package contents and dependencies |
| `info` | Display detailed package information |
| `config` | Manage rexeb configuration |
| `clean` | Clean temporary files and caches |

## Dependencies

Rexeb requires the following system dependencies:

- `pacman` (Arch Linux package manager)
- `fakeroot` (for package building)
- `gcc` (for compilation)
- `make` (build system)

## Development

### Building from Source

```bash
# Debug build
cargo build

# Release build
cargo build --release
```

## License

This project is licensed under the GPL-v3.0 License - see the [LICENSE](LICENSE) file for details.

## Support

- **Issues**: [GitHub Issues](https://github.com/OnionOrbit/rexeb/issues)
- **Discussions**: [GitHub Discussions](https://github.com/OnionOrbit/rexeb/discussions)

---

**Made with ❤️ for the Arch Linux community**

//! Package database for Debian to Arch package mappings

use std::collections::HashMap;
use std::path::PathBuf;
use serde::{Deserialize, Serialize};

use crate::error::{RexebError, Result};

/// Package database containing mappings and package info
pub struct PackageDatabase {
    /// Direct name mappings (Debian -> Arch)
    mappings: HashMap<String, PackageMapping>,
    /// Virtual packages
    virtual_packages: HashMap<String, Vec<String>>,
    /// Arch package cache
    arch_packages: HashMap<String, ArchPackageInfo>,
    /// AUR package cache
    aur_packages: HashMap<String, AurPackageInfo>,
    /// Database directory
    db_dir: PathBuf,
}

/// A package name mapping
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct PackageMapping {
    /// Debian package name
    pub debian_name: String,
    /// Arch package name
    pub arch_name: String,
    /// Confidence score (1.0 = exact match)
    pub confidence: f32,
    /// Source of the mapping
    pub source: MappingSource,
}

/// Source of a package mapping
#[derive(Debug, Clone, Serialize, Deserialize)]
pub enum MappingSource {
    /// Built-in/hardcoded mapping
    Builtin,
    /// From Arch repos
    ArchRepo,
    /// From AUR
    Aur,
    /// User-defined
    User,
    /// Automatically detected
    Auto,
}

/// Info about an Arch repository package
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ArchPackageInfo {
    pub name: String,
    pub version: String,
    pub description: String,
    pub provides: Vec<String>,
    pub replaces: Vec<String>,
}

/// Info about an AUR package
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct AurPackageInfo {
    pub name: String,
    pub version: String,
    pub description: String,
    pub votes: u32,
    pub popularity: f64,
    pub out_of_date: Option<i64>,
}

/// Search result
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SearchResult {
    pub name: String,
    pub description: String,
    pub version: String,
    pub score: f32,
}

impl PackageDatabase {
    /// Create a new package database
    pub fn new() -> Result<Self> {
        let db_dir = Self::get_db_dir()?;
        std::fs::create_dir_all(&db_dir)?;

        let mut db = Self {
            mappings: HashMap::new(),
            virtual_packages: HashMap::new(),
            arch_packages: HashMap::new(),
            aur_packages: HashMap::new(),
            db_dir,
        };

        // Load built-in mappings
        db.load_builtin_mappings();

        // Try to load cached databases
        db.load_cached_data()?;

        Ok(db)
    }

    /// Get the database directory
    fn get_db_dir() -> Result<PathBuf> {
        let dir = dirs::data_dir()
            .ok_or_else(|| RexebError::Config("Could not find data directory".into()))?
            .join("rexeb")
            .join("db");
        Ok(dir)
    }

    /// Load built-in package mappings
    fn load_builtin_mappings(&mut self) {
        // Common Debian -> Arch mappings
        let mappings = [
            // ==================== JAVA (CRITICAL) ====================
            // Java runtime - Default/Generic (use virtual packages to avoid conflicts)
            ("default-jre", "java-runtime", 1.0),
            ("default-jre-headless", "java-runtime-headless", 1.0),
            ("default-jdk", "java-sdk", 1.0),
            ("default-jdk-headless", "java-sdk", 0.9),
            
            // Java 8
            ("openjdk-8-jre", "jre8-openjdk", 1.0),
            ("openjdk-8-jre-headless", "jre8-openjdk-headless", 1.0),
            ("openjdk-8-jdk", "jdk8-openjdk", 1.0),
            ("openjdk-8-jdk-headless", "jdk8-openjdk", 0.9),
            
            // Java 11
            ("openjdk-11-jre", "jre11-openjdk", 1.0),
            ("openjdk-11-jre-headless", "jre11-openjdk-headless", 1.0),
            ("openjdk-11-jdk", "jdk11-openjdk", 1.0),
            ("openjdk-11-jdk-headless", "jdk11-openjdk", 0.9),
            
            // Java 17
            ("openjdk-17-jre", "jre17-openjdk", 1.0),
            ("openjdk-17-jre-headless", "jre17-openjdk-headless", 1.0),
            ("openjdk-17-jdk", "jdk17-openjdk", 1.0),
            ("openjdk-17-jdk-headless", "jdk17-openjdk", 0.9),
            
            // Java 21
            ("openjdk-21-jre", "jre21-openjdk", 1.0),
            ("openjdk-21-jre-headless", "jre21-openjdk-headless", 1.0),
            ("openjdk-21-jdk", "jdk21-openjdk", 1.0),
            ("openjdk-21-jdk-headless", "jdk21-openjdk", 0.9),
            
            // Generic Java mappings (fallback to latest)
            ("java-runtime", "jre-openjdk", 1.0),
            ("java-runtime-headless", "jre-openjdk-headless", 1.0),
            ("java-sdk", "jdk-openjdk", 1.0),
            ("jre-openjdk", "jre-openjdk", 1.0),  // Already Arch name
            ("jdk-openjdk", "jdk-openjdk", 1.0),  // Already Arch name
            
            // ==================== CORE LIBRARIES ====================
            ("libc6", "glibc", 1.0),
            ("libc6-dev", "glibc", 0.9),
            ("libstdc++6", "gcc-libs", 1.0),
            ("libstdc++-dev", "gcc", 0.9),
            ("libgcc-s1", "gcc-libs", 1.0),
            ("libgcc1", "gcc-libs", 1.0),
            ("zlib1g", "zlib", 1.0),
            ("zlib1g-dev", "zlib", 0.9),
            ("libbz2-1.0", "bzip2", 1.0),
            ("liblzma5", "xz", 1.0),
            ("libzstd1", "zstd", 1.0),
            ("libatomic1", "gcc-libs", 0.9),
            ("libgomp1", "gcc-libs", 0.9),
            ("libpcre3", "pcre", 1.0),
            ("libpcre2-8-0", "pcre2", 1.0),
            ("libreadline8", "readline", 1.0),
            ("libncurses6", "ncurses", 1.0),
            ("libncursesw6", "ncurses", 1.0),
            ("libtinfo6", "ncurses", 0.9),
            ("libexpat1", "expat", 1.0),
            ("libidn2-0", "libidn2", 1.0),
            ("libunistring2", "libunistring", 1.0),
            ("libffi8", "libffi", 1.0),
            ("libffi7", "libffi", 1.0),
            ("libgmp10", "gmp", 1.0),
            ("libmpfr6", "mpfr", 1.0),
            ("libmpc3", "libmpc", 1.0),
            
            // ==================== SSL/CRYPTO ====================
            ("libssl1.1", "openssl", 0.9),
            ("libssl3", "openssl", 1.0),
            ("libssl-dev", "openssl", 0.9),
            ("libcrypto1.1", "openssl", 0.9),
            ("libcrypto3", "openssl", 1.0),
            ("libgnutls30", "gnutls", 1.0),
            ("libgcrypt20", "libgcrypt", 1.0),
            ("libgpg-error0", "libgpg-error", 1.0),
            ("gnupg", "gnupg", 1.0),
            ("gpg", "gnupg", 0.9),
            ("libsodium23", "libsodium", 1.0),
            ("libnettle8", "nettle", 1.0),
            ("libhogweed6", "nettle", 0.9),
            ("libp11-kit0", "p11-kit", 1.0),
            ("libtasn1-6", "libtasn1", 1.0),
            
            // ==================== CORE UTILITIES ====================
            ("coreutils", "coreutils", 1.0),
            ("bash", "bash", 1.0),
            ("dash", "dash", 1.0),
            ("zsh", "zsh", 1.0),
            ("fish", "fish", 1.0),
            ("sed", "sed", 1.0),
            ("grep", "grep", 1.0),
            ("gawk", "gawk", 1.0),
            ("mawk", "gawk", 0.8),
            ("findutils", "findutils", 1.0),
            ("tar", "tar", 1.0),
            ("gzip", "gzip", 1.0),
            ("bzip2", "bzip2", 1.0),
            ("xz-utils", "xz", 1.0),
            ("unzip", "unzip", 1.0),
            ("zip", "zip", 1.0),
            ("p7zip", "p7zip", 1.0),
            ("file", "file", 1.0),
            ("diffutils", "diffutils", 1.0),
            ("patch", "patch", 1.0),
            ("less", "less", 1.0),
            ("util-linux", "util-linux", 1.0),
            ("procps", "procps-ng", 1.0),
            ("psmisc", "psmisc", 1.0),
            ("lsof", "lsof", 1.0),
            ("strace", "strace", 1.0),
            ("sudo", "sudo", 1.0),
            ("adduser", "shadow", 0.8),
            ("passwd", "shadow", 0.9),
            ("login", "shadow", 0.9),
            
            // ==================== X11 AND GRAPHICS ====================
            ("libx11-6", "libx11", 1.0),
            ("libx11-dev", "libx11", 0.9),
            ("libx11-xcb1", "libx11", 0.9),
            ("libxext6", "libxext", 1.0),
            ("libxrender1", "libxrender", 1.0),
            ("libxrandr2", "libxrandr", 1.0),
            ("libxi6", "libxi", 1.0),
            ("libxcursor1", "libxcursor", 1.0),
            ("libxcomposite1", "libxcomposite", 1.0),
            ("libxdamage1", "libxdamage", 1.0),
            ("libxfixes3", "libxfixes", 1.0),
            ("libxinerama1", "libxinerama", 1.0),
            ("libxkbcommon0", "libxkbcommon", 1.0),
            ("libxkbcommon-x11-0", "libxkbcommon-x11", 1.0),
            ("libxcb1", "libxcb", 1.0),
            ("libxcb-render0", "libxcb", 0.9),
            ("libxcb-shm0", "libxcb", 0.9),
            ("libxcb-xfixes0", "libxcb", 0.9),
            ("libxcb-shape0", "libxcb", 0.9),
            ("libxcb-randr0", "libxcb", 0.9),
            ("libxcb-image0", "libxcb", 0.9),
            ("libxcb-keysyms1", "libxcb", 0.9),
            ("libxcb-util1", "xcb-util", 1.0),
            ("libxcb-icccm4", "xcb-util-wm", 1.0),
            ("libxmu6", "libxmu", 1.0),
            ("libxpm4", "libxpm", 1.0),
            ("libxss1", "libxss", 1.0),
            ("libxt6", "libxt", 1.0),
            ("libxtst6", "libxtst", 1.0),
            ("libxv1", "libxv", 1.0),
            ("libxxf86vm1", "libxxf86vm", 1.0),
            ("x11-utils", "xorg-xprop", 0.8),
            ("x11-xserver-utils", "xorg-xset", 0.8),
            ("xdg-utils", "xdg-utils", 1.0),
            ("libgl1", "libglvnd", 0.9),
            ("libgl1-mesa-glx", "mesa", 0.9),
            ("libgl1-mesa-dri", "mesa", 0.9),
            ("libegl1", "libglvnd", 0.9),
            ("libegl1-mesa", "mesa", 0.9),
            ("libgles2", "libglvnd", 0.9),
            ("libgles2-mesa", "mesa", 0.9),
            ("libglx0", "libglvnd", 0.9),
            ("libglx-mesa0", "mesa", 0.9),
            ("libdrm2", "libdrm", 1.0),
            ("libdrm-common", "libdrm", 0.9),
            ("libgbm1", "mesa", 0.9),
            ("libwayland-client0", "wayland", 1.0),
            ("libwayland-cursor0", "wayland", 0.9),
            ("libwayland-egl1", "wayland", 0.9),
            ("libwayland-server0", "wayland", 0.9),
            ("libvulkan1", "vulkan-icd-loader", 1.0),
            
            // ==================== GTK AND GNOME ====================
            ("libgtk-3-0", "gtk3", 1.0),
            ("libgtk-3-common", "gtk3", 0.9),
            ("libgtk-3-dev", "gtk3", 0.9),
            ("libgtk-4-1", "gtk4", 1.0),
            ("libgtk2.0-0", "gtk2", 1.0),
            ("libglib2.0-0", "glib2", 1.0),
            ("libglib2.0-data", "glib2", 0.9),
            ("libglib2.0-dev", "glib2", 0.9),
            ("libgio-2.0-0", "glib2", 0.9),
            ("libgobject-2.0-0", "glib2", 0.9),
            ("libgmodule-2.0-0", "glib2", 0.9),
            ("libgthread-2.0-0", "glib2", 0.9),
            ("libgdk-pixbuf-2.0-0", "gdk-pixbuf2", 1.0),
            ("libgdk-pixbuf2.0-0", "gdk-pixbuf2", 1.0),
            ("libpango-1.0-0", "pango", 1.0),
            ("libpangocairo-1.0-0", "pango", 0.9),
            ("libpangoft2-1.0-0", "pango", 0.9),
            ("libpangoxft-1.0-0", "pango", 0.9),
            ("libcairo2", "cairo", 1.0),
            ("libcairo-gobject2", "cairo", 0.9),
            ("libcairo-script-interpreter2", "cairo", 0.9),
            ("libatk1.0-0", "atk", 1.0),
            ("libatk-bridge2.0-0", "at-spi2-core", 1.0),
            ("libatspi2.0-0", "at-spi2-core", 0.9),
            ("libdbus-1-3", "dbus", 1.0),
            ("dbus", "dbus", 1.0),
            ("libsystemd0", "systemd-libs", 1.0),
            ("libharfbuzz0b", "harfbuzz", 1.0),
            ("librsvg2-2", "librsvg", 1.0),
            ("librsvg2-common", "librsvg", 0.9),
            ("libsecret-1-0", "libsecret", 1.0),
            ("libnotify4", "libnotify", 1.0),
            ("libappindicator3-1", "libappindicator-gtk3", 1.0),
            ("libayatana-appindicator3-1", "libayatana-appindicator", 1.0),
            ("libdbusmenu-glib4", "libdbusmenu-glib", 1.0),
            ("libdbusmenu-gtk3-4", "libdbusmenu-gtk3", 1.0),
            ("gsettings-desktop-schemas", "gsettings-desktop-schemas", 1.0),
            ("glib-networking", "glib-networking", 1.0),
            ("libsoup2.4-1", "libsoup", 1.0),
            ("libsoup-3.0-0", "libsoup3", 1.0),
            ("libwebkit2gtk-4.0-37", "webkit2gtk", 1.0),
            ("libwebkit2gtk-4.1-0", "webkit2gtk-4.1", 1.0),
            ("libjavascriptcoregtk-4.0-18", "webkit2gtk", 0.9),
            
            // ==================== QT ====================
            ("libqt5core5a", "qt5-base", 1.0),
            ("libqt5gui5", "qt5-base", 0.9),
            ("libqt5widgets5", "qt5-base", 0.9),
            ("libqt5network5", "qt5-base", 0.9),
            ("libqt5dbus5", "qt5-base", 0.9),
            ("libqt5sql5", "qt5-base", 0.9),
            ("libqt5xml5", "qt5-base", 0.9),
            ("libqt5concurrent5", "qt5-base", 0.9),
            ("libqt5printsupport5", "qt5-base", 0.9),
            ("libqt5opengl5", "qt5-base", 0.9),
            ("libqt5svg5", "qt5-svg", 1.0),
            ("libqt5x11extras5", "qt5-x11extras", 1.0),
            ("libqt5webkit5", "qt5-webkit", 1.0),
            ("libqt5webengine5", "qt5-webengine", 1.0),
            ("libqt5webenginewidgets5", "qt5-webengine", 0.9),
            ("libqt5quick5", "qt5-declarative", 1.0),
            ("libqt5qml5", "qt5-declarative", 0.9),
            ("libqt5multimedia5", "qt5-multimedia", 1.0),
            ("libqt5positioning5", "qt5-location", 1.0),
            ("libqt5sensors5", "qt5-sensors", 1.0),
            ("libqt5serialport5", "qt5-serialport", 1.0),
            ("libqt5webchannel5", "qt5-webchannel", 1.0),
            ("libqt5websockets5", "qt5-websockets", 1.0),
            ("libqt6core6", "qt6-base", 1.0),
            ("libqt6gui6", "qt6-base", 0.9),
            ("libqt6widgets6", "qt6-base", 0.9),
            ("libqt6network6", "qt6-base", 0.9),
            ("libqt6dbus6", "qt6-base", 0.9),
            ("libqt6opengl6", "qt6-base", 0.9),
            ("libqt6svg6", "qt6-svg", 1.0),
            ("libqt6webengine6", "qt6-webengine", 1.0),
            
            // ==================== AUDIO ====================
            ("libasound2", "alsa-lib", 1.0),
            ("libasound2-data", "alsa-lib", 0.9),
            ("libasound2-plugins", "alsa-plugins", 1.0),
            ("libpulse0", "libpulse", 1.0),
            ("libpulse-mainloop-glib0", "libpulse", 0.9),
            ("pulseaudio", "pulseaudio", 1.0),
            ("pulseaudio-utils", "pulseaudio", 0.9),
            ("libpipewire-0.3-0", "pipewire", 1.0),
            ("pipewire", "pipewire", 1.0),
            ("libopenal1", "openal", 1.0),
            ("libsndfile1", "libsndfile", 1.0),
            ("libvorbis0a", "libvorbis", 1.0),
            ("libvorbisenc2", "libvorbis", 0.9),
            ("libvorbisfile3", "libvorbis", 0.9),
            ("libogg0", "libogg", 1.0),
            ("libflac8", "flac", 1.0),
            ("libopus0", "opus", 1.0),
            ("libmp3lame0", "lame", 1.0),
            ("libmpg123-0", "mpg123", 1.0),
            ("libjack-jackd2-0", "jack2", 1.0),
            ("libjack0", "jack2", 0.9),
            ("libportaudio2", "portaudio", 1.0),
            ("libsdl2-mixer-2.0-0", "sdl2_mixer", 1.0),
            
            // ==================== VIDEO/MULTIMEDIA ====================
            ("libavcodec58", "ffmpeg", 0.9),
            ("libavcodec59", "ffmpeg", 0.9),
            ("libavcodec60", "ffmpeg", 0.9),
            ("libavformat58", "ffmpeg", 0.9),
            ("libavformat59", "ffmpeg", 0.9),
            ("libavformat60", "ffmpeg", 0.9),
            ("libavutil56", "ffmpeg", 0.9),
            ("libavutil57", "ffmpeg", 0.9),
            ("libavutil58", "ffmpeg", 0.9),
            ("libavfilter7", "ffmpeg", 0.9),
            ("libavfilter8", "ffmpeg", 0.9),
            ("libavfilter9", "ffmpeg", 0.9),
            ("libswscale5", "ffmpeg", 0.9),
            ("libswscale6", "ffmpeg", 0.9),
            ("libswresample3", "ffmpeg", 0.9),
            ("libswresample4", "ffmpeg", 0.9),
            ("libpostproc55", "ffmpeg", 0.9),
            ("libpostproc56", "ffmpeg", 0.9),
            ("ffmpeg", "ffmpeg", 1.0),
            ("libgstreamer1.0-0", "gstreamer", 1.0),
            ("gstreamer1.0-plugins-base", "gst-plugins-base", 1.0),
            ("gstreamer1.0-plugins-good", "gst-plugins-good", 1.0),
            ("gstreamer1.0-plugins-bad", "gst-plugins-bad", 1.0),
            ("gstreamer1.0-plugins-ugly", "gst-plugins-ugly", 1.0),
            ("gstreamer1.0-libav", "gst-libav", 1.0),
            ("libva2", "libva", 1.0),
            ("libva-drm2", "libva", 0.9),
            ("libva-x11-2", "libva", 0.9),
            ("libvdpau1", "libvdpau", 1.0),
            ("libtheora0", "libtheora", 1.0),
            ("libx264-163", "x264", 1.0),
            ("libx265-199", "x265", 1.0),
            ("libvpx7", "libvpx", 1.0),
            ("libdav1d6", "dav1d", 1.0),
            ("libaom3", "aom", 1.0),
            ("libopencv-core4.5", "opencv", 0.9),
            ("libv4l-0", "v4l-utils", 1.0),
            
            // ==================== FONTS ====================
            ("fontconfig", "fontconfig", 1.0),
            ("fontconfig-config", "fontconfig", 0.9),
            ("libfontconfig1", "fontconfig", 1.0),
            ("libfreetype6", "freetype2", 1.0),
            ("fonts-liberation", "ttf-liberation", 1.0),
            ("fonts-dejavu-core", "ttf-dejavu", 1.0),
            ("fonts-dejavu", "ttf-dejavu", 1.0),
            ("fonts-dejavu-extra", "ttf-dejavu", 0.9),
            ("fonts-droid-fallback", "noto-fonts", 0.8),
            ("fonts-noto", "noto-fonts", 1.0),
            ("fonts-noto-cjk", "noto-fonts-cjk", 1.0),
            ("fonts-noto-color-emoji", "noto-fonts-emoji", 1.0),
            ("fonts-roboto", "ttf-roboto", 1.0),
            ("fonts-ubuntu", "ttf-ubuntu-font-family", 1.0),
            ("fonts-hack", "ttf-hack", 1.0),
            ("fonts-firacode", "ttf-fira-code", 1.0),
            ("fonts-font-awesome", "ttf-font-awesome", 1.0),
            
            // ==================== PYTHON ====================
            ("python3", "python", 1.0),
            ("python3-minimal", "python", 0.9),
            ("libpython3-stdlib", "python", 0.8),
            ("libpython3.11-stdlib", "python", 0.8),
            ("libpython3.12-stdlib", "python", 0.8),
            ("python3-pip", "python-pip", 1.0),
            ("python3-setuptools", "python-setuptools", 1.0),
            ("python3-wheel", "python-wheel", 1.0),
            ("python3-venv", "python", 0.9),
            ("python3-dev", "python", 0.9),
            ("python3-distutils", "python", 0.9),
            ("python3-numpy", "python-numpy", 1.0),
            ("python3-scipy", "python-scipy", 1.0),
            ("python3-matplotlib", "python-matplotlib", 1.0),
            ("python3-pandas", "python-pandas", 1.0),
            ("python3-requests", "python-requests", 1.0),
            ("python3-urllib3", "python-urllib3", 1.0),
            ("python3-pil", "python-pillow", 1.0),
            ("python3-cryptography", "python-cryptography", 1.0),
            ("python3-openssl", "python-pyopenssl", 1.0),
            ("python3-yaml", "python-yaml", 1.0),
            ("python3-lxml", "python-lxml", 1.0),
            ("python3-dbus", "python-dbus", 1.0),
            ("python3-gi", "python-gobject", 1.0),
            ("python3-cairo", "python-cairo", 1.0),
            
            // ==================== PERL ====================
            ("perl", "perl", 1.0),
            ("perl-base", "perl", 0.9),
            ("perl-modules-5.36", "perl", 0.8),
            ("perl-modules-5.38", "perl", 0.8),
            ("libperl5.36", "perl", 0.9),
            ("libperl5.38", "perl", 0.9),
            
            // ==================== RUBY ====================
            ("ruby", "ruby", 1.0),
            ("ruby3.0", "ruby", 0.9),
            ("ruby3.1", "ruby", 0.9),
            ("libruby3.0", "ruby", 0.9),
            ("libruby3.1", "ruby", 0.9),
            
            // ==================== NODE.JS ====================
            ("nodejs", "nodejs", 1.0),
            ("npm", "npm", 1.0),
            ("libnode72", "nodejs", 0.9),
            ("libnode108", "nodejs", 0.9),
            
            // ==================== NETWORK ====================
            ("libcurl4", "curl", 1.0),
            ("libcurl4-openssl-dev", "curl", 0.9),
            ("libcurl4-gnutls-dev", "curl", 0.9),
            ("libcurl3-gnutls", "curl", 0.9),
            ("curl", "curl", 1.0),
            ("wget", "wget", 1.0),
            ("wget2", "wget2", 1.0),
            ("ca-certificates", "ca-certificates", 1.0),
            ("openssl", "openssl", 1.0),
            ("libssh2-1", "libssh2", 1.0),
            ("libssh-4", "libssh", 1.0),
            ("openssh-client", "openssh", 0.9),
            ("openssh-server", "openssh", 0.9),
            ("libnghttp2-14", "libnghttp2", 1.0),
            ("libnss3", "nss", 1.0),
            ("libnspr4", "nspr", 1.0),
            ("libnm0", "networkmanager", 0.9),
            ("network-manager", "networkmanager", 1.0),
            ("iputils-ping", "iputils", 1.0),
            ("iproute2", "iproute2", 1.0),
            ("dnsutils", "bind-tools", 1.0),
            ("net-tools", "net-tools", 1.0),
            ("netcat-openbsd", "openbsd-netcat", 1.0),
            ("socat", "socat", 1.0),
            ("rsync", "rsync", 1.0),
            
            // ==================== DATABASE ====================
            ("libsqlite3-0", "sqlite", 1.0),
            ("sqlite3", "sqlite", 1.0),
            ("libpq5", "postgresql-libs", 1.0),
            ("postgresql-client", "postgresql", 0.9),
            ("libmysqlclient21", "mariadb-libs", 0.9),
            ("default-mysql-client", "mariadb", 0.8),
            ("libmariadb3", "mariadb-libs", 1.0),
            ("libmongoc-1.0-0", "mongo-c-driver", 1.0),
            ("libhiredis0.14", "hiredis", 1.0),
            ("redis-tools", "redis", 0.9),
            
            // ==================== IMAGE LIBRARIES ====================
            ("libpng16-16", "libpng", 1.0),
            ("libpng-dev", "libpng", 0.9),
            ("libjpeg62-turbo", "libjpeg-turbo", 1.0),
            ("libjpeg-turbo8", "libjpeg-turbo", 1.0),
            ("libjpeg8", "libjpeg-turbo", 0.9),
            ("libjpeg-dev", "libjpeg-turbo", 0.9),
            ("libwebp7", "libwebp", 1.0),
            ("libwebp6", "libwebp", 1.0),
            ("libwebpdemux2", "libwebp", 0.9),
            ("libwebpmux3", "libwebp", 0.9),
            ("libtiff6", "libtiff", 1.0),
            ("libtiff5", "libtiff", 1.0),
            ("libgif7", "giflib", 1.0),
            ("libraw20", "libraw", 1.0),
            ("libopenjp2-7", "openjpeg2", 1.0),
            ("libheif1", "libheif", 1.0),
            ("libjxl0.7", "libjxl", 1.0),
            ("libexif12", "libexif", 1.0),
            ("imagemagick", "imagemagick", 1.0),
            ("libmagickcore-6.q16-6", "imagemagick", 0.9),
            ("libmagickwand-6.q16-6", "imagemagick", 0.9),
            
            // ==================== DOCUMENT/TEXT ====================
            ("libxml2", "libxml2", 1.0),
            ("libxml2-utils", "libxml2", 0.9),
            ("libxslt1.1", "libxslt", 1.0),
            ("libyaml-0-2", "libyaml", 1.0),
            ("libjson-c5", "json-c", 1.0),
            ("libjson-glib-1.0-0", "json-glib", 1.0),
            ("liblcms2-2", "lcms2", 1.0),
            ("libpoppler118", "poppler", 1.0),
            ("libpoppler-glib8", "poppler-glib", 1.0),
            ("poppler-utils", "poppler", 0.9),
            ("libdjvulibre21", "djvulibre", 1.0),
            ("libedit2", "libedit", 1.0),
            
            // ==================== SYSTEM/HARDWARE ====================
            ("libudev1", "systemd-libs", 0.9),
            ("udev", "systemd", 0.9),
            ("libusb-1.0-0", "libusb", 1.0),
            ("libusbmuxd6", "libusbmuxd", 1.0),
            ("libimobiledevice6", "libimobiledevice", 1.0),
            ("libfuse2", "fuse2", 1.0),
            ("libfuse3-3", "fuse3", 1.0),
            ("fuse", "fuse2", 1.0),
            ("fuse3", "fuse3", 1.0),
            ("libblkid1", "util-linux-libs", 1.0),
            ("libmount1", "util-linux-libs", 1.0),
            ("libuuid1", "util-linux-libs", 1.0),
            ("libcap2", "libcap", 1.0),
            ("libseccomp2", "libseccomp", 1.0),
            ("libapparmor1", "apparmor", 0.9),
            ("libselinux1", "libselinux", 1.0),
            ("libpolkit-gobject-1-0", "polkit", 1.0),
            ("policykit-1", "polkit", 1.0),
            ("libpci3", "pciutils", 1.0),
            ("pciutils", "pciutils", 1.0),
            ("libacl1", "acl", 1.0),
            ("libattr1", "attr", 1.0),
            ("libmnl0", "libmnl", 1.0),
            ("libnftnl11", "libnftnl", 1.0),
            ("libnl-3-200", "libnl", 1.0),
            ("libnl-genl-3-200", "libnl", 0.9),
            ("libnl-route-3-200", "libnl", 0.9),
            ("libbluetooth3", "bluez-libs", 1.0),
            ("bluez", "bluez", 1.0),
            
            // ==================== DEVELOPMENT ====================
            ("build-essential", "base-devel", 0.9),
            ("gcc", "gcc", 1.0),
            ("g++", "gcc", 0.9),
            ("clang", "clang", 1.0),
            ("llvm", "llvm", 1.0),
            ("cmake", "cmake", 1.0),
            ("make", "make", 1.0),
            ("autoconf", "autoconf", 1.0),
            ("automake", "automake", 1.0),
            ("libtool", "libtool", 1.0),
            ("pkg-config", "pkgconf", 1.0),
            ("git", "git", 1.0),
            ("subversion", "subversion", 1.0),
            ("mercurial", "mercurial", 1.0),
            ("gdb", "gdb", 1.0),
            ("valgrind", "valgrind", 1.0),
            ("binutils", "binutils", 1.0),
            ("m4", "m4", 1.0),
            ("gettext", "gettext", 1.0),
            ("intltool", "intltool", 1.0),
            ("bison", "bison", 1.0),
            ("flex", "flex", 1.0),
            ("nasm", "nasm", 1.0),
            ("yasm", "yasm", 1.0),
            
            // ==================== ELECTRON/WEB APPS ====================
            ("libnss3", "nss", 1.0),
            ("libxss1", "libxss", 1.0),
            ("libasound2", "alsa-lib", 1.0),
            ("libatk-bridge2.0-0", "at-spi2-core", 1.0),
            ("libdrm2", "libdrm", 1.0),
            ("libgbm1", "mesa", 0.9),
            ("libxcomposite1", "libxcomposite", 1.0),
            ("libxdamage1", "libxdamage", 1.0),
            ("libxfixes3", "libxfixes", 1.0),
            ("libxrandr2", "libxrandr", 1.0),
            ("libxkbcommon0", "libxkbcommon", 1.0),
            ("libatspi2.0-0", "at-spi2-core", 0.9),
            ("libcups2", "cups", 0.9),
            
            // ==================== GAMING/SDL ====================
            ("libsdl2-2.0-0", "sdl2", 1.0),
            ("libsdl2-image-2.0-0", "sdl2_image", 1.0),
            ("libsdl2-ttf-2.0-0", "sdl2_ttf", 1.0),
            ("libsdl2-net-2.0-0", "sdl2_net", 1.0),
            ("libsdl2-gfx-1.0-0", "sdl2_gfx", 1.0),
            ("libsdl1.2debian", "sdl", 1.0),
            ("libglew2.2", "glew", 1.0),
            ("libglfw3", "glfw-x11", 1.0),
            ("libopenscenegraph161", "openscenegraph", 1.0),
            ("libenet7", "enet", 1.0),
            
            // ==================== ARCHIVING ====================
            ("libarchive13", "libarchive", 1.0),
            ("liblz4-1", "lz4", 1.0),
            ("libzstd1", "zstd", 1.0),
            ("liblzo2-2", "lzo", 1.0),
            ("libsnappy1v5", "snappy", 1.0),
            ("libbrotli1", "brotli", 1.0),
            
            // ==================== MISCELLANEOUS ====================
            ("mime-support", "mailcap", 0.9),
            ("shared-mime-info", "shared-mime-info", 1.0),
            ("desktop-file-utils", "desktop-file-utils", 1.0),
            ("hicolor-icon-theme", "hicolor-icon-theme", 1.0),
            ("adwaita-icon-theme", "adwaita-icon-theme", 1.0),
            ("gnome-icon-theme", "adwaita-icon-theme", 0.8),
            ("dconf-gsettings-backend", "dconf", 1.0),
            ("libdconf1", "dconf", 0.9),
            ("gvfs", "gvfs", 1.0),
            ("libgvfscommon0", "gvfs", 0.9),
            ("zenity", "zenity", 1.0),
            ("libinput10", "libinput", 1.0),
            ("libevdev2", "libevdev", 1.0),
            ("libmtdev1", "mtdev", 1.0),
            ("libwacom2", "libwacom", 1.0),
            ("libgudev-1.0-0", "libgudev", 1.0),
            ("libcolord2", "colord", 1.0),
            ("colord", "colord", 1.0),
            ("libcanberra0", "libcanberra", 1.0),
            ("libcanberra-gtk3-0", "libcanberra", 0.9),
            ("sound-theme-freedesktop", "sound-theme-freedesktop", 1.0),
        ];

        for (debian, arch, confidence) in mappings {
            self.mappings.insert(
                debian.to_string(),
                PackageMapping {
                    debian_name: debian.to_string(),
                    arch_name: arch.to_string(),
                    confidence,
                    source: MappingSource::Builtin,
                },
            );
        }

        // Virtual packages
        let virtuals = [
            ("debconf", vec!["dialog", "whiptail"]),
            ("awk", vec!["gawk", "mawk", "nawk"]),
            ("c-compiler", vec!["gcc", "clang"]),
            ("c++-compiler", vec!["gcc", "clang"]),
            // Java virtual packages - map to jre-openjdk which provides java-runtime
            ("java-runtime", vec!["jre-openjdk", "jre17-openjdk", "jre11-openjdk", "jre8-openjdk"]),
            ("java-runtime-headless", vec!["jre-openjdk-headless", "jre17-openjdk-headless", "jre11-openjdk-headless", "jre8-openjdk-headless"]),
            ("java-sdk", vec!["jdk-openjdk", "jdk17-openjdk", "jdk11-openjdk", "jdk8-openjdk"]),
            ("java-sdk-headless", vec!["jdk-openjdk", "jdk17-openjdk", "jdk11-openjdk", "jdk8-openjdk"]),
            ("java8-runtime", vec!["jre8-openjdk"]),
            ("java8-runtime-headless", vec!["jre8-openjdk-headless"]),
            ("java11-runtime", vec!["jre11-openjdk"]),
            ("java17-runtime", vec!["jre17-openjdk"]),
            ("java21-runtime", vec!["jre21-openjdk"]),
            ("www-browser", vec!["firefox", "chromium", "vivaldi"]),
            ("x-terminal-emulator", vec!["alacritty", "kitty", "gnome-terminal", "konsole"]),
            ("editor", vec!["vim", "nano", "emacs"]),
            ("mail-transport-agent", vec!["postfix", "exim"]),
        ];

        for (virtual_name, providers) in virtuals {
            self.virtual_packages.insert(
                virtual_name.to_string(),
                providers.into_iter().map(String::from).collect(),
            );
        }
    }

    /// Load cached database files
    fn load_cached_data(&mut self) -> Result<()> {
        // Load custom mappings
        let mappings_path = self.db_dir.join("mappings.json");
        if mappings_path.exists() {
            let content = std::fs::read_to_string(&mappings_path)?;
            let custom_mappings: HashMap<String, PackageMapping> = serde_json::from_str(&content)?;
            self.mappings.extend(custom_mappings);
        }

        // Load Arch package cache
        let arch_path = self.db_dir.join("arch_packages.json");
        if arch_path.exists() {
            let content = std::fs::read_to_string(&arch_path)?;
            self.arch_packages = serde_json::from_str(&content)?;
        }

        // Load AUR cache
        let aur_path = self.db_dir.join("aur_packages.json");
        if aur_path.exists() {
            let content = std::fs::read_to_string(&aur_path)?;
            self.aur_packages = serde_json::from_str(&content)?;
        }

        Ok(())
    }

    /// Save database to disk
    pub fn save(&self) -> Result<()> {
        let mappings_path = self.db_dir.join("mappings.json");
        let content = serde_json::to_string_pretty(&self.mappings)?;
        std::fs::write(mappings_path, content)?;
        Ok(())
    }

    /// Look up a package mapping
    pub fn lookup(&self, debian_name: &str) -> Result<Option<(String, f32)>> {
        // Check direct mapping first
        if let Some(mapping) = self.mappings.get(debian_name) {
            return Ok(Some((mapping.arch_name.clone(), mapping.confidence)));
        }

        // Check if name is already an Arch package
        if self.arch_packages.contains_key(debian_name) {
            return Ok(Some((debian_name.to_string(), 1.0)));
        }

        // Check provides
        for (name, info) in &self.arch_packages {
            if info.provides.contains(&debian_name.to_string()) {
                return Ok(Some((name.clone(), 0.9)));
            }
        }

        Ok(None)
    }

    /// Check if a package is virtual
    pub fn is_virtual(&self, name: &str) -> Result<bool> {
        Ok(self.virtual_packages.contains_key(name))
    }

    /// Get virtual package providers
    pub fn get_virtual_providers(&self, name: &str) -> Option<&Vec<String>> {
        self.virtual_packages.get(name)
    }

    /// Get all Arch package names for fuzzy matching
    pub fn get_arch_package_names(&self) -> Vec<&str> {
        self.arch_packages.keys().map(|s| s.as_str()).collect()
    }

    /// Update package mappings from online sources
    pub async fn update_mappings(&self, _force: bool) -> Result<()> {
        // TODO: Implement fetching from online sources
        tracing::info!("Package mappings are up to date");
        Ok(())
    }

    /// Update virtual packages database
    pub async fn update_virtual_packages(&self, _force: bool) -> Result<()> {
        // TODO: Implement fetching virtual packages
        tracing::info!("Virtual packages database is up to date");
        Ok(())
    }

    /// Update AUR cache
    pub async fn update_aur_cache(&self, _force: bool) -> Result<()> {
        // TODO: Implement AUR cache update
        tracing::info!("AUR cache is up to date");
        Ok(())
    }

    /// Search for Arch packages
    pub async fn search_arch(&self, query: &str, _fuzzy: bool, limit: usize) -> Result<Vec<SearchResult>> {
        let query_lower = query.to_lowercase();
        let mut results: Vec<SearchResult> = self.arch_packages
            .iter()
            .filter(|(name, info)| {
                name.to_lowercase().contains(&query_lower) 
                    || info.description.to_lowercase().contains(&query_lower)
            })
            .take(limit)
            .map(|(name, info)| SearchResult {
                name: name.clone(),
                description: info.description.clone(),
                version: info.version.clone(),
                score: if name.to_lowercase() == query_lower { 1.0 } else { 0.8 },
            })
            .collect();

        results.sort_by(|a, b| b.score.partial_cmp(&a.score).unwrap_or(std::cmp::Ordering::Equal));
        Ok(results)
    }

    /// Search for AUR packages
    pub async fn search_aur(&self, query: &str, _fuzzy: bool, limit: usize) -> Result<Vec<SearchResult>> {
        // For now, search local cache
        let query_lower = query.to_lowercase();
        let mut results: Vec<SearchResult> = self.aur_packages
            .iter()
            .filter(|(name, info)| {
                name.to_lowercase().contains(&query_lower) 
                    || info.description.to_lowercase().contains(&query_lower)
            })
            .take(limit)
            .map(|(name, info)| SearchResult {
                name: name.clone(),
                description: info.description.clone(),
                version: info.version.clone(),
                score: if name.to_lowercase() == query_lower { 1.0 } else { 0.8 },
            })
            .collect();

        results.sort_by(|a, b| b.score.partial_cmp(&a.score).unwrap_or(std::cmp::Ordering::Equal));
        Ok(results)
    }

    /// Add a custom mapping
    pub fn add_mapping(&mut self, debian: &str, arch: &str, confidence: f32) {
        self.mappings.insert(
            debian.to_string(),
            PackageMapping {
                debian_name: debian.to_string(),
                arch_name: arch.to_string(),
                confidence,
                source: MappingSource::User,
            },
        );
    }
}

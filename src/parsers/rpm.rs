//! RPM package (.rpm) parser
//!
//! RPM files consist of:
//! - lead: 96 bytes, magic `ed ab ee db`
//! - signature header: tags + data store (not verified — rexeb has no keyring)
//! - main header: tags + data store (name, version, deps, scripts, ...)
//! - payload: usually cpio, compressed with gzip/xz/zstd/bzip2
//!
//! Only binary RPMs are supported; source RPMs (`.src.rpm`) are rejected
//! with a clear error. Dependencies use RPM semantics: soname requires
//! (`libc.so.6`), file requires (`/usr/bin/sh`), and plain package names.
//! Sonames and paths are resolved against the local system (`ldconfig` +
//! `pacman -Qo`/`pacman -F`); plain names flow through the standard
//! resolver pipeline (exact repo match first, then DB/fuzzy/AUR).

use std::collections::HashMap;
use std::fs::File;
use std::io::{Read, Seek, SeekFrom};
use std::path::{Component, Path, PathBuf};
use std::sync::OnceLock;

use tempfile::TempDir;

use crate::error::{RexebError, Result};
use crate::models::{
    Architecture, Dependency, DependencyType, License, MaintainerScript, PackageFormat,
    PackageMetadata, VersionOp,
};
use crate::parsers::Parser;

// RPM header tag numbers (stable ABI)
const TAG_NAME: u32 = 1000;
const TAG_VERSION: u32 = 1001;
const TAG_RELEASE: u32 = 1002;
const TAG_EPOCH: u32 = 1003;
const TAG_SUMMARY: u32 = 1004;
const TAG_DESCRIPTION: u32 = 1005;
const TAG_LICENSE: u32 = 1014;
const TAG_PACKAGER: u32 = 1015;
const TAG_URL: u32 = 1020;
const TAG_ARCH: u32 = 1022;
const TAG_PREIN: u32 = 1025;
const TAG_PREINPROG: u32 = 1026;
const TAG_POSTIN: u32 = 1027;
const TAG_POSTINPROG: u32 = 1028;
const TAG_PREUN: u32 = 1029;
const TAG_PREUNPROG: u32 = 1030;
const TAG_POSTUN: u32 = 1031;
const TAG_POSTUNPROG: u32 = 1032;
const TAG_PROVIDENAME: u32 = 1047;
const TAG_REQUIREFLAGS: u32 = 1048;
const TAG_REQUIRENAME: u32 = 1049;
const TAG_REQUIREVERSION: u32 = 1050;
const TAG_CONFLICTFLAGS: u32 = 1053;
const TAG_CONFLICTNAME: u32 = 1054;
const TAG_CONFLICTVERSION: u32 = 1055;
const TAG_OBSOLETENAME: u32 = 1090;
const TAG_OBSOLETEFLAGS: u32 = 1114;
const TAG_OBSOLETEVERSION: u32 = 1115;
const TAG_PAYLOADFORMAT: u32 = 1124;
const TAG_PAYLOADCOMPRESSOR: u32 = 1125;

// RPM value types
const TYPE_INT8: u32 = 2;
const TYPE_INT16: u32 = 3;
const TYPE_INT32: u32 = 4;
const TYPE_INT64: u32 = 5;
const TYPE_STRING: u32 = 6;
const TYPE_BIN: u32 = 7;
const TYPE_STRING_ARRAY: u32 = 8;
const TYPE_I18NSTRING: u32 = 9;

/// Quick magic-byte check for RPM files
pub fn is_rpm(path: &Path) -> bool {
    let mut file = match File::open(path) {
        Ok(f) => f,
        Err(_) => return false,
    };
    let mut magic = [0u8; 4];
    match file.read_exact(&mut magic) {
        Ok(()) => magic == [0xed, 0xab, 0xee, 0xdb],
        Err(_) => false,
    }
}

/// A parsed RPM header: tag number -> value
#[derive(Debug, Default)]
struct RpmHeader {
    values: HashMap<u32, RpmValue>,
}

/// A single RPM header value
#[derive(Debug, Clone)]
enum RpmValue {
    String(String),
    StringArray(Vec<String>),
    Int32(Vec<u32>),
    /// Raw binary payload (signatures, digests); retained for future verification.
    #[allow(dead_code)]
    Bytes(Vec<u8>),
}

impl RpmHeader {
    fn string(&self, tag: u32) -> Option<String> {
        match self.values.get(&tag)? {
            RpmValue::String(s) => Some(s.clone()),
            RpmValue::StringArray(a) => a.first().cloned(),
            RpmValue::Int32(v) => v.first().map(|n| n.to_string()),
            RpmValue::Bytes(_) => None,
        }
    }

    fn string_array(&self, tag: u32) -> Vec<String> {
        match self.values.get(&tag) {
            Some(RpmValue::StringArray(a)) => a.clone(),
            Some(RpmValue::String(s)) => vec![s.clone()],
            _ => Vec::new(),
        }
    }

    fn int32_array(&self, tag: u32) -> Vec<u32> {
        match self.values.get(&tag) {
            Some(RpmValue::Int32(v)) => v.clone(),
            _ => Vec::new(),
        }
    }

    fn uint32(&self, tag: u32) -> Option<u32> {
        self.int32_array(tag).into_iter().next()
    }
}

/// Read one RPM header (magic + index + store) from the current position
fn read_header(file: &mut File) -> Result<RpmHeader> {
    let mut magic = [0u8; 16];
    file.read_exact(&mut magic).map_err(|e| {
        RexebError::RpmParsing(format!("Truncated RPM header: {}", e))
    })?;
    if magic[0..4] != [0x8e, 0xad, 0xe8, 0x01] {
        return Err(RexebError::RpmParsing(
            "Bad RPM header magic (not an RPM file?)".into(),
        ));
    }
    let index_count = u32::from_be_bytes([magic[8], magic[9], magic[10], magic[11]]) as usize;
    let data_len = u32::from_be_bytes([magic[12], magic[13], magic[14], magic[15]]) as usize;
    if index_count > 10_000 || data_len > 256 * 1024 * 1024 {
        return Err(RexebError::RpmParsing(
            "Implausible RPM header size (corrupt file?)".into(),
        ));
    }

    let mut index = vec![0u8; index_count * 16];
    file.read_exact(&mut index)
        .map_err(|e| RexebError::RpmParsing(format!("Truncated RPM index: {}", e)))?;
    let mut store = vec![0u8; data_len];
    file.read_exact(&mut store)
        .map_err(|e| RexebError::RpmParsing(format!("Truncated RPM store: {}", e)))?;

    let mut header = RpmHeader::default();
    for entry in index.chunks_exact(16) {
        let tag = u32::from_be_bytes([entry[0], entry[1], entry[2], entry[3]]);
        let typ = u32::from_be_bytes([entry[4], entry[5], entry[6], entry[7]]);
        let offset = u32::from_be_bytes([entry[8], entry[9], entry[10], entry[11]]) as usize;
        let count = u32::from_be_bytes([entry[12], entry[13], entry[14], entry[15]]) as usize;
        if offset >= store.len() {
            continue;
        }
        let value = parse_value(typ, count, &store[offset..]);
        if let Some(v) = value {
            header.values.insert(tag, v);
        }
    }
    Ok(header)
}

/// Parse one index entry from the store slice at its offset
fn parse_value(typ: u32, count: usize, data: &[u8]) -> Option<RpmValue> {
    match typ {
        TYPE_STRING => Some(RpmValue::String(read_c_string(data)?)),
        TYPE_STRING_ARRAY | TYPE_I18NSTRING => {
            let mut out = Vec::new();
            let mut rest = data;
            for _ in 0..count {
                let s = read_c_string(rest)?;
                rest = &rest[s.len() + 1..];
                out.push(s);
            }
            Some(RpmValue::StringArray(out))
        }
        TYPE_INT32 => {
            if data.len() < count * 4 {
                return None;
            }
            let mut out = Vec::with_capacity(count);
            for chunk in data[..count * 4].chunks_exact(4) {
                out.push(u32::from_be_bytes([chunk[0], chunk[1], chunk[2], chunk[3]]));
            }
            Some(RpmValue::Int32(out))
        }
        TYPE_INT16 => {
            if data.len() < count * 2 {
                return None;
            }
            let mut out = Vec::with_capacity(count);
            for chunk in data[..count * 2].chunks_exact(2) {
                out.push(u32::from(u16::from_be_bytes([chunk[0], chunk[1]])));
            }
            Some(RpmValue::Int32(out))
        }
        TYPE_INT64 => {
            if data.len() < count * 8 {
                return None;
            }
            // Only the low 32 bits are ever needed (sizes); keep it simple
            let mut out = Vec::with_capacity(count);
            for chunk in data[..count * 8].chunks_exact(8) {
                out.push(u32::from_be_bytes([chunk[4], chunk[5], chunk[6], chunk[7]]));
            }
            Some(RpmValue::Int32(out))
        }
        TYPE_INT8 | TYPE_BIN => Some(RpmValue::Bytes(data[..data.len().min(count)].to_vec())),
        _ => None,
    }
}

/// Read a NUL-terminated string (lossy)
fn read_c_string(data: &[u8]) -> Option<String> {
    let end = data.iter().position(|&b| b == 0)?;
    Some(String::from_utf8_lossy(&data[..end]).into_owned())
}

/// Parser for RPM packages
pub struct RpmParser {
    /// Path to the .rpm file
    path: PathBuf,
    /// Temporary directory for extraction
    temp_dir: TempDir,
    /// Path to extracted data directory
    data_dir: PathBuf,
    /// Parsed main header
    header: RpmHeader,
    /// Installed file list (absolute paths, for analysis/conflicts)
    files: Vec<PathBuf>,
    /// Accumulated installed size in bytes
    installed_size: u64,
    /// Cached `ldconfig -p` soname -> path map
    ldconfig_cache: OnceLock<HashMap<String, String>>,
}

impl RpmParser {
    /// Create a new parser for the given .rpm file
    pub fn new(path: impl AsRef<Path>) -> Result<Self> {
        let path = path.as_ref().to_path_buf();

        if !path.exists() {
            return Err(RexebError::file_not_found(&path));
        }
        if !is_rpm(&path) {
            return Err(RexebError::RpmParsing(format!(
                "Not an RPM file: {}",
                path.display()
            )));
        }

        // Prefixed so `rexeb clean --temp` can find leftovers (e.g. from --keep-temp)
        let temp_dir = tempfile::Builder::new().prefix("rexeb-").tempdir()?;
        let data_dir = temp_dir.path().join("data");
        std::fs::create_dir_all(&data_dir)?;

        let mut parser = Self {
            path,
            temp_dir,
            data_dir,
            header: RpmHeader::default(),
            files: Vec::new(),
            installed_size: 0,
            ldconfig_cache: OnceLock::new(),
        };

        parser.extract_payload()?;

        Ok(parser)
    }

    /// Path to the source .rpm file
    pub fn path(&self) -> &Path {
        self.path.as_path()
    }

    /// Get the extraction directory path
    pub fn extract_dir(&self) -> &Path {
        self.data_dir.as_path()
    }

    /// Read headers, then stream-decompress and extract the cpio payload
    fn extract_payload(&mut self) -> Result<()> {
        let mut file = File::open(&self.path)?;

        // Lead (96 bytes, magic already verified)
        file.seek(SeekFrom::Start(96))?;

        // Signature header: read index, skip the store (signatures are not
        // verified — rexeb has no keyring)
        let mut sig_head = [0u8; 16];
        file.read_exact(&mut sig_head).map_err(|e| {
            RexebError::RpmParsing(format!("Truncated signature header: {}", e))
        })?;
        if sig_head[0..4] != [0x8e, 0xad, 0xe8, 0x01] {
            return Err(RexebError::RpmParsing("Bad signature header magic".into()));
        }
        let sig_il = u32::from_be_bytes([sig_head[8], sig_head[9], sig_head[10], sig_head[11]]) as i64;
        let sig_dl = u32::from_be_bytes([sig_head[12], sig_head[13], sig_head[14], sig_head[15]]) as i64;
        if sig_il > 10_000 || sig_dl > 256 * 1024 * 1024 {
            return Err(RexebError::RpmParsing(
                "Implausible signature header size (corrupt file?)".into(),
            ));
        }
        // The header record is padded to 8-byte alignment
        let skip = sig_il * 16 + sig_dl;
        file.seek(SeekFrom::Current((skip + 7) & !7))?;

        // Main header
        self.header = read_header(&mut file)?;

        let compressor = self
            .header
            .string(TAG_PAYLOADCOMPRESSOR)
            .unwrap_or_else(|| "gzip".to_string());
        let format = self.header.string(TAG_PAYLOADFORMAT);
        if let Some(f) = format {
            if f != "cpio" {
                return Err(RexebError::RpmParsing(format!(
                    "Unsupported RPM payload format: {}",
                    f
                )));
            }
        }

        // Payload follows the header, possibly with alignment padding: scan
        // a few bytes for the compressor magic instead of assuming offset 0.
        let magic = compressor_magic(&compressor).ok_or_else(|| {
            RexebError::RpmParsing(format!("Unsupported compressor: {}", compressor))
        })?;
        let mut lookahead = [0u8; 16];
        file.read_exact(&mut lookahead).map_err(|e| {
            RexebError::RpmParsing(format!("Truncated payload: {}", e))
        })?;
        let start = (0..8)
            .find(|&i| lookahead[i..].starts_with(magic))
            .ok_or_else(|| RexebError::RpmParsing("Payload start not found".into()))?;
        let chained = std::io::Cursor::new(lookahead[start..].to_vec()).chain(file);

        let mut decoder: Box<dyn Read> = match compressor.as_str() {
            "gzip" => Box::new(flate2::read::GzDecoder::new(chained)),
            "xz" | "lzma" => Box::new(xz2::read::XzDecoder::new(chained)),
            "zstd" => Box::new(zstd::stream::read::Decoder::new(chained)?),
            "bzip2" | "bz2" | "bzip" => Box::new(bzip2::read::BzDecoder::new(chained)),
            other => {
                return Err(RexebError::RpmParsing(format!(
                    "Unsupported compressor: {}",
                    other
                )))
            }
        };

        let data_dir = self.data_dir.clone();
        let (files, size) = extract_cpio(&mut decoder, &data_dir)?;
        self.files = files;
        self.installed_size = size;
        Ok(())
    }

    /// Parse metadata from the RPM header
    pub fn parse(&self) -> Result<PackageMetadata> {
        let name = self.header.string(TAG_NAME).ok_or_else(|| {
            RexebError::RpmParsing("RPM has no name tag".into())
        })?;
        let version = self.header.string(TAG_VERSION).ok_or_else(|| {
            RexebError::RpmParsing("RPM has no version tag".into())
        })?;
        let release = self
            .header
            .string(TAG_RELEASE)
            .unwrap_or_else(|| "1".to_string());
        let arch = self.header.string(TAG_ARCH).unwrap_or_default();
        let arch = Architecture::from_rpm(&arch)?;

        let mut meta = PackageMetadata::new(&name, &version);
        meta.release = sanitize_release(&release);
        if let Some(epoch) = self.header.uint32(TAG_EPOCH) {
            if epoch > 0 {
                meta.epoch = Some(epoch);
            }
        }
        meta.arch = arch;
        meta.description = self
            .header
            .string(TAG_DESCRIPTION)
            .or_else(|| self.header.string(TAG_SUMMARY))
            .map(|d| first_line(&d))
            .unwrap_or_default();
        if meta.description.is_empty() {
            meta.description = format!("Converted from {}.rpm", name);
        }
        meta.url = self.header.string(TAG_URL);
        meta.maintainer = self.header.string(TAG_PACKAGER);
        meta.license = License::from_str(
            &self
                .header
                .string(TAG_LICENSE)
                .unwrap_or_default(),
        );
        meta.installed_size = self.installed_size;
        meta.files = self.files.clone();

        self.parse_dependencies(&mut meta);
        self.parse_scripts(&mut meta);

        Ok(meta)
    }

    /// Translate RPM dependency tags into Arch-bound dependencies
    fn parse_dependencies(&self, meta: &mut PackageMetadata) {
        let mut mapped = 0usize;
        let mut skipped = 0usize;

        let push = |deps: &mut Vec<Dependency>,
                    names: Vec<String>,
                    flags: Vec<u32>,
                    versions: Vec<String>,
                    parser: &RpmParser,
                    mapped: &mut usize,
                    skipped: &mut usize| {
            for (i, raw_name) in names.iter().enumerate() {
                let flag = flags.get(i).copied().unwrap_or(0);
                let evr = versions.get(i).map(|s| s.as_str()).unwrap_or("");
                // Skip checks run on the raw name: stripping would turn
                // `rpmlib(...)` into `rpmlib` and defeat the match.
                if is_skipped_require(raw_name) {
                    *skipped += 1;
                    continue;
                }
                let name = strip_qualifiers(raw_name);
                if name.is_empty() {
                    *skipped += 1;
                    continue;
                }
                let mut dep = Dependency::new(name);
                if let Some(op) = VersionOp::from_rpm_flags(flag) {
                    if !evr.trim().is_empty() {
                        dep.version_op = Some(op);
                        dep.version = Some(evr.trim().to_string());
                    }
                }
                // Eager system mapping for sonames/paths/exact names;
                // anything left unmapped flows through the normal pipeline.
                if name.starts_with('/') || is_soname(name) {
                    if let Some((arch, confidence)) = parser.resolve_system_dep(name) {
                        dep.set_arch_name(arch, confidence);
                        *mapped += 1;
                    }
                } else if Self::exact_arch_match(name) {
                    dep.set_arch_name(name.to_string(), 1.0);
                    *mapped += 1;
                }
                deps.push(dep);
            }
        };

        push(
            meta.dependencies
                .entry(DependencyType::Depends)
                .or_default(),
            self.header.string_array(TAG_REQUIRENAME),
            self.header.int32_array(TAG_REQUIREFLAGS),
            self.header.string_array(TAG_REQUIREVERSION),
            self,
            &mut mapped,
            &mut skipped,
        );
        push(
            meta.dependencies
                .entry(DependencyType::Conflicts)
                .or_default(),
            self.header.string_array(TAG_CONFLICTNAME),
            self.header.int32_array(TAG_CONFLICTFLAGS),
            self.header.string_array(TAG_CONFLICTVERSION),
            self,
            &mut mapped,
            &mut skipped,
        );
        // RPM Obsoletes ~= Arch Replaces (take over another package's files)
        push(
            meta.dependencies
                .entry(DependencyType::Replaces)
                .or_default(),
            self.header.string_array(TAG_OBSOLETENAME),
            self.header.int32_array(TAG_OBSOLETEFLAGS),
            self.header.string_array(TAG_OBSOLETEVERSION),
            self,
            &mut mapped,
            &mut skipped,
        );

        for raw_name in self.header.string_array(TAG_PROVIDENAME) {
            if raw_name.starts_with("config(") {
                skipped += 1;
                continue;
            }
            let name = strip_qualifiers(&raw_name);
            if name.is_empty() {
                skipped += 1;
                continue;
            }
            meta.dependencies
                .entry(DependencyType::Provides)
                .or_default()
                .push(Dependency::new(name));
        }

        tracing::debug!(
            "RPM deps: {} eagerly mapped, {} skipped (rpmlib/config/rtld)",
            mapped,
            skipped
        );
    }

    /// Translate RPM scriptlets into maintainer scripts
    ///
    /// `<lua>` scriptlets cannot run on Arch and are skipped with a warning.
    fn parse_scripts(&self, meta: &mut PackageMetadata) {
        for (body_tag, prog_tag, script) in [
            (TAG_PREIN, TAG_PREINPROG, MaintainerScript::PreInst),
            (TAG_POSTIN, TAG_POSTINPROG, MaintainerScript::PostInst),
            (TAG_PREUN, TAG_PREUNPROG, MaintainerScript::PreRm),
            (TAG_POSTUN, TAG_POSTUNPROG, MaintainerScript::PostRm),
        ] {
            let body = self.header.string(body_tag).unwrap_or_default();
            if body.trim().is_empty() {
                continue;
            }
            let prog = self.header.string(prog_tag).unwrap_or_default();
            if prog.contains("lua") {
                tracing::warn!(
                    "Skipping {:?} scriptlet: <lua> scriptlets cannot run on Arch",
                    script
                );
                continue;
            }
            meta.scripts.insert(script, body);
        }
    }

    /// Resolve a soname or absolute-path require against the local system
    ///
    /// `ldconfig` + `pacman -Qo` first (installed packages), then the
    /// `pacman -F` file database (also covers not-installed packages).
    /// Returns `None` when nothing matches — the require then flows through
    /// the normal resolver pipeline.
    fn resolve_system_dep(&self, name: &str) -> Option<(String, f32)> {
        if is_soname(name) {
            if let Some(path) = self.ldconfig_lookup(name) {
                if let Some(pkg) = pacman_owner(&path) {
                    return Some((pkg, 0.95));
                }
            }
        } else if name.starts_with('/') && Path::new(name).exists() {
            if let Some(pkg) = pacman_owner(name) {
                return Some((pkg, 0.95));
            }
        }
        pacman_files_db(name).map(|pkg| (pkg, 0.9))
    }

    /// Whether an Arch package of exactly this name exists (sync DBs or installed)
    fn exact_arch_match(name: &str) -> bool {
        if name.is_empty() || name.contains(|c: char| !(c.is_ascii_alphanumeric() || "@._+-".contains(c))) {
            return false;
        }
        for args in [&["-Si", name][..], &["-Qi", name][..]] {
            if let Ok(output) = std::process::Command::new("pacman")
                .args(args)
                .output()
            {
                if output.status.success() {
                    return true;
                }
            }
        }
        false
    }

    /// Look up a soname in the `ldconfig -p` cache (parsed once per parser)
    fn ldconfig_lookup(&self, soname: &str) -> Option<String> {
        let cache = self.ldconfig_cache.get_or_init(|| {
            let mut map = HashMap::new();
            if let Ok(output) = std::process::Command::new("ldconfig").arg("-p").output() {
                if output.status.success() {
                    for line in String::from_utf8_lossy(&output.stdout).lines() {
                        // "\tlibc.so.6 (libc6,x86-64) => /usr/lib/libc.so.6"
                        let line = line.trim();
                        if let Some((lib, path)) = line.split_once("=>") {
                            if let Some(lib_name) = lib.split_whitespace().next() {
                                map.insert(
                                    lib_name.to_string(),
                                    path.trim().to_string(),
                                );
                            }
                        }
                    }
                }
            }
            map
        });
        cache.get(soname).cloned()
    }
}

impl Parser for RpmParser {
    fn new(path: &Path) -> Result<Self> {
        RpmParser::new(path)
    }

    fn parse(&self) -> Result<PackageMetadata> {
        self.parse()
    }

    fn extract_dir(&self) -> &Path {
        self.extract_dir()
    }

    fn format(&self) -> PackageFormat {
        PackageFormat::Rpm
    }

    fn persist(self: Box<Self>) -> PathBuf {
        let path = self.temp_dir.path().to_path_buf();
        std::mem::forget(self.temp_dir);
        path
    }
}

/// Magic bytes identifying each supported payload compressor
fn compressor_magic(compressor: &str) -> Option<&'static [u8]> {
    match compressor {
        "gzip" => Some(&[0x1f, 0x8b]),
        "xz" | "lzma" => Some(&[0xfd, 0x37, 0x7a, 0x58, 0x5a, 0x00]),
        "zstd" => Some(&[0x28, 0xb5, 0x2f, 0xfd]),
        "bzip2" | "bz2" | "bzip" => Some(&[0x42, 0x5a, 0x68]),
        _ => None,
    }
}

/// Strip RPM qualifiers: `libfoo.so.1()(64bit)` -> `libfoo.so.1`
fn strip_qualifiers(name: &str) -> &str {
    name.split('(').next().unwrap_or(name).trim()
}

/// Whether a require is environment noise that is always satisfied
///
/// - `rpmlib(...)`: rpm feature markers (payload compression etc.)
/// - `config(...)`: config-file markers, satisfied by the owning package
/// - `rtld(GNU_HASH)`: the dynamic loader (glibc is always installed)
/// - `group(...)`/`user(...)`: sysusers markers (no Arch equivalent needed)
fn is_skipped_require(name: &str) -> bool {
    name.starts_with("rpmlib(")
        || name.starts_with("config(")
        || name == "rtld(GNU_HASH)"
        || name.starts_with("group(")
        || name.starts_with("user(")
}

/// Whether a require name is a shared-library soname
fn is_soname(name: &str) -> bool {
    !name.starts_with('/') && name.contains(".so")
}

/// Sanitize an RPM release (`1.fc39`, `3.el9_0`) for Arch's pkgrel
fn sanitize_release(release: &str) -> String {
    let cleaned: String = release
        .chars()
        .map(|c| {
            if c.is_ascii_alphanumeric() || c == '.' || c == '_' || c == '+' {
                c
            } else {
                '.'
            }
        })
        .collect();
    let cleaned = cleaned.trim_matches('.').to_string();
    if cleaned.is_empty() {
        "1".to_string()
    } else {
        cleaned
    }
}

/// First line of a (possibly multi-line) description, trimmed
fn first_line(text: &str) -> String {
    text.lines().next().unwrap_or("").trim().to_string()
}

/// Find the owning Arch package of a local path via `pacman -Qo`
fn pacman_owner(path: &str) -> Option<String> {
    let output = std::process::Command::new("pacman")
        .arg("-Qo")
        .arg(path)
        .output()
        .ok()?;
    if !output.status.success() {
        return None;
    }
    // "<path> is owned by <pkg> <version>"
    let line = String::from_utf8_lossy(&output.stdout);
    let line = line.lines().next()?.trim();
    if !line.contains(" is owned by ") {
        return None;
    }
    let mut parts = line.split_whitespace();
    let _version = parts.next_back()?;
    parts.next_back().map(|s| s.to_string())
}

/// Find the Arch package shipping a file via the `pacman -F` database
///
/// Pure parsing lives in [`parse_pacman_files_db`] so it stays unit-testable.
fn pacman_files_db(query: &str) -> Option<String> {
    let output = std::process::Command::new("pacman")
        .arg("-F")
        .arg("--machinereadable")
        .arg(query)
        .output()
        .ok()?;
    if !output.status.success() {
        return None;
    }
    parse_pacman_files_db(&String::from_utf8_lossy(&output.stdout))
}

/// Parse `pacman -F --machinereadable` output (`repo\tpkg\tver\tpath`)
fn parse_pacman_files_db(output: &str) -> Option<String> {
    for line in output.lines() {
        let line = line.trim();
        if line.is_empty() {
            continue;
        }
        let mut fields = line.split('\t');
        let (Some(_repo), Some(pkg)) = (fields.next(), fields.next()) else {
            continue;
        };
        if !pkg.trim().is_empty() && !pkg.contains('/') {
            return Some(pkg.trim().to_string());
        }
    }
    None
}

/// Extract a cpio (newc) archive, returning installed files + total size
///
/// Only regular files, directories, and symlinks are restored; device
/// nodes and other special files are skipped. Paths escaping the output
/// directory (`..`, absolute paths) are contained, never followed.
fn extract_cpio(reader: &mut dyn Read, out_dir: &Path) -> Result<(Vec<PathBuf>, u64)> {
    let mut files = Vec::new();
    let mut total_size = 0u64;

    loop {
        let mut header = [0u8; 110];
        match reader.read_exact(&mut header) {
            Ok(()) => {}
            Err(e) if e.kind() == std::io::ErrorKind::UnexpectedEof => break,
            Err(e) => return Err(RexebError::Extraction(e.to_string())),
        }
        if &header[0..6] != b"070701" {
            return Err(RexebError::Extraction(
                "Bad cpio magic (unsupported payload format?)".into(),
            ));
        }
        let field = |offset: usize| -> Result<u32> {
            let text = std::str::from_utf8(&header[offset..offset + 8])
                .map_err(|_| RexebError::Extraction("Bad cpio header".into()))?;
            u32::from_str_radix(text, 16)
                .map_err(|_| RexebError::Extraction("Bad cpio header".into()))
        };
        let mode = field(14)?;
        let filesize = field(54)? as u64;
        let namesize = field(94)? as usize;
        if namesize == 0 || namesize > 4096 {
            return Err(RexebError::Extraction(
                "Bad cpio namesize (corrupt payload?)".into(),
            ));
        }

        let mut name_buf = vec![0u8; namesize];
        reader
            .read_exact(&mut name_buf)
            .map_err(|e| RexebError::Extraction(e.to_string()))?;
        skip_pad(&mut *reader, 110 + namesize)?;

        let name = String::from_utf8_lossy(&name_buf);
        let name = name.trim_end_matches('\0');
        if name == "TRAILER!!!" {
            break;
        }

        // Contain the path: strip leading slashes/dots, reject `..`
        let rel = name.trim_start_matches('/').trim_start_matches("./");
        let rel_path = Path::new(rel);
        if rel.is_empty()
            || rel_path
                .components()
                .any(|c| matches!(c, Component::ParentDir | Component::Prefix(_) | Component::RootDir))
        {
            tracing::warn!("Skipping suspicious cpio path: {}", name);
            skip_bytes(&mut *reader, filesize)?;
            skip_pad(&mut *reader, filesize as usize)?;
            continue;
        }
        let dest = out_dir.join(rel_path);

        let file_type = mode & 0o170000;
        if file_type == 0o040000 {
            std::fs::create_dir_all(&dest)?;
            #[cfg(unix)]
            {
                use std::os::unix::fs::PermissionsExt;
                let _ = std::fs::set_permissions(&dest, std::fs::Permissions::from_mode(mode & 0o7777));
            }
        } else if file_type == 0o120000 {
            // Symlink: file data is the target
            if filesize > 4096 {
                return Err(RexebError::Extraction(
                    "Bad cpio symlink size (corrupt payload?)".into(),
                ));
            }
            let mut target_buf = vec![0u8; filesize as usize];
            reader
                .read_exact(&mut target_buf)
                .map_err(|e| RexebError::Extraction(e.to_string()))?;
            skip_pad(&mut *reader, filesize as usize)?;
            let target = String::from_utf8_lossy(&target_buf).into_owned();
            if let Some(parent) = dest.parent() {
                std::fs::create_dir_all(parent)?;
            }
            if dest.exists() || dest.symlink_metadata().is_ok() {
                let _ = std::fs::remove_file(&dest);
            }
            #[cfg(unix)]
            std::os::unix::fs::symlink(&target, &dest)?;
            #[cfg(not(unix))]
            std::fs::write(&dest, target.as_bytes())?;
            files.push(PathBuf::from("/").join(rel_path));
        } else if file_type == 0o100000 {
            if let Some(parent) = dest.parent() {
                std::fs::create_dir_all(parent)?;
            }
            let mut out = File::create(&dest)?;
            std::io::copy(&mut (&mut *reader).take(filesize), &mut out)?;
            skip_pad(&mut *reader, filesize as usize)?;
            #[cfg(unix)]
            {
                use std::os::unix::fs::PermissionsExt;
                let _ = std::fs::set_permissions(&dest, std::fs::Permissions::from_mode(mode & 0o7777));
            }
            files.push(PathBuf::from("/").join(rel_path));
            total_size += filesize;
        } else {
            // Device nodes, fifos, sockets: cannot (and need not) be restored
            tracing::debug!("Skipping special cpio entry: {} (mode {:o})", name, mode);
            skip_bytes(&mut *reader, filesize)?;
            skip_pad(&mut *reader, filesize as usize)?;
        }
    }

    Ok((files, total_size))
}

/// Skip cpio 4-byte alignment padding after `len` bytes of payload
fn skip_pad(reader: &mut dyn Read, len: usize) -> Result<()> {
    let pad = (4 - len % 4) % 4;
    skip_bytes(reader, pad as u64)
}

/// Discard exactly `n` bytes from the reader
fn skip_bytes(reader: &mut dyn Read, n: u64) -> Result<()> {
    std::io::copy(&mut (&mut *reader).take(n), &mut std::io::sink())
        .map_err(|e| RexebError::Extraction(e.to_string()))?;
    Ok(())
}

//! End-to-end tests with programmatically built fixture `.deb` packages
//!
//! Fixtures are real `ar` archives (hand-rolled framing around
//! `tar`+`gzip` members), so they exercise the same code path as user
//! downloads. All tests are hermetic: no network access.

use std::io::{Read, Write};
use std::path::{Path, PathBuf};

use rexeb::models::{Dependency, DependencyType, PackageMetadata};

/// gzip a set of `(path, content)` entries into a tar archive
fn tar_gz(entries: &[(&str, &[u8])]) -> Vec<u8> {
    let encoder = flate2::write::GzEncoder::new(Vec::new(), flate2::Compression::default());
    let mut tar = tar::Builder::new(encoder);
    for (name, data) in entries {
        let mut header = tar::Header::new_gnu();
        header.set_size(data.len() as u64);
        header.set_mode(0o644);
        header.set_mtime(0);
        header.set_cksum();
        tar.append_data(&mut header, name, *data).unwrap();
    }
    tar.into_inner().unwrap().finish().unwrap()
}

/// Append one member to an `ar` archive (System V/GNU header layout:
// name(16) mtime(12) uid(6) gid(6) mode(8) size(10) magic(2))
fn ar_member(out: &mut Vec<u8>, name: &str, data: &[u8]) {
    let mut header = [b' '; 60];
    let gnu_name = format!("{}/", name);
    let name_bytes = gnu_name.as_bytes();
    let name_len = name_bytes.len().min(16);
    header[..name_len].copy_from_slice(&name_bytes[..name_len]);

    let put = |header: &mut [u8; 60], offset: usize, width: usize, value: &str| {
        let bytes = value.as_bytes();
        let len = bytes.len().min(width);
        header[offset..offset + len].copy_from_slice(&bytes[..len]);
    };
    put(&mut header, 16, 12, "0");
    put(&mut header, 28, 6, "0");
    put(&mut header, 34, 6, "0");
    put(&mut header, 40, 8, "100644");
    put(&mut header, 48, 10, &data.len().to_string());
    header[58] = b'`';
    header[59] = b'\n';

    out.extend_from_slice(&header);
    out.extend_from_slice(data);
    if data.len() % 2 == 1 {
        out.push(b'\n');
    }
}

/// Build a minimal but valid `.deb` in `dir`
fn make_fixture_deb(
    dir: &Path,
    name: &str,
    version: &str,
    depends: Option<&str>,
) -> PathBuf {
    let mut control = format!(
        "Package: {}\nVersion: {}\nSection: test\nPriority: optional\nArchitecture: amd64\nMaintainer: Rexeb Test <test@example.com>\nDescription: Fixture package for rexeb integration tests\n",
        name, version
    );
    if let Some(d) = depends {
        control.push_str(&format!("Depends: {}\n", d));
    }
    let script = format!("#!/bin/sh\necho {}\n", name);
    let control_tar = tar_gz(&[("control", control.as_bytes())]);
    let data_tar = tar_gz(&[(format!("usr/bin/{}", name).as_str(), script.as_bytes())]);

    let mut deb = b"!<arch>\n".to_vec();
    ar_member(&mut deb, "debian-binary", b"2.0\n");
    ar_member(&mut deb, "control.tar.gz", &control_tar);
    ar_member(&mut deb, "data.tar.gz", &data_tar);

    let path = dir.join(format!("{}_{}_amd64.deb", name, version));
    let mut file = std::fs::File::create(&path).unwrap();
    file.write_all(&deb).unwrap();
    path
}

/// Scratch dir + fixture `.deb` (dir is deleted when the guard drops)
fn fixture(name: &str, version: &str, depends: Option<&str>) -> (tempfile::TempDir, PathBuf) {
    let dir = tempfile::Builder::new()
        .prefix("rexeb-test-")
        .tempdir()
        .unwrap();
    let deb = make_fixture_deb(dir.path(), name, version, depends);
    (dir, deb)
}

/// Read `.PKGINFO` back out of a built `.pkg.tar.zst`
fn read_pkginfo_from_package(path: &Path) -> String {
    let file = std::fs::File::open(path).unwrap();
    let decoder = zstd::stream::read::Decoder::new(file).unwrap();
    let mut archive = tar::Archive::new(decoder);
    for entry in archive.entries().unwrap() {
        let mut entry = entry.unwrap();
        let is_pkginfo = entry
            .path()
            .map(|p| p.to_string_lossy().contains(".PKGINFO"))
            .unwrap_or(false);
        if is_pkginfo {
            let mut content = String::new();
            entry.read_to_string(&mut content).unwrap();
            return content;
        }
    }
    panic!(".PKGINFO not found in {}", path.display());
}

#[test]
fn parse_fixture_metadata() {
    let (_tmp, deb) = fixture("weebots", "1.2.3-1", Some("libc6 (>= 2.34), weebots-data | curl"));
    let parser = rexeb::parsers::deb::DebParser::new(&deb).unwrap();
    let metadata = parser.parse().unwrap();

    assert_eq!(metadata.name, "weebots");
    assert_eq!(metadata.version, "1.2.3-1");
    let deps = metadata.get_deps(DependencyType::Depends);
    assert_eq!(deps.len(), 2);
    assert_eq!(deps[0].debian_name, "libc6");
    assert_eq!(deps[1].debian_name, "weebots-data");
    assert_eq!(deps[1].alternatives.len(), 1);
    assert_eq!(deps[1].alternatives[0].debian_name, "curl");
    assert!(!metadata.files.is_empty());
}

#[test]
fn pkginfo_drops_self_and_unmapped_dependencies() {
    // Regression test for the reported bug: a self-reference or unmapped
    // Debian name must never reach `.PKGINFO` as `depend = ...`, or
    // `pacman -U` fails with "couldn't resolve dependency".
    let mut metadata = PackageMetadata::new("weebots", "1.0");

    // Mapped straight back at the package itself
    let mut self_dep = Dependency::new("weebots");
    self_dep.set_arch_name("weebots", 1.0);
    // Never mapped at all
    let unmapped = Dependency::new("weebots-data");
    // A healthy mapped dependency
    let mut good = Dependency::new("curl");
    good.set_arch_name("curl", 1.0);

    metadata
        .dependencies
        .entry(DependencyType::Depends)
        .or_default()
        .extend([self_dep, unmapped, good]);

    let notes = rexeb::resolver::DependencyResolver::prune_self_references(&mut metadata);
    assert!(
        notes.iter().any(|n| n.contains("self-reference")),
        "expected a self-reference prune note, got {:?}",
        notes
    );

    let pkginfo = metadata.to_pkginfo();
    assert!(
        !pkginfo.contains("depend = weebots"),
        "self-dependency leaked into .PKGINFO:\n{}",
        pkginfo
    );
    assert!(
        !pkginfo.contains("weebots-data"),
        "unmapped dependency leaked into .PKGINFO:\n{}",
        pkginfo
    );
    assert!(
        pkginfo.contains("depend = curl"),
        "mapped dependency missing from .PKGINFO:\n{}",
        pkginfo
    );
}

#[test]
fn prune_promotes_usable_alternative() {
    let mut metadata = PackageMetadata::new("weebots", "1.0");
    // `Depends: weebots | curl` where only the alternative resolved
    let mut primary = Dependency::parse("weebots | curl").unwrap();
    assert_eq!(primary.alternatives.len(), 1);
    primary.alternatives[0].set_arch_name("curl", 1.0);
    metadata
        .dependencies
        .entry(DependencyType::Depends)
        .or_default()
        .push(primary);

    let notes = rexeb::resolver::DependencyResolver::prune_self_references(&mut metadata);
    assert!(
        notes.iter().any(|n| n.contains("alternative")),
        "expected an alternative-promotion note, got {:?}",
        notes
    );
    assert!(metadata.to_pkginfo().contains("depend = curl"));
}

#[test]
fn prune_drops_versioned_self_conflicts() {
    // Debian upgrade guards (`Breaks: weebots (<< 2.0)`) are meaningless on
    // Arch and would make the transaction unresolvable.
    let mut metadata = PackageMetadata::new("weebots", "2.0");
    let guard = Dependency::parse("weebots (<< 2.0)").unwrap();
    metadata
        .dependencies
        .entry(DependencyType::Breaks)
        .or_default()
        .push(guard);

    let notes = rexeb::resolver::DependencyResolver::prune_self_references(&mut metadata);
    assert_eq!(notes.len(), 1);
    assert!(!metadata.to_pkginfo().contains("conflict = weebots"));
}

#[test]
fn convert_fixture_end_to_end() {
    let (_tmp, deb) = fixture("weebots", "2.0-1", Some("weebots, libc6 (>= 2.34)"));
    let parser = rexeb::parsers::deb::DebParser::new(&deb).unwrap();
    let mut metadata = parser.parse().unwrap();

    // Simulate resolution without network: map libc6, leave everything
    // else untouched, then prune.
    for deps in metadata.dependencies.values_mut() {
        for dep in deps.iter_mut() {
            if dep.debian_name == "libc6" {
                dep.set_arch_name("glibc", 1.0);
            }
        }
    }
    let notes = rexeb::resolver::DependencyResolver::prune_self_references(&mut metadata);
    assert!(notes.iter().any(|n| n.contains("self-reference")));

    let out = tempfile::Builder::new()
        .prefix("rexeb-test-")
        .tempdir()
        .unwrap();
    let converter = rexeb::converter::PackageConverter::new(metadata, parser.extract_dir())
        .unwrap()
        .with_overwrite(true);
    let package = converter
        .build(out.path(), rexeb::cli::OutputFormat::PkgTarZst)
        .unwrap();
    assert!(package.exists());

    let pkginfo = read_pkginfo_from_package(&package);
    assert!(pkginfo.contains("pkgname = weebots"));
    assert!(
        !pkginfo.contains("depend = weebots"),
        "self-dependency in built package:\n{}",
        pkginfo
    );
    assert!(!pkginfo.contains("libc6"), "raw Debian name leaked:\n{}", pkginfo);
    assert!(pkginfo.contains("depend = glibc"));
}

#[tokio::test]
async fn dry_run_writes_nothing() {
    // No Depends entry: resolve() touches no network, keeping this hermetic.
    let (_tmp, deb) = fixture("weebots", "1.0-1", None);
    let out = tempfile::Builder::new()
        .prefix("rexeb-test-")
        .tempdir()
        .unwrap();
    let args = rexeb::cli::ConvertArgs {
        input: vec![deb],
        output: Some(out.path().to_path_buf()),
        skip_deps: false,
        force: false,
        pkgbuild: false,
        yes: true,
        interactive: false,
        dry_run: true,
        sign: false,
        sign_key: None,
        pseudo64: false,
        keep_temp: false,
        sandbox: false,
        sandbox_backend: None,
        name: None,
        version_override: None,
        release: None,
        format: None,
    };
    rexeb::cli::execute_convert(&args, true, Some(1))
        .await
        .unwrap();
    assert!(
        std::fs::read_dir(out.path()).unwrap().next().is_none(),
        "dry run must not write anything"
    );
}

// ---------------------------------------------------------------------------
// RPM fixtures + tests
// ---------------------------------------------------------------------------

/// Build a `newc` cpio archive from `(path, content, mode)` entries
fn cpio_newc(entries: &[(&str, &[u8], u32)]) -> Vec<u8> {
    fn entry(out: &mut Vec<u8>, name: &str, data: &[u8], mode: u32) {
        out.extend_from_slice(
            format!(
                "070701{:08x}{:08x}{:08x}{:08x}{:08x}{:08x}{:08x}{:08x}{:08x}{:08x}{:08x}{:08x}{:08x}",
                0u32, // ino
                mode, 0u32, 0u32, // uid gid
                1u32, 0u32, // nlink mtime
                data.len() as u32, // filesize
                0u32, 0u32, 0u32, 0u32, // devmajor devminor rdevmajor rdevminor
                name.len() as u32 + 1, // namesize (with NUL)
                0u32, // check
            )
            .as_bytes(),
        );
        out.extend_from_slice(name.as_bytes());
        out.push(0);
        while out.len() % 4 != 0 {
            out.push(0);
        }
        out.extend_from_slice(data);
        while out.len() % 4 != 0 {
            out.push(0);
        }
    }

    let mut out = Vec::new();
    for (name, data, mode) in entries {
        entry(&mut out, name, data, *mode);
    }
    entry(&mut out, "TRAILER!!!", &[], 0);
    out
}

/// Encode an RPM header section from `(tag, type, count, store-bytes)` entries
fn rpm_header(entries: &[(u32, u32, u32, Vec<u8>)]) -> Vec<u8> {
    let mut index = Vec::new();
    let mut store: Vec<u8> = Vec::new();
    for (tag, typ, count, data) in entries {
        if *typ == 4 {
            // int32 values are 4-aligned in the store
            while store.len() % 4 != 0 {
                store.push(0);
            }
        }
        let offset = store.len() as u32;
        index.extend_from_slice(&tag.to_be_bytes());
        index.extend_from_slice(&typ.to_be_bytes());
        index.extend_from_slice(&offset.to_be_bytes());
        index.extend_from_slice(&count.to_be_bytes());
        store.extend_from_slice(data);
    }
    let mut out = vec![0x8e, 0xad, 0xe8, 0x01, 0, 0, 0, 0];
    out.extend_from_slice(&(entries.len() as u32).to_be_bytes());
    out.extend_from_slice(&(store.len() as u32).to_be_bytes());
    out.extend_from_slice(&index);
    out.extend_from_slice(&store);
    // Header records are padded to 8-byte alignment
    while out.len() % 8 != 0 {
        out.push(0);
    }
    out
}

fn rpm_str(s: &str) -> Vec<u8> {
    let mut v = s.as_bytes().to_vec();
    v.push(0);
    v
}

fn rpm_str_array(items: &[&str]) -> Vec<u8> {
    let mut v = Vec::new();
    for item in items {
        v.extend_from_slice(item.as_bytes());
        v.push(0);
    }
    v
}

fn rpm_ints(values: &[u32]) -> Vec<u8> {
    let mut v = Vec::new();
    for value in values {
        v.extend_from_slice(&value.to_be_bytes());
    }
    v
}

/// Build a minimal but valid `.rpm` in `dir` (gzip-compressed cpio payload)
fn make_fixture_rpm(
    dir: &Path,
    name: &str,
    version: &str,
    release: &str,
    arch: &str,
) -> PathBuf {
    let bin_path = format!("usr/bin/{}", name);
    let link_path = format!("usr/bin/{}-link", name);
    let script = format!("#!/bin/sh\necho {}\n", name);
    let cpio = cpio_newc(&[
        ("usr", &[], 0o040755),
        ("usr/bin", &[], 0o040755),
        (bin_path.as_str(), script.as_bytes(), 0o100755),
        (link_path.as_str(), name.as_bytes(), 0o120777),
    ]);

    let mut encoder =
        flate2::write::GzEncoder::new(Vec::new(), flate2::Compression::default());
    encoder.write_all(&cpio).unwrap();
    let payload = encoder.finish().unwrap();

    // Signature header: a single SIZE entry (tag 1000, int32)
    let sig = rpm_header(&[(1000, 4, 1, rpm_ints(&[payload.len() as u32]))]);

    let requires = [
        "rpmlib(PayloadFilesHavePrefix)",
        "config(fixture)",
        "rtld(GNU_HASH)",
        "group(fixture)",
        "/nonexistent/path/dep",
    ];
    let main = rpm_header(&[
        (1000, 6, 1, rpm_str(name)),
        (1001, 6, 1, rpm_str(version)),
        (1002, 6, 1, rpm_str(release)),
        (1004, 6, 1, rpm_str("Fixture package")),
        (
            1005,
            6,
            1,
            rpm_str("Fixture package for rexeb integration tests"),
        ),
        (1014, 6, 1, rpm_str("MIT")),
        (1015, 6, 1, rpm_str("Rexeb Test <test@example.com>")),
        (1022, 6, 1, rpm_str(arch)),
        (1027, 6, 1, rpm_str("echo postinstall")),
        (1028, 6, 1, rpm_str("/bin/sh")),
        (
            1047,
            8,
            2,
            rpm_str_array(&[name, &format!("{}({})", name, arch)]),
        ),
        (1048, 4, requires.len() as u32, rpm_ints(&vec![0u32; requires.len()])),
        (1049, 8, requires.len() as u32, rpm_str_array(&requires)),
        (1050, 8, requires.len() as u32, rpm_str_array(&vec![""; requires.len()])),
        (1124, 6, 1, rpm_str("cpio")),
        (1125, 6, 1, rpm_str("gzip")),
    ]);

    // Lead: magic + major/minor + type + archnum + name + osnum + sigtype
    let mut rpm = vec![0xed, 0xab, 0xee, 0xdb, 3, 0, 0, 0];
    rpm.extend_from_slice(&1u16.to_be_bytes());
    let mut name_field = [0u8; 66];
    let copy_len = name.len().min(65);
    name_field[..copy_len].copy_from_slice(&name.as_bytes()[..copy_len]);
    rpm.extend_from_slice(&name_field);
    rpm.extend_from_slice(&1u16.to_be_bytes());
    rpm.extend_from_slice(&5u16.to_be_bytes());
    rpm.extend_from_slice(&[0u8; 16]);
    assert_eq!(rpm.len(), 96);
    rpm.extend_from_slice(&sig);
    rpm.extend_from_slice(&main);
    rpm.extend_from_slice(&payload);

    let path = dir.join(format!("{}-{}-{}.{}.rpm", name, version, release, arch));
    std::fs::write(&path, &rpm).unwrap();
    path
}

fn rpm_fixture(
    name: &str,
    version: &str,
    release: &str,
    arch: &str,
) -> (tempfile::TempDir, PathBuf) {
    let dir = tempfile::Builder::new()
        .prefix("rexeb-test-")
        .tempdir()
        .unwrap();
    let rpm = make_fixture_rpm(dir.path(), name, version, release, arch);
    (dir, rpm)
}

#[test]
fn parse_fixture_rpm_metadata() {
    let (_tmp, rpm) = rpm_fixture("weebots", "1.2.3", "1.fc39", "x86_64");
    assert!(rexeb::parsers::rpm::is_rpm(&rpm));

    let parser = rexeb::parsers::rpm::RpmParser::new(&rpm).unwrap();
    let metadata = parser.parse().unwrap();

    assert_eq!(metadata.name, "weebots");
    assert_eq!(metadata.version, "1.2.3");
    assert_eq!(metadata.release, "1.fc39");

    // Environment noise (rpmlib/config/rtld/group) is skipped; the
    // nonexistent path require survives as an (unmapped) dependency.
    let deps = metadata.get_deps(DependencyType::Depends);
    assert_eq!(deps.len(), 1);
    assert_eq!(deps[0].debian_name, "/nonexistent/path/dep");
    assert!(deps[0].arch_name.is_none());

    // Self-provides are recorded (pruning happens at resolve time)
    let provides = metadata.get_deps(DependencyType::Provides);
    assert!(provides.iter().any(|d| d.debian_name == "weebots"));

    // Scriptlets map to maintainer scripts
    assert!(metadata.scripts.contains_key(
        &rexeb::models::MaintainerScript::PostInst
    ));

    // Payload: regular file + symlink restored, dirs created
    let data = parser.extract_dir();
    assert!(data.join("usr/bin/weebots").is_file());
    assert_eq!(
        metadata.files.iter().filter(|f| f.ends_with("usr/bin/weebots")).count(),
        1
    );
    assert!(metadata.installed_size > 0);
}

#[test]
fn rpm_end_to_end_convert() {
    let (_tmp, rpm) = rpm_fixture("weebots", "2.0", "3.fc40", "x86_64");
    let parser = rexeb::parsers::rpm::RpmParser::new(&rpm).unwrap();
    let mut metadata = parser.parse().unwrap();
    metadata.normalize_version();

    // Fedora release `3.fc40` collapses to Arch pkgrel `3`
    assert_eq!(metadata.version, "2.0");
    assert_eq!(metadata.release, "3");

    let out = tempfile::Builder::new()
        .prefix("rexeb-test-")
        .tempdir()
        .unwrap();
    let converter = rexeb::converter::PackageConverter::new(metadata, parser.extract_dir())
        .unwrap()
        .with_overwrite(true);
    let package = converter
        .build(out.path(), rexeb::cli::OutputFormat::PkgTarZst)
        .unwrap();
    assert!(package.exists());

    // Junk requires must never leak into the built package
    let pkginfo = read_pkginfo_from_package(&package);
    assert!(pkginfo.contains("pkgname = weebots"));
    assert!(pkginfo.contains("pkgver = 2.0-3"));
    assert!(!pkginfo.contains("rpmlib"));
    assert!(!pkginfo.contains("rtld"));
}

#[test]
fn rpm_rejects_source_packages() {
    let (_tmp, rpm) = rpm_fixture("weebots", "1.0", "1", "src");
    let parser = rexeb::parsers::rpm::RpmParser::new(&rpm).unwrap();
    let err = parser.parse().unwrap_err();
    assert!(
        err.to_string().contains("Source RPM"),
        "unexpected error: {}",
        err
    );
}

#[test]
fn detect_and_create_dispatches_on_magic() {
    let (_tmp_deb, deb) = fixture("weebots", "1.0-1", None);
    let (_tmp_rpm, rpm) = rpm_fixture("weebots", "1.0", "1", "x86_64");

    // Even with swapped extensions, magic bytes win
    let deb_dir = tempfile::Builder::new().prefix("rexeb-test-").tempdir().unwrap();
    let rpm_dir = tempfile::Builder::new().prefix("rexeb-test-").tempdir().unwrap();
    let renamed_deb = deb_dir.path().join("package.rpm");
    let renamed_rpm = rpm_dir.path().join("package.deb");
    std::fs::copy(&deb, &renamed_deb).unwrap();
    std::fs::copy(&rpm, &renamed_rpm).unwrap();

    let parser = rexeb::parsers::detect_and_create(&renamed_deb).unwrap();
    assert_eq!(parser.format(), rexeb::models::PackageFormat::Deb);
    let parser = rexeb::parsers::detect_and_create(&renamed_rpm).unwrap();
    assert_eq!(parser.format(), rexeb::models::PackageFormat::Rpm);
}

// ---------------------------------------------------------------------------
// AppImage helper tests (pure functions — no runtime execution)
// ---------------------------------------------------------------------------

#[test]
fn appimage_magic_detection() {
    let dir = tempfile::Builder::new().prefix("rexeb-test-").tempdir().unwrap();

    // Minimal AppImage header: ELF + AI\x02 at offset 8
    let mut appimage = vec![0x7f, b'E', b'L', b'F', 2, 1, 1, 0, b'A', b'I', 2, 0];
    appimage.extend_from_slice(&[0u8; 8]); // e_type + e_machine = 0 (unknown)
    let app_path = dir.path().join("app");
    std::fs::write(&app_path, &appimage).unwrap();
    assert!(rexeb::parsers::appimage::is_appimage(&app_path));
    assert!(rexeb::parsers::appimage::arch_from_elf(&app_path).is_none());

    // Same shape but x86-64 e_machine
    appimage[18] = 62;
    appimage[19] = 0;
    let app64_path = dir.path().join("app64");
    std::fs::write(&app64_path, &appimage).unwrap();
    assert_eq!(
        rexeb::parsers::appimage::arch_from_elf(&app64_path),
        Some(rexeb::models::Architecture::X86_64)
    );

    let not_app = dir.path().join("not-app");
    std::fs::write(&not_app, b"hello world, not an appimage").unwrap();
    assert!(!rexeb::parsers::appimage::is_appimage(&not_app));
}

#[test]
fn appimage_filename_splitting() {
    use rexeb::parsers::appimage::split_appimage_filename;

    assert_eq!(
        split_appimage_filename("Foo-1.2.3-x86_64"),
        ("foo".to_string(), Some("1.2.3".to_string()))
    );
    assert_eq!(
        split_appimage_filename("my-app_2.0"),
        ("my-app".to_string(), Some("2.0".to_string()))
    );
    assert_eq!(
        split_appimage_filename("tool-x86_64"),
        ("tool".to_string(), None)
    );
    assert_eq!(
        split_appimage_filename("1.2.3-x86_64"),
        ("appimage-app".to_string(), Some("1.2.3".to_string()))
    );
    assert_eq!(
        split_appimage_filename("Cool Tool-3.0-beta1"),
        ("cool-tool".to_string(), Some("3.0".to_string()))
    );
    assert_eq!(
        rexeb::parsers::appimage::sanitize_pkgname("Foo Bar!!"),
        "foo-bar".to_string()
    );
    assert_eq!(
        rexeb::parsers::appimage::sanitize_pkgname(""),
        "appimage-app".to_string()
    );
}

#[test]
fn appimage_stage_rewrites_desktop_and_icon() {
    use rexeb::models::Architecture;

    // Fake extracted AppDir
    let approot = tempfile::Builder::new().prefix("rexeb-test-").tempdir().unwrap();
    let approot = approot.path().join("squashfs-root");
    std::fs::create_dir_all(&approot).unwrap();
    std::fs::write(approot.join("AppRun"), b"#!/bin/sh\nexec app\n").unwrap();
    std::fs::write(
        approot.join("Foo Bar.desktop"),
        "[Desktop Entry]\nName=Foo Bar\nExec=AppRun %F\nIcon=foobar\nComment=Does foo\nCategories=Utility\n",
    )
    .unwrap();
    std::fs::write(approot.join("foobar.png"), b"fakepng").unwrap();

    let meta = rexeb::parsers::appimage::extract_metadata(
        &approot,
        "Foo-Bar-1.2.3-x86_64",
        Architecture::X86_64,
    )
    .unwrap();
    assert_eq!(meta.name, "foo-bar");
    assert_eq!(meta.version, "1.2.3");
    assert_eq!(meta.description, "Does foo");
    assert!(meta.icon_source.is_some());

    let data = tempfile::Builder::new().prefix("rexeb-test-").tempdir().unwrap();
    let (files, size) = rexeb::parsers::appimage::stage_appdir(&approot, data.path(), &meta).unwrap();
    assert!(size > 0);

    // Full AppDir under /opt/<name>
    assert!(data.path().join("opt/foo-bar/AppRun").is_file());
    // Rewritten desktop entry
    let desktop = std::fs::read_to_string(data.path().join("usr/share/applications/foo-bar.desktop")).unwrap();
    assert!(desktop.contains("Exec=/opt/foo-bar/AppRun %F"), "{}", desktop);
    assert!(desktop.contains("Icon=foo-bar"), "{}", desktop);
    assert!(desktop.contains("Type=Application"), "{}", desktop);
    assert!(desktop.contains("Categories=Utility;"), "{}", desktop);
    assert!(desktop.contains("Name=Foo Bar"), "{}", desktop);
    // Staged icon
    assert!(data.path().join("usr/share/pixmaps/foo-bar.png").is_file());
    // File list uses absolute install paths
    assert!(files.iter().any(|f| f == &PathBuf::from("/opt/foo-bar/AppRun")));
    assert!(files.iter().any(|f| f == &PathBuf::from("/usr/share/applications/foo-bar.desktop")));
}

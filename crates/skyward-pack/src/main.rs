use anyhow::{bail, ensure, Context, Result};
use clap::{Parser, Subcommand};
use sha2::{Digest, Sha256};
use skyward_config::ramdisk as paths;
use std::fs::File;
use std::io::{Read, Seek, SeekFrom, Write};
use std::path::{Path, PathBuf};

mod avb;

use skyward_bootimg::{BootImage, Ramdisk, VendorBoot};

/// Builds Skyward recovery images from a device's own stock images.
#[derive(Parser)]
#[command(name = "skyward-pack")]
struct Cli {
    #[command(subcommand)]
    cmd: Cmd,
}

#[derive(Subcommand)]
enum Cmd {
    /// Print an image's header and the files in its ramdisk.
    List { image: PathBuf },
    /// Write one ramdisk file to stdout.
    Cat { image: PathBuf, path: String },
    /// Unpack and pack an image's ramdisk unchanged, to check that a rebuilt
    /// archive keeps every entry.
    Repack { image: PathBuf, out: PathBuf },
    /// Join a kernel and another image's ramdisk into one boot image.
    Boot {
        /// Image holding the kernel (the device's boot partition).
        #[arg(long)]
        kernel: PathBuf,
        /// Image holding the ramdisk.
        #[arg(long)]
        ramdisk: PathBuf,
        #[arg(long)]
        out: PathBuf,
    },
    /// Build a Skyward recovery image on top of the device's stock recovery:
    /// its init, adbd, SELinux policy and libraries stay, the recovery
    /// service becomes Skyward. The image fills the whole partition and ends
    /// in an unsigned AVB footer of its own.
    Recovery {
        /// The device's stock recovery image: the whole partition, as the
        /// phone or a full update package holds it, ending in its AVB footer.
        #[arg(long)]
        stock: PathBuf,
        /// Skyward built for the device (aarch64-linux-android).
        #[arg(long)]
        binary: PathBuf,
        /// The first program of a system installed beside the installed
        /// one, skyward-system-init built for the device and linked
        /// statically.
        #[arg(long)]
        system_init: PathBuf,
        /// The device configuration, devices/<codename>.toml.
        #[arg(long)]
        config: PathBuf,
        /// The UI typeface.
        #[arg(long)]
        font: PathBuf,
        /// The terminal's monospaced typeface.
        #[arg(long)]
        mono: PathBuf,
        /// The monospaced typeface's licence, which goes with it.
        #[arg(long)]
        mono_license: PathBuf,
        #[arg(long)]
        out: PathBuf,
    },
    /// Print the stock build a device's recovery is made on, as lines of
    /// name=value: its version, where its full update package is, and the
    /// SHA-256 of the recovery partition in that package. Given somewhere
    /// to write it, also take that partition out of the package on its
    /// server, reading only what the partition takes, and write it there
    /// once its SHA-256 is that one.
    Stock {
        /// The device configuration, devices/<codename>.toml.
        #[arg(long)]
        config: PathBuf,
        /// Where to write the stock recovery image.
        #[arg(long)]
        out: Option<PathBuf>,
    },
}

fn main() -> Result<()> {
    match Cli::parse().cmd {
        Cmd::List { image } => list(&image),
        Cmd::Cat { image, path } => {
            let rd = Ramdisk::decode(&BootImage::read(&image)?.ramdisk)?;
            let e = rd.find(&path).with_context(|| format!("{path} is not in the ramdisk"))?;
            std::io::stdout().write_all(&e.data)?;
            Ok(())
        }
        Cmd::Repack { image, out } => {
            let mut img = BootImage::read(&image)?;
            img.ramdisk = Ramdisk::decode(&img.ramdisk)?.encode()?;
            write(&out, &img, &img.to_bytes()?)
        }
        Cmd::Boot { kernel, ramdisk, out } => {
            let mut img = BootImage::read(&ramdisk)?;
            img.kernel = BootImage::read(&kernel)?.kernel;
            write(&out, &img, &img.to_bytes()?)
        }
        Cmd::Recovery { stock, binary, system_init, config, font, mono, mono_license, out } => {
            let size = partition_size(&stock)?;
            let build = Build { binary: &binary, system_init: &system_init, config: &config, font: &font, mono: &mono, mono_license: &mono_license };
            skyward_config::DeviceConfig::load(&config)?;
            if starts_with(&stock, b"VNDRBOOT")? {
                // A phone that keeps its recovery root in vendor_boot has no
                // recovery partition to fill: the stock image is the whole
                // vendor_boot partition, and the ramdisk among its fragments
                // is that root.
                let mut img = VendorBoot::read(&stock)?;
                let (at, rd) = img
                    .fragments
                    .iter()
                    .enumerate()
                    .find_map(|(i, f)| Ramdisk::decode(&f.data).ok().map(|rd| (i, rd)))
                    .with_context(|| format!("no ramdisk in {} unpacks", stock.display()))?;
                ensure!(rd.find(paths::RECOVERY).is_some(), "the ramdisk in {} holds no {}", stock.display(), paths::RECOVERY);
                let rd = skyward_on(rd, &build).with_context(|| format!("build on {}", stock.display()))?;
                img.fragments[at].data = rd.encode()?;
                let bytes = img.to_bytes()?;
                let ramdisk: usize = img.fragments.iter().map(|f| f.data.len()).sum();
                let image = avb::footed(&bytes, "vendor_boot", size, &Sha256::digest(&bytes))?;
                write_raw(&out, &image, &format!("header and {ramdisk} bytes ramdisk, {} bytes in all", image.len()))
            } else {
                let mut img = BootImage::read(&stock)?;
                let rd = skyward_on(Ramdisk::decode(&img.ramdisk)?, &build).with_context(|| format!("build on {}", stock.display()))?;
                img.ramdisk = rd.encode()?;
                let bytes = img.to_bytes()?;
                write(&out, &img, &avb::footed(&bytes, "recovery", size, &Sha256::digest(&bytes))?)
            }
        }
        Cmd::Stock { config, out } => {
            let stock = skyward_config::DeviceConfig::load(&config)?.stock.with_context(|| format!("{} names no stock build", config.display()))?;
            println!("version={}\npackage={}\nrecovery={}", stock.version, stock.package, stock.recovery);
            match out {
                Some(out) => take_stock_recovery(&stock, &out),
                None => Ok(()),
            }
        }
    }
}

/// What a recovery image is made from beyond the device's stock ramdisk.
struct Build<'a> {
    binary: &'a PathBuf,
    system_init: &'a PathBuf,
    config: &'a PathBuf,
    font: &'a PathBuf,
    mono: &'a PathBuf,
    mono_license: &'a PathBuf,
}

/// The stock recovery ramdisk with Skyward in the recovery service's place:
/// its commands, its first-boot program, its configuration, its typefaces,
/// and the properties its programs read.
fn skyward_on(mut rd: Ramdisk, build: &Build) -> Result<Ramdisk> {
    // A stock recovery ramdisk has no third-party tree; refuse to build
    // on top of another custom recovery.
    if let Some(e) = rd.entries.iter().find(|e| e.name == "FFiles" || e.name.starts_with("twres")) {
        bail!("the ramdisk holds {}, which is not a stock recovery", e.name);
    }
    let read = |p: &PathBuf| std::fs::read(p).with_context(|| format!("read {}", p.display()));
    // The phone's own recovery stays beside Skyward's: when Skyward's screen
    // cannot come on, it hands the phone over to that one, so an unlit
    // screen is still a recovery rather than a dark phone.
    let stock_recovery = rd.find(paths::RECOVERY).map(|e| e.data.clone());
    rd.replace(paths::RECOVERY, read(build.binary)?)?;
    if let Some(data) = stock_recovery {
        rd.add(STOCK_RECOVERY, 0o755, data)?;
    }
    // Install scripts look for the commands a stock recovery lacks
    // among its programs, and its unzip there too, a link to ziptool
    // until it points here; the recovery binary is each of them.
    let (_, recovery) = paths::RECOVERY.rsplit_once('/').expect("the recovery binary is in a folder");
    for command in paths::COMMANDS {
        rd.link(command, recovery)?;
    }
    rd.add(paths::SYSTEM_INIT, 0o755, read(build.system_init)?)?;
    rd.add(paths::CONFIG, 0o644, read(build.config)?)?;
    rd.add(paths::FONT, 0o644, read(build.font)?)?;
    rd.add(paths::MONO, 0o644, read(build.mono)?)?;
    rd.add(paths::MONO_LICENSE, 0o644, read(build.mono_license)?)?;
    // A stock ramdisk may run logd without any scheduling profiles,
    // and logd stops as soon as it cannot find the one it asks for.
    if rd.find(paths::TASK_PROFILES).is_none() {
        rd.add(paths::TASK_PROFILES, 0o644, TASK_PROFILES.to_vec())?;
    }
    rd.add(paths::MTP_RC, 0o644, MTP_RC.to_vec())?;
    // On an unlocked device adbd keeps root when ro.secure is 0 and
    // lets any computer in without authorization when ro.adb.secure is
    // 0; init keeps the first line it reads for a name, so the stock
    // lines are taken out rather than overridden by the ones below.
    // update_engine refuses a package built before the recovery
    // itself unless both of the last two are set; the recovery checks
    // packages against the installed system instead.
    let stock_props = &rd
        .find(paths::PROP_DEFAULT)
        .with_context(|| format!("{} is not in the ramdisk", paths::PROP_DEFAULT))?
        .data;
    let mut props = without(stock_props, &BUILD_VERSION);
    props = without(&props, &ADBD);
    props.extend_from_slice(b"\nro.secure=0\nro.adb.secure=0\nro.debuggable=1\nro.ota.allow_downgrade=true\n");
    rd.replace(paths::PROP_DEFAULT, props)?;
    // init starts the recovery service again the moment it ends, and a
    // recovery that ends as it comes up — with a panic, or a fault in the
    // display — would be started again and again, taking the port down
    // with every start. Started once, the phone keeps what the last start
    // left it: a port to reach it by, and a screen that says what it can.
    once(&mut rd)?;
    Ok(rd)
}

/// Whether `path` starts with `magic`.
fn starts_with(path: &Path, magic: &[u8]) -> Result<bool> {
    let mut file = File::open(path).with_context(|| format!("open {}", path.display()))?;
    let mut head = vec![0u8; magic.len()];
    match file.read_exact(&mut head) {
        Ok(()) => Ok(head == magic),
        Err(e) if e.kind() == std::io::ErrorKind::UnexpectedEof => Ok(false),
        Err(e) => Err(e).with_context(|| format!("read {}", path.display())),
    }
}

/// Write `bytes` to `out`, saying what the image holds.
fn write_raw(out: &Path, bytes: &[u8], note: &str) -> Result<()> {
    std::fs::write(out, bytes).with_context(|| format!("write {}", out.display()))?;
    println!("wrote {} ({note})", out.display());
    Ok(())
}

/// The partition a recovery is made on, as a full update package names it.
const RECOVERY: &str = "recovery";

/// Take the recovery partition out of the stock build's full update package
/// on its server, reading only what that partition takes of it, and write it
/// to `out` once its SHA-256 is the one the configuration names.
fn take_stock_recovery(stock: &skyward_config::Stock, out: &Path) -> Result<()> {
    let parent = out.parent().filter(|p| !p.as_os_str().is_empty()).unwrap_or(Path::new("."));
    std::fs::create_dir_all(parent).with_context(|| format!("make {}", parent.display()))?;
    // Taken out beside where it goes, so it is moved there whole.
    let work = tempfile::Builder::new().prefix(".stock").tempdir_in(parent).with_context(|| format!("make a folder in {}", parent.display()))?;
    let payload = unpayload::Payload::open_url(&stock.package).with_context(|| format!("open {}", stock.package))?;
    let options = unpayload::ExtractOptions { output_dir: work.path().to_path_buf(), partitions: vec![RECOVERY.into()], ..Default::default() };
    let report = |event: unpayload::Event<'_>| match event {
        unpayload::Event::Started { partitions } => {
            for (name, bytes) in partitions {
                println!("taking {name} out of the package: {bytes} bytes");
            }
        }
        unpayload::Event::Extracted(image) => println!("took {} out: {} bytes", image.partition, image.size),
        unpayload::Event::Progress { .. } => {}
    };
    let taken = payload.extract(&options, &report).with_context(|| format!("take {RECOVERY} out of {}", stock.package))?;
    let image = taken.iter().find(|i| i.partition == RECOVERY).with_context(|| format!("{} holds no {RECOVERY} partition", stock.package))?;
    let sum: String = image.sha256.as_deref().context("the package's own sum of the image was not checked")?.iter().map(|b| format!("{b:02x}")).collect();
    ensure!(sum == stock.recovery, "the {RECOVERY} partition in {} has the SHA-256 {sum}, where the configuration names {}", stock.package, stock.recovery);
    std::fs::rename(&image.path, out).with_context(|| format!("move {} to {}", image.path.display(), out.display()))?;
    println!("wrote {} ({} bytes), its SHA-256 the one the configuration names", out.display(), image.size);
    Ok(())
}

/// The size of the partition `stock` fills: all of it, as the phone and a
/// full update package hold it, which its AVB footer ends.
fn partition_size(stock: &Path) -> Result<usize> {
    let mut file = File::open(stock).with_context(|| format!("open {}", stock.display()))?;
    let size = file.metadata().with_context(|| format!("stat {}", stock.display()))?.len();
    let mut magic = [0u8; 4];
    if size >= 64 {
        file.seek(SeekFrom::Start(size - 64)).with_context(|| format!("seek in {}", stock.display()))?;
        file.read_exact(&mut magic).with_context(|| format!("read {}", stock.display()))?;
    }
    ensure!(
        &magic == b"AVBf",
        "{} does not end in an AVB footer; give the whole recovery partition, as the phone or a full update package holds it",
        stock.display()
    );
    Ok(usize::try_from(size)?)
}

/// What a recovery image says about the build it is: the version and patch
/// levels of its own ramdisk's system and vendor partition, which are older
/// than those installed beside it whenever the phone has taken an update.
/// The phone's secure services read these to tell the secure world which
/// build is running, and keys the installed system made are refused when
/// they say the wrong one, so they are left out here for the recovery to
/// fill in from the installed system once it has that mounted.
const BUILD_VERSION: [&str; 3] = ["ro.build.version.release", "ro.build.version.security_patch", "ro.vendor.build.security_patch"];

/// Where the recovery the ramdisk came with is kept, beside Skyward's.
const STOCK_RECOVERY: &str = "system/bin/recovery.stock";

/// The ramdisk's own init rules, in the places a recovery keeps them: at
/// the root, as Android's recovery ramdisk does, or under the system.
const INIT_RC: [&str; 2] = ["init.rc", "system/etc/init/hw/init.rc"];

/// The properties that decide whether adbd runs as root and whether a
/// computer may attach without being authorized. A stock property file may
/// set them before the ones Skyward writes, and init keeps the first it
/// reads for a name, so they are taken out first.
const ADBD: [&str; 3] = ["ro.secure", "ro.adb.secure", "ro.debuggable"];

/// The scheduling profiles a recovery's programs look up by name. A
/// recovery's cgroup setup mounts no controllers, so of all a system's
/// profiles do, only the timer slack applies here.
const TASK_PROFILES: &[u8] = include_bytes!("task_profiles.json");

/// The init rules that give the USB port file transfer beside adb, which a
/// stock recovery does not offer.
const MTP_RC: &[u8] = include_bytes!("mtp.rc");

/// Declare the recovery service as one init starts once, in whichever of
/// the ramdisk's own init rules declares it. A stock one declares it
/// without `oneshot`, so init starts it again as soon as it ends.
fn once(rd: &mut Ramdisk) -> Result<()> {
    for path in INIT_RC {
        let Some(entry) = rd.find(path) else { continue };
        let text = String::from_utf8_lossy(&entry.data);
        let mut out = String::with_capacity(text.len() + 16);
        let mut ours = false;
        for line in text.lines() {
            out.push_str(line);
            out.push('\n');
            let t = line.trim_start();
            if t.starts_with("service ") {
                ours = t.split_whitespace().nth(1) == Some("recovery");
                continue;
            }
            if ours && t.starts_with("oneshot") {
                ours = false;
            } else if ours && !t.is_empty() && !t.starts_with('#') {
                out.push_str("    oneshot\n");
                ours = false;
            }
        }
        rd.replace(path, out.into_bytes())?;
        return Ok(());
    }
    Ok(())
}

/// A property file without the lines that set any of `names`.
fn without(props: &[u8], names: &[&str]) -> Vec<u8> {
    let mut out = Vec::with_capacity(props.len());
    for line in props.split_inclusive(|b| *b == b'\n') {
        let key = line.split(|b| *b == b'=').next().unwrap_or_default();
        if !names.iter().any(|n| n.as_bytes() == key) {
            out.extend_from_slice(line);
        }
    }
    out
}

/// Write `bytes`, the image `img` makes, to `out`.
fn write(out: &Path, img: &BootImage, bytes: &[u8]) -> Result<()> {
    std::fs::write(out, bytes).with_context(|| format!("write {}", out.display()))?;
    println!(
        "wrote {} ({} bytes kernel, {} bytes ramdisk, {} bytes in all)",
        out.display(),
        img.kernel.len(),
        img.ramdisk.len(),
        bytes.len()
    );
    Ok(())
}

fn list(image: &Path) -> Result<()> {
    let img = BootImage::read(image)?;
    let h = &img.header;
    println!(
        "header v{}  kernel {} bytes  ramdisk {} bytes  {:?}",
        h.header_version(),
        img.kernel.len(),
        img.ramdisk.len(),
        h.osversionpatch
    );
    if img.ramdisk.is_empty() {
        return Ok(());
    }
    let rd = Ramdisk::decode(&img.ramdisk)?;
    for e in &rd.entries {
        println!("{:06o} {:>5}:{:<5} {:>10}  {}", e.mode, e.uid, e.gid, e.data.len(), e.name);
    }
    println!("{} entries", rd.entries.len());
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_lines_that_set_a_name_are_the_only_ones_left_out() {
        let props = b"ro.build.id=TQ\nro.build.version.release=13\nro.build.version.release_or_codename=13\nro.product.name=ishtar\n\
ro.vendor.build.security_patch=2026-10-01\nro.vendor.build.security_patch_level=1\n";
        let out = without(props, &BUILD_VERSION);
        let text = String::from_utf8(out).expect("property files are text");
        assert!(!text.contains("ro.build.version.release=13"));
        assert!(!text.contains("ro.vendor.build.security_patch=2026-10-01"), "the vendor partition's patch level is left out too");
        assert!(text.contains("ro.build.version.release_or_codename=13"), "a longer name is left alone");
        assert!(text.contains("ro.vendor.build.security_patch_level=1"), "a longer name is left alone");
        assert!(text.contains("ro.build.id=TQ") && text.contains("ro.product.name=ishtar"));
    }

    #[test]
    fn the_profiles_name_all_the_recovery_asks_for() {
        let profiles: serde_json::Value = serde_json::from_slice(TASK_PROFILES).expect("the profiles are JSON");
        let names = |key: &str| -> Vec<String> {
            profiles[key].as_array().expect("a list").iter().map(|p| p["Name"].as_str().expect("a name").to_string()).collect()
        };
        let (plain, aggregate) = (names("Profiles"), names("AggregateProfiles"));
        // What libprocessgroup asks for when a program sets its scheduling
        // policy, and what the recovery's own init scripts give logd.
        let asked = [
            "SCHED_SP_DEFAULT",
            "SCHED_SP_BACKGROUND",
            "SCHED_SP_FOREGROUND",
            "SCHED_SP_TOP_APP",
            "SCHED_SP_SYSTEM",
            "SCHED_SP_RT_APP",
            "CPUSET_SP_DEFAULT",
            "CPUSET_SP_BACKGROUND",
            "CPUSET_SP_FOREGROUND",
            "CPUSET_SP_TOP_APP",
            "CPUSET_SP_SYSTEM",
            "CPUSET_SP_RESTRICTED",
            "BlkIOBackground",
            "BlkIOForeground",
            "ServiceCapacityLow",
        ];
        for name in asked {
            assert!(plain.iter().chain(&aggregate).any(|n| n == name), "{name} is missing");
        }
        for a in profiles["AggregateProfiles"].as_array().expect("a list") {
            for part in a["Profiles"].as_array().expect("a list") {
                let part = part.as_str().expect("a name");
                assert!(plain.iter().any(|n| n == part), "{} is made of {part}, which is not a profile", a["Name"]);
            }
        }
    }

    #[test]
    fn the_partition_is_as_large_as_a_stock_image_ending_in_its_footer() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("recovery.img");
        let mut whole = vec![0u8; 8192];
        whole[8192 - 64..8192 - 60].copy_from_slice(b"AVBf");
        std::fs::write(&path, &whole).unwrap();
        assert_eq!(partition_size(&path).unwrap(), 8192);
        std::fs::write(&path, &whole[..8000]).unwrap();
        assert!(partition_size(&path).is_err(), "an image cut short of its partition's end");
        std::fs::write(&path, b"AVBf").unwrap();
        assert!(partition_size(&path).is_err(), "too short to end in a footer");
    }

    #[test]
    fn a_file_without_a_last_newline_keeps_its_last_line() {
        let out = without(b"a=1\nro.build.version.release=13", &BUILD_VERSION);
        assert_eq!(out, b"a=1\n");
        let kept = without(b"a=1\nb=2", &BUILD_VERSION);
        assert_eq!(kept, b"a=1\nb=2");
    }
}

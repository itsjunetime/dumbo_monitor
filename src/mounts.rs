use core::{fmt::Display, ops::ControlFlow, str::FromStr};
use std::{
    borrow::Cow,
    collections::HashMap,
    io::{self, ErrorKind, Read},
    path::Path,
};

use ntfy::Priority;
use rand::prelude::SliceRandom;
use sys_mount::{FilesystemType, Mount, MountFlags};

use crate::Alerter;

#[derive(Debug, PartialEq, Clone)]
pub enum MountDevice {
    Path(Box<Path>),
    Other(Box<str>),
}

impl AsRef<Path> for MountDevice {
    fn as_ref(&self) -> &Path {
        match self {
            Self::Path(p) => p,
            Self::Other(s) => <str as AsRef<Path>>::as_ref(s),
        }
    }
}

impl Display for MountDevice {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::Path(p) => write!(f, "{}", p.display()),
            Self::Other(s) => write!(f, "{s}"),
        }
    }
}

#[derive(Debug, PartialEq, Clone)]
pub enum FsType {
    Btrfs,
    Zfs,
    Ext2,
    Ext3,
    Ext4,
    Tmpfs,
    DevTmpfs,
    DevPts,
    Sysfs,
    Securityfs,
    CGroup2,
    PStore,
    EfiVarfs,
    Bpf,
    Configfs,
    Proc,
    Selinuxfs,
    Autofs,
    Mqueue,
    Debugfs,
    HugeTlbfs,
    Tracefs,
    Fusectl,
    Vfat,
    BinfmtMisc,
    RpcPipefs,
    Fuse(FuseFs),
    Other(Box<str>),
}

impl Display for FsType {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::Btrfs => write!(f, "btrfs"),
            Self::Zfs => write!(f, "zfs"),
            Self::Ext2 => write!(f, "ext2"),
            Self::Ext3 => write!(f, "ext3"),
            Self::Ext4 => write!(f, "ext4"),
            Self::Tmpfs => write!(f, "tmpfs"),
            Self::DevTmpfs => write!(f, "devtmpfs"),
            Self::DevPts => write!(f, "devpts"),
            Self::Sysfs => write!(f, "sysfs"),
            Self::Securityfs => write!(f, "securityfs"),
            Self::CGroup2 => write!(f, "cgroup2"),
            Self::PStore => write!(f, "pstore"),
            Self::EfiVarfs => write!(f, "efivarfs"),
            Self::Bpf => write!(f, "bpf"),
            Self::Configfs => write!(f, "configfs"),
            Self::Proc => write!(f, "proc"),
            Self::Selinuxfs => write!(f, "selinuxfs"),
            Self::Autofs => write!(f, "autofs"),
            Self::Mqueue => write!(f, "mqueue"),
            Self::Debugfs => write!(f, "debugfs"),
            Self::HugeTlbfs => write!(f, "hugetlbfs"),
            Self::Tracefs => write!(f, "tracefs"),
            Self::Fusectl => write!(f, "fusectl"),
            Self::Vfat => write!(f, "vfat"),
            Self::BinfmtMisc => write!(f, "binfmt_misc"),
            Self::RpcPipefs => write!(f, "rpc_pipefs"),
            Self::Fuse(fuse) => write!(f, "fuse.{fuse}"),
            Self::Other(o) => write!(f, "{o}"),
        }
    }
}

impl<'a> From<Cow<'a, str>> for FsType {
    fn from(value: Cow<'a, str>) -> Self {
        match value.as_ref() {
            "btrfs" => Self::Btrfs,
            "zfs" => Self::Zfs,
            "ext2" => Self::Ext2,
            "ext3" => Self::Ext3,
            "ext4" => Self::Ext4,
            "tmpfs" => Self::Tmpfs,
            "devtmpfs" => Self::DevTmpfs,
            "devpts" => Self::DevPts,
            "sysfs" => Self::Sysfs,
            "securityfs" => Self::Securityfs,
            "cgroup2" => Self::CGroup2,
            "pstore" => Self::PStore,
            "efivarfs" => Self::EfiVarfs,
            "bpf" => Self::Bpf,
            "configfs" => Self::Configfs,
            "proc" => Self::Proc,
            "selinuxfs" => Self::Selinuxfs,
            "autofs" => Self::Autofs,
            "fusectl" => Self::Fusectl,
            "mqueue" => Self::Mqueue,
            "debugfs" => Self::Debugfs,
            "hugetlbfs" => Self::HugeTlbfs,
            "tracefs" => Self::Tracefs,
            "vfat" => Self::Vfat,
            "binfmt_misc" => Self::BinfmtMisc,
            "rpc_pipefs" => Self::RpcPipefs,
            other => {
                const FUSE_PREFIX: &str = "fuse.";
                if other.starts_with(FUSE_PREFIX) {
                    Self::Fuse(FuseFs::from(other.split_at(FUSE_PREFIX.len()).1))
                } else {
                    Self::Other(Box::from(value))
                }
            }
        }
    }
}

#[derive(Debug, PartialEq, Clone)]
pub enum FuseFs {
    GvfsdFuse,
    Portal,
    Other(Box<str>),
}

impl Display for FuseFs {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::GvfsdFuse => write!(f, "gvfsd-fuse"),
            Self::Portal => write!(f, "portal"),
            Self::Other(o) => write!(f, "{o}"),
        }
    }
}

impl From<&str> for FuseFs {
    fn from(s: &str) -> Self {
        match s {
            "gvfsd-fuse" => Self::GvfsdFuse,
            "portal" => Self::Portal,
            _ => Self::Other(Box::from(s)),
        }
    }
}

#[derive(Debug, PartialEq, Clone)]
pub enum MountOption {
    Kv { key: Box<str>, value: Box<str> },
    Simple(Box<str>),
}

// TODO: Add option to just pull options from fstab instead of specifying them here
pub struct MountPoint {
    pub device: MountDevice,
    pub mount_point: Box<Path>,
    pub fs_type: FsType,
    pub options: MountOptions,
}

pub enum MountParseMissing {
    Device,
    MountPoint,
    FsType,
    Options,
}

impl FromStr for MountPoint {
    type Err = MountParseMissing;
    fn from_str(s: &str) -> Result<Self, Self::Err> {
        let mut words = s.split_ascii_whitespace();

        let device = words.next().ok_or(Self::Err::Device)?;
        let device = if device.starts_with("/") {
            MountDevice::Path(Box::from(Path::new(device)))
        } else {
            MountDevice::Other(Box::from(device))
        };

        let mount_point = Box::from(Path::new(words.next().ok_or(Self::Err::MountPoint)?));
        let fs_type = FsType::from(Cow::Borrowed(words.next().ok_or(Self::Err::FsType)?));

        let options = words.next().ok_or(Self::Err::Options)?;
        let options = options
            .split(',')
            .map(|opt| {
                if let Some((key, value)) = opt.split_once('=') {
                    MountOption::Kv {
                        key: Box::from(key),
                        value: Box::from(value),
                    }
                } else {
                    MountOption::Simple(Box::from(opt))
                }
            })
            .collect::<Vec<_>>();

        Ok(Self {
            device,
            mount_point,
            fs_type,
            options: MountOptions { inner: options },
        })
    }
}

struct AllMounts {
    mounted: HashMap<Box<Path>, MountPoint>,
    warnings: Vec<(MountParseMissing, Box<str>)>,
}

fn get_all_mounts() -> Result<AllMounts, io::Error> {
    let mounts = std::fs::read_to_string("/proc/mounts")?;

    let mut warnings = Vec::new();
    let mut mounted = HashMap::new();

    for line in mounts.lines() {
        match MountPoint::from_str(line) {
            Err(e) => warnings.push((e, Box::from(line))),
            Ok(p) => _ = mounted.insert(p.mount_point.clone(), p),
        }
    }

    Ok(AllMounts { mounted, warnings })
}

#[derive(PartialEq, Clone)]
pub struct MountOptions {
    pub inner: Vec<MountOption>,
}

impl Display for MountOptions {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        for (idx, opt) in self.inner.iter().enumerate() {
            match opt {
                MountOption::Simple(v) => write!(f, "{v}")?,
                MountOption::Kv { key, value } => write!(f, "{key}={value}")?,
            }

            if idx != self.inner.len() - 1 {
                write!(f, ",")?;
            }
        }

        Ok(())
    }
}

enum FailureReason {
    NotMounted,
    WrongDeviceOnPoint(MountDevice),
    WrongFsType(FsType),
    WrongOptions(MountOptions),
}

impl Display for FailureReason {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::NotMounted => write!(f, "the device is not mounted"),
            Self::WrongDeviceOnPoint(dev) => {
                write!(f, "the wrong device ({dev}) was found mounted")
            }
            Self::WrongFsType(fs) => {
                write!(f, "the wrong filesystem type ({fs}) was found mounted")
            }
            Self::WrongOptions(opts) => write!(
                f,
                "the wrong options ({opts}) were used to mout the filesystem"
            ),
        }
    }
}

pub fn check_mountpoints(watched: &[MountPoint], ntfy: &mut impl Alerter) {
    let mounts = match get_all_mounts() {
        Ok(m) => m,
        Err(e) => {
            eprintln!("[ERROR] Couldn't get all mount points by reading /proc/mounts: {e}");
            return;
        }
    };

    for warning in &mounts.warnings {
        println!("[ERROR] /proc/mounts line couldn't be parsed: '{}'", warning.1);
    }

    for watched in watched {
        let mut fail = |reason: FailureReason| {
            fail_mountpoint(watched, reason, ntfy);
        };

        let Some(mounted) = mounts.mounted.get(&*watched.mount_point) else {
            fail(FailureReason::NotMounted);
            continue;
        };

        if mounted.device != watched.device {
            fail(FailureReason::WrongDeviceOnPoint(mounted.device.clone()));
            continue;
        }

        if mounted.fs_type != watched.fs_type {
            fail(FailureReason::WrongFsType(mounted.fs_type.clone()));
            continue;
        }

        if mounted.options != watched.options {
            fail(FailureReason::WrongOptions(mounted.options.clone()));
            continue;
        }
    }
}

fn fail_mountpoint(mp: &MountPoint, reason: FailureReason, ntfy: &mut impl Alerter) {
    println!(
        "[FAILURE] Mount point {} failed: {reason}",
        mp.mount_point.display()
    );

    let point_display = mp.mount_point.display();
    match &reason {
        FailureReason::NotMounted => ntfy.send_msg(
            "Filesystem not mounted",
            format!("Nothing mounted to {point_display} (expected {}); attempting mount", mp.device),
            ["error"],
            Priority::High
        ),
        FailureReason::WrongDeviceOnPoint(wrong_device) => ntfy.send_msg(
            "Wrong device mounted",
            format!("Wrong device mounted to {point_display}: expected {}, found {}", mp.device, wrong_device),
            ["error"],
            Priority::High
        ),
        FailureReason::WrongFsType(wrong_type) => ntfy.send_msg(
            "Wrong FS type mounted",
            format!(
                "Correct device ({}) but wrong FS type mounted to {point_display}: expected {}, found {wrong_type}",
                mp.device,
                mp.fs_type,
            ),
            ["error"],
            Priority::High
        ),
        FailureReason::WrongOptions(wrong_opts) => ntfy.send_msg(
            "FS Mounted with Wrong Options",
            format!(
                "Correct device ({}) mounted to {point_display}, but with wrong options: expected {}, found {wrong_opts}. Attempting remount.",
                mp.device,
                mp.options
            ),
            ["error"],
            Priority::High
        ),
    }

    if matches!(
        reason,
        FailureReason::NotMounted | FailureReason::WrongOptions(_)
    ) {
        let options = mp.options.to_string();
        let fs_type = mp.fs_type.to_string();
        println!(
            "[INFO] Attempting mount for {point_display} with device {}, fstype {fs_type}, options {options}",
            mp.device,
        );

        let mut builder = Mount::builder().data(&options);
        if let FailureReason::WrongOptions(_) = reason {
            builder = builder.flags(MountFlags::REMOUNT);
        }

        let res = builder
            .fstype(FilesystemType::Manual(&fs_type))
            .mount(&mp.device, &mp.mount_point);

        if let Err(e) = res {
            println!("[ERROR] Couldn't mount {point_display} again: {e}");

            ntfy.send_msg(
                "Remounting failed",
                format!("Failed to remount {} to {point_display}: {e}", mp.device),
                ["error"],
                Priority::High,
            );
        }
    }
}

pub fn check_file_within_mountpoint(mp: &Path, ntfy: &mut impl Alerter) {
    enum FoundReadableFile {
        Yes,
        No,
    }

    fn recurse_into_dir(
        found_file: &mut bool,
        allowable_failures_left: &mut usize,
        last_failed_file: &mut Option<(Box<Path>, std::io::Error)>,
        dir: &Path,
        mount_point: &Path,
        ntfy: &mut impl Alerter,
    ) -> ControlFlow<FoundReadableFile> {
        let i = match std::fs::read_dir(dir) {
            Ok(i) => i,
            Err(e) => {
                println!(
                    "[WARNING] Can't read contents of directory ({}) even though permissions implied we could: {e}",
                    dir.display()
                );
                return ControlFlow::Continue(());
            }
        };

        let mut valid_metas = i
            .into_iter()
            .flat_map(|item| {
                item.and_then(|entry| entry.metadata().map(|meta| (entry, meta)))
                    .inspect_err(|e| {
                        // I'm just warning here 'cause I don't expect it to ever happen - if we can read
                        // that there's a list of directories, I expect that we should be able to get the
                        // name of each one.
                        println!("[WARNING] Couldn't stat item within {}: {e}", dir.display());
                    })
                    .ok()
            })
            .collect::<Vec<_>>();

        let mut rng = rand::rng();

        valid_metas.shuffle(&mut rng);

        for (entry, metadata) in valid_metas {
            // None of that - no infinite loops for stack overflow
            if metadata.is_symlink() {
                continue;
            }

            let path = entry.path();

            // If it's a directory, go into it and look at everything there.
            if metadata.is_dir() {
                let flow = recurse_into_dir(
                    found_file,
                    allowable_failures_left,
                    last_failed_file,
                    &path,
                    mount_point,
                    ntfy,
                );

                // if they've returned `Break`, we found our answer. Otherwise, just move onto the
                // next thing.
                match flow {
                    ControlFlow::Break(found) => return ControlFlow::Break(found),
                    ControlFlow::Continue(()) => continue,
                }
            }

            // We need to be able to try to read it; if its size is 0 then we can't read anything
            // from it.
            if metadata.len() == 0 {
                continue;
            }

            *found_file = true;
            let mut first_byte = [0u8; 1];

            // Just try to open it and then read one byte.
            let res = std::fs::File::open(&path).and_then(|mut file| file.read(&mut first_byte));

            // If we can't open it or read a single byte, then we check why...
            match res {
                Err(err) => match err.kind() {
                    // If we failed bc of permissions, that's permissible. Just how it is sometimes.
                    // go onto the next thing.
                    ErrorKind::PermissionDenied => continue,
                    // for any other reason (or at least, for any reason that I've been able to find
                    // within the `ErrorKind` variant), we treat it as failed
                    _ => {
                        // decrease the amount of our allowable failures
                        *allowable_failures_left = allowable_failures_left.saturating_sub(1);
                        // and if we've hit the end, tell ntfy to send the notification and exit
                        // (since we have our answer)
                        if *allowable_failures_left == 0 {
                            alert_cant_read_mountpoint(
                                ReadFileFailReason::FailedFinalTry { path: &path, err },
                                mount_point,
                                ntfy,
                            );
                            return ControlFlow::Break(FoundReadableFile::No);
                        } else {
                            // If we haven't hit the end, log and record it, and then break out of
                            // this directory (to get more variety in the types of files we try to
                            // check).
                            println!(
                                "[ERROR] Failed to read file {} on mountpoint {} ({allowable_failures_left} allowed left): {err}",
                                path.display(),
                                mount_point.display()
                            );
                            *last_failed_file = Some((path.into(), err));
                            break;
                        }
                    }
                },
                // If we read it all fine, then we've found that there are still files on this mount
                // that are readable! Good to go, return all good.
                Ok(_) => {
                    println!("[INFO] Successfully read {}, exiting check", path.display());
                    return ControlFlow::Break(FoundReadableFile::Yes);
                }
            }
        }

        ControlFlow::Continue(())
    }

    let mut found_file = false;
    let mut last_failed_file = None;
    let mut allowable_failures = 3;

    let flow = recurse_into_dir(
        &mut found_file,
        &mut allowable_failures,
        &mut last_failed_file,
        mp,
        mp,
        ntfy,
    );

    // If we still are allowed some failures left (meaning that we never reported a failed file
    // read), and at lesat one file did unexpectedly fail to read, and we never found a single
    // readable file, then report it.
    if allowable_failures > 0
        && let Some((path, err)) = last_failed_file
        && let ControlFlow::Break(FoundReadableFile::No) = flow
    {
        alert_cant_read_mountpoint(
            ReadFileFailReason::FailedFinalTry { path: &path, err },
            mp,
            ntfy,
        );
    }

    if !found_file {
        alert_cant_read_mountpoint(ReadFileFailReason::FoundNoFiles, mp, ntfy);
    }
}

#[derive(thiserror::Error, Debug)]
enum ReadFileFailReason<'a> {
    #[error("A sufficient amount of files failed to read with inexplicable errors; last one was {} with {err}", path.display())]
    FailedFinalTry { path: &'a Path, err: std::io::Error },
    #[error("Unable to find any files on the mountpoint")]
    FoundNoFiles,
}

fn alert_cant_read_mountpoint(
    failure_reason: ReadFileFailReason<'_>,
    mount_point: &Path,
    ntfy: &mut impl Alerter,
) {
    println!(
        "[ERROR] Filesystem readability check failed on {}: {failure_reason}",
        mount_point.display()
    );

    ntfy.send_msg(
        "Filesystem Read Check Failed",
        format!(
            "Filesystem readability check failed on {}: {failure_reason}",
            mount_point.display()
        ),
        ["fs_check"],
        Priority::High,
    );
}

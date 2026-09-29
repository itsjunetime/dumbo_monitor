use std::{
    borrow::Cow,
    path::{Path, PathBuf},
};

use crate::{
    modules::{ModuleFailActionAfterAlert, ModuleWatch, PciId, StateToEnsure},
    mounts::{FsType, MountDevice, MountOption, MountOptions, MountPoint},
};

#[derive(knus::Decode)]
pub struct FileReprConfig {
    #[knus(child, unwrap(argument))]
    ntfy_url: String,

    #[knus(child, unwrap(argument))]
    ntfy_topic: String,

    #[knus(child, unwrap(argument))]
    passive_mode: Option<bool>,

    #[knus(children(name = "module"))]
    watch_modules: Vec<FileReprModuleWatch>,

    #[knus(children(name = "mount"))]
    watch_mountpoints: Vec<FileReprMountPoint>,

    #[knus(child, unwrap(arguments))]
    ensure_readable_files_within: Vec<String>,
}

#[derive(knus::Decode)]
struct FileReprModuleWatch {
    #[knus(argument)]
    name: String,
    #[knus(child, unwrap(argument))]
    ensure_loaded: bool,
    #[knus(child, unwrap(arguments))]
    ensure_used_by: Vec<u16>,
    #[knus(child, unwrap(child))]
    on_failure: Option<OnFailure>,
}

#[derive(thiserror::Error, Debug)]
pub enum ModuleWatchConversionErr {
    #[error(
        "Module {name} is required to be unloaded, but also in use by a set of pci Ids ({ensure_used_by:0x?}) - these are mutually exclusive states, so please remove the set of ids or change the requirement to loaded."
    )]
    UnloadedButUsedBy {
        name: String,
        ensure_used_by: Vec<u16>,
    },
}

impl TryFrom<FileReprModuleWatch> for ModuleWatch {
    type Error = ModuleWatchConversionErr;

    fn try_from(value: FileReprModuleWatch) -> Result<Self, Self::Error> {
        let FileReprModuleWatch {
            name,
            ensure_loaded,
            ensure_used_by,
            on_failure,
        } = value;
        // todo: allow specification of 'this should be loaded but should be in use by nothing (not
        // there's nothing that we need it to be in use for)'
        let ensure_state = match (ensure_loaded, ensure_used_by.len()) {
            (true, _) => StateToEnsure::Loaded {
                used_by: ensure_used_by.into_iter().map(PciId).collect(),
            },
            (false, 0) => StateToEnsure::Unloaded,
            (false, 1..) => {
                return Err(ModuleWatchConversionErr::UnloadedButUsedBy {
                    name: name.to_string(),
                    ensure_used_by,
                });
            }
        };

        Ok(Self {
            name,
            ensure_state,
            actions_upon_fail: Vec::from(on_failure.unwrap_or_default()),
        })
    }
}

#[derive(knus::Decode, Default)]
struct OnFailure {
    #[knus(argument)]
    try_modload: Option<bool>,
    #[knus(argument)]
    try_pci_rescan: bool,
    #[knus(argument)]
    reboot: Option<bool>,
}

impl From<OnFailure> for Vec<ModuleFailActionAfterAlert> {
    fn from(value: OnFailure) -> Self {
        let OnFailure {
            try_modload,
            try_pci_rescan,
            reboot,
        } = value;

        let try_modload = try_modload.unwrap_or(true);
        let reboot = reboot.unwrap_or(false);

        [
            try_modload.then_some(ModuleFailActionAfterAlert::LoadModule),
            try_pci_rescan.then_some(ModuleFailActionAfterAlert::PciRescan),
            reboot.then_some(ModuleFailActionAfterAlert::Reboot),
        ]
        .into_iter()
        .flatten()
        .collect()
    }
}

#[derive(knus::Decode)]
struct FileReprMountPoint {
    #[knus(argument)]
    device: String,

    #[knus(child, unwrap(argument))]
    mount_point: String,
    #[knus(child, unwrap(argument))]
    fs_type: String,
    #[knus(child, unwrap(argument))]
    options: String,
}

impl From<FileReprMountPoint> for MountPoint {
    fn from(value: FileReprMountPoint) -> Self {
        let FileReprMountPoint {
            device,
            mount_point,
            fs_type,
            options,
        } = value;

        MountPoint {
            device: if device.starts_with('/') {
                MountDevice::Path(PathBuf::from(device).into())
            } else {
                MountDevice::Other(Box::from(device))
            },
            mount_point: PathBuf::from(mount_point).into(),
            fs_type: FsType::from(Cow::Owned(fs_type)),
            options: MountOptions {
                inner: options
                    .split(',')
                    .map(|opt| {
                        let mut eq_split = opt.split('=');

                        // There's always gonna be at least one
                        let first = eq_split.next().unwrap();
                        match eq_split.next() {
                            Some(next) => MountOption::Kv {
                                key: first.into(),
                                value: next.into(),
                            },
                            None => MountOption::Simple(Box::from(first)),
                        }
                    })
                    .collect(),
            },
        }
    }
}

pub struct Config {
    pub ntfy_url: String,
    pub ntfy_topic: String,

    pub passive_mode: bool,

    // todo: Add module defaults section
    pub watch_modules: Vec<crate::modules::ModuleWatch>,

    pub watch_mountpoints: Vec<crate::mounts::MountPoint>,
    pub ensure_readable_files_within: Vec<Box<Path>>,
}

#[derive(thiserror::Error, Debug)]
pub enum FromFileReprConfigErr {
    #[error(transparent)]
    ModuleConversion(ModuleWatchConversionErr),
}

impl TryFrom<FileReprConfig> for Config {
    type Error = FromFileReprConfigErr;
    fn try_from(value: FileReprConfig) -> Result<Self, Self::Error> {
        let FileReprConfig {
            ntfy_url,
            ntfy_topic,
            passive_mode,
            watch_modules,
            watch_mountpoints,
            ensure_readable_files_within,
        } = value;

        Ok(Self {
            ntfy_url,
            ntfy_topic,
            passive_mode: passive_mode.unwrap_or(true),
            watch_modules: watch_modules
                .into_iter()
                .map(TryFrom::try_from)
                .collect::<Result<_, _>>()
                .map_err(FromFileReprConfigErr::ModuleConversion)?,
            watch_mountpoints: watch_mountpoints
                .into_iter()
                .map(From::from)
                .collect::<Vec<_>>(),
            ensure_readable_files_within: ensure_readable_files_within
                .into_iter()
                .map(|p| PathBuf::from(p).into())
                .collect(),
        })
    }
}

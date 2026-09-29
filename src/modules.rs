use core::{
    error::Error,
    fmt::{Debug, Display},
};
use std::{collections::BTreeSet, io};

use iddqd::{IdHashItem, IdHashMap, id_upcast};
use ntfy::Priority;
use pci_info::{PciDeviceEnumerationError, PciInfoError, PciInfoPropertyError};
use yoke::Yoke;

use crate::Alerter;

#[derive(PartialEq, Clone, Copy, Eq, PartialOrd, Ord, Debug)]
pub struct PciId(pub u16);

impl Display for PciId {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "{:0X}", self.0)
    }
}

pub enum StateToEnsure {
    Loaded { used_by: BTreeSet<PciId> },
    Unused,
    Unloaded,
}

#[derive(PartialEq)]
pub enum ModuleFailActionAfterAlert {
    // Just reload the module
    LoadModule,
    // Maybe there's some way to tell the system to like reload a pci connection?
    PciRescan,
    // Reboot the whole PC. Probably shouldn't rly do this.
    Reboot,
}

pub struct ModuleWatch {
    pub name: String,
    pub ensure_state: StateToEnsure,
    pub actions_upon_fail: Vec<ModuleFailActionAfterAlert>,
    // min_fail_time_before_action: Duration // TODO: Enable this
}

#[derive(Copy, Clone, Debug, PartialEq)]
enum ModuleState {
    Live,
    Loading,
    Unloading,
}

#[derive(Debug, PartialEq)]
pub struct Module<'a> {
    name: &'a str,
    size_bytes: usize,
    loaded_instances: usize,
    dependencies: Vec<&'a str>,
    state: ModuleState,
    loaded_offset: usize,
}

impl IdHashItem for Module<'_> {
    type Key<'a>
        = &'a str
    where
        Self: 'a;

    fn key(&self) -> Self::Key<'_> {
        self.name
    }

    id_upcast!();
}

impl<'a> TryFrom<&'a str> for Module<'a> {
    type Error = Warning<'a>;

    fn try_from(s: &'a str) -> Result<Self, Self::Error> {
        let mut words = s.split_ascii_whitespace();

        let name = words.next().ok_or(Self::Error::NoName)?;

        let size_bytes_str = words.next().ok_or(Self::Error::NoSize)?;
        let size_bytes = size_bytes_str
            .parse::<usize>()
            .map_err(|_| Self::Error::SizeNotANumber(size_bytes_str))?;

        let load_count_str = words.next().ok_or(Self::Error::NoLoadCount)?;
        let loaded_instances = load_count_str
            .parse::<usize>()
            .map_err(|_| Self::Error::LoadCountNotANumber(load_count_str))?;

        let dependencies_str = words.next().ok_or(Self::Error::NoDependencies)?;
        let dependencies = if dependencies_str == "-" {
            Vec::new()
        } else {
            dependencies_str
                .split(',')
                .filter(|s| !s.is_empty())
                .collect::<Vec<_>>()
        };

        let state_str = words.next().ok_or(Self::Error::NoState)?;
        let state = match state_str {
            "Live" => ModuleState::Live,
            "Loading" => ModuleState::Loading,
            "Unloading" => ModuleState::Unloading,
            unknown => return Err(Self::Error::UnrecognizedState(unknown)),
        };

        let loaded_offset_str = words.next().ok_or(Self::Error::NoLoadedOffset)?;
        let loaded_offset = loaded_offset_str
            .strip_prefix("0x")
            .and_then(|off| usize::from_str_radix(off, 16).ok())
            .ok_or(Self::Error::LoadedOffsetNotHex(loaded_offset_str))?;

        Ok(Self {
            name,
            size_bytes,
            loaded_instances,
            dependencies,
            state,
            loaded_offset,
        })
    }
}

#[derive(yoke::Yokeable, Debug, PartialEq)]
struct LoadedModules<'a> {
    modules: IdHashMap<Module<'a>>,
    warnings: Vec<Warning<'a>>,
}

impl<'a> From<&'a str> for LoadedModules<'a> {
    fn from(value: &'a str) -> Self {
        let mut modules = IdHashMap::new();
        let mut warnings = Vec::new();

        for line in value.lines() {
            match Module::try_from(line) {
                Err(warning) => warnings.push(warning),
                Ok(module) => modules
                    .insert_unique(module)
                    .expect("/proc/modules gave us a duplicate module listing?"),
            }
        }

        LoadedModules { modules, warnings }
    }
}

#[derive(Debug, PartialEq)]
pub enum Warning<'a> {
    NoName,
    NoSize,
    SizeNotANumber(&'a str),
    NoLoadCount,
    LoadCountNotANumber(&'a str),
    NoDependencies,
    NoState,
    UnrecognizedState(&'a str),
    NoLoadedOffset,
    LoadedOffsetNotHex(&'a str),
}

#[derive(thiserror::Error, Debug)]
pub enum GetModulesError {
    #[error("Couldn't read `/proc/modules`: {0}")]
    ReadProcModulesFailed(io::Error),
}

fn get_all_modules() -> Result<Yoke<LoadedModules<'static>, Box<str>>, GetModulesError> {
    let string_rep =
        std::fs::read_to_string("/proc/modules").map_err(GetModulesError::ReadProcModulesFailed)?;

    Ok(Yoke::attach_to_cart(string_rep.into(), |s| {
        LoadedModules::from(s)
    }))
}

#[derive(thiserror::Error, Debug)]
pub enum CheckModulesError<E: Error> {
    #[error("Couldn't get the list of modules present: {0}")]
    GetModules(GetModulesError),
    #[error("Couldn't get some PCI information: {0}")]
    PciFetch(E),
}

pub trait PciDevice {
    type GetDriverErr: Error + Debug;
    fn id(&self) -> PciId;
    fn get_driver(&self) -> Result<&Option<String>, Self::GetDriverErr>;
}

impl PciDevice for pci_info::PciDevice {
    type GetDriverErr = PciInfoPropertyError;
    fn id(&self) -> PciId {
        PciId(self.device_id())
    }
    fn get_driver(&self) -> Result<&Option<String>, Self::GetDriverErr> {
        self.os_driver().map_err(|e| match e {
            PciInfoPropertyError::Unsupported => PciInfoPropertyError::Unsupported,
            PciInfoPropertyError::Error(e) => PciInfoPropertyError::Error(e.clone()),
        })
    }
}

type DeviceCollection<D, DE, E> = Result<Vec<Result<D, DE>>, E>;

pub trait PciFetcher {
    type Error: Error;
    type InfoError: Error;
    type Device: PciDevice;
    fn fetch() -> DeviceCollection<Self::Device, Self::InfoError, Self::Error>;
}

pub struct SysPciFetcher;

impl PciFetcher for SysPciFetcher {
    type Error = PciInfoError;
    type InfoError = PciDeviceEnumerationError;
    type Device = pci_info::PciDevice;

    fn fetch() -> DeviceCollection<Self::Device, Self::InfoError, Self::Error> {
        pci_info::PciInfo::enumerate_pci().map(|iter| iter.into_iter().collect())
    }
}

pub fn check_modules<F: PciFetcher>(
    watches: &[ModuleWatch],
    do_not_rescan_for: &mut BTreeSet<PciId>,
    passive_mode: bool,
    ntfy: &mut impl Alerter,
) -> Result<(), CheckModulesError<FetchOrGetDriverErr<F>>> {
    if watches.is_empty() {
        return Ok(());
    }

    let all_modules = get_all_modules().map_err(CheckModulesError::GetModules)?;
    let modules = &all_modules.get().modules;

    check_modules_with_system::<F>(watches, modules, do_not_rescan_for, passive_mode, ntfy)
        .map_err(CheckModulesError::PciFetch)
}

#[derive(thiserror::Error)]
pub enum FetchOrGetDriverErr<F: PciFetcher> {
    #[error("Couldn't fetch devices: {0}")]
    Fetch(F::Error),
    #[error("Couldn't get properties of device {:x}: {err}", id.0)]
    GetDriver {
        id: PciId,
        err: <F::Device as PciDevice>::GetDriverErr,
    },
}

impl<F: PciFetcher> Debug for FetchOrGetDriverErr<F> {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        let type_name = std::any::type_name::<F>();
        match self {
            Self::Fetch(e) => write!(f, "FetchOrGetDriverErr<{type_name}>::Fetch({e:?})"),
            Self::GetDriver { id, err } => write!(
                f,
                "FetchOrGetDriverErr::<{type_name}>::GetDriver {{ id: {id:?}, err: {err:?} }}",
            ),
        }
    }
}

pub fn check_modules_with_system<F: PciFetcher>(
    watches: &[ModuleWatch],
    modules: &IdHashMap<Module<'_>>,
    do_not_rescan_for: &mut BTreeSet<PciId>,
    passive_mode: bool,
    ntfy: &mut impl Alerter,
) -> Result<(), FetchOrGetDriverErr<F>> {
    let mut pci_devices = Vec::new();

    for watch in watches {
        let mut fail = |reason: ModuleFailureReason| {
            fail_module(watch, reason, do_not_rescan_for, passive_mode, ntfy);
        };

        match &watch.ensure_state {
            StateToEnsure::Unloaded => {
                if let Some(module) = modules.get(&*watch.name) {
                    fail(ModuleFailureReason::ModuleLoaded(module.state));
                }
            }
            StateToEnsure::Unused => match modules.get(&*watch.name) {
                None => fail(ModuleFailureReason::ModuleUnloaded),
                Some(m) if m.loaded_instances > 0 => fail(ModuleFailureReason::ModuleInUse),
                Some(_) => (),
            },
            StateToEnsure::Loaded { used_by } => {
                let Some(module) = modules.get(&*watch.name) else {
                    fail(ModuleFailureReason::ModuleUnloaded);
                    continue;
                };

                if module.loaded_instances == 0 {
                    fail(ModuleFailureReason::ModuleNotInUse);
                    continue;
                }

                if pci_devices.is_empty() {
                    pci_devices = F::fetch().map_err(FetchOrGetDriverErr::Fetch)?;
                }

                let mut devices_left_to_find = used_by.clone();
                for device in &pci_devices {
                    let device = match device {
                        Ok(d) => d,
                        Err(e) => {
                            println!("[ERROR] Failed to retrieve a pci device: {e}");
                            continue;
                        }
                    };

                    let device_id = device.id();
                    if !used_by.contains(&device_id) {
                        continue;
                    }

                    devices_left_to_find.remove(&device_id);

                    let used_driver =
                        device
                            .get_driver()
                            .map_err(|source| FetchOrGetDriverErr::GetDriver {
                                id: device_id,
                                err: source,
                            })?;

                    if used_driver.as_ref().is_none_or(|d| d != &watch.name) {
                        fail(ModuleFailureReason::DeviceNotUsingModule {
                            device: device_id,
                            using_instead: used_driver.clone(),
                        });
                    }
                }

                if !devices_left_to_find.is_empty() {
                    fail(ModuleFailureReason::DevicesNotFound(devices_left_to_find));
                }
            }
        }
    }

    Ok(())
}

enum ModuleFailureReason {
    ModuleLoaded(ModuleState),
    ModuleUnloaded,
    ModuleInUse,
    ModuleNotInUse,
    DeviceNotUsingModule {
        device: PciId,
        using_instead: Option<String>,
    },
    DevicesNotFound(BTreeSet<PciId>),
}

fn fail_module(
    module: &ModuleWatch,
    reason: ModuleFailureReason,
    do_not_rescan_for: &mut BTreeSet<PciId>,
    passive_mode: bool,
    ntfy: &mut impl Alerter,
) {
    use std::fmt::Write;

    enum OneOrMany {
        One(PciId),
        Many(BTreeSet<PciId>),
    }

    let mut reload_pcie_for = None;

    // TODO: reload/reboot base on fail action
    match reason {
        ModuleFailureReason::ModuleLoaded(state) => ntfy.send_msg(
            "Module in Wrong State",
            format!("Kernel module {} was found loaded in state {:?}, but we expected it to be unloaded",
                module.name,
                state
            ),
            ["module"],
            Priority::High
        ),
        ModuleFailureReason::ModuleUnloaded => ntfy.send_msg(
            "Module Not Loaded",
            format!("Kernel module {} was found unloaded, but we expected it to be loaded", module.name),
            ["module"],
            Priority::High
        ),
        ModuleFailureReason::ModuleInUse => ntfy.send_msg(
            "Module In-Use",
            format!("Kernel module {} was found in-use, but we expected it to be loaded but unused", module.name),
            ["module"],
            Priority::High
        ),
        ModuleFailureReason::ModuleNotInUse => ntfy.send_msg(
            "Module Unused",
            format!("Kernel module {} was found loaded but unused; we expected it to be used", module.name),
            ["module"],
            Priority::High
        ),
        ModuleFailureReason::DeviceNotUsingModule { device, using_instead } => {
            reload_pcie_for = Some(OneOrMany::One(device));

            let mut msg_str = format!("PCI device {device} was found not using expected kernel module {}.", module.name);
            if let Some(instead) = using_instead {
                write!(msg_str, " The device was found to be using {instead} instead").unwrap();
            }

            ntfy.send_msg(
                "PCI Not Using Module",
                msg_str,
                ["module"],
                Priority::High
            );
        },
        ModuleFailureReason::DevicesNotFound(pcis) => {
            let mut pci_str = String::new();
            for id in &pcis {
                if !pci_str.is_empty() {
                    pci_str.push_str(", ");
                }

                write!(pci_str, "{id}").unwrap();
            }

            reload_pcie_for = Some(OneOrMany::Many(pcis));

            ntfy.send_msg(
                "PCI Device Not Found",
                format!("The expected PCI Device (IDs) were not found: {pci_str}"),
                ["module"],
                Priority::High
            );
        }
    }

    if !passive_mode
        && module.actions_upon_fail.contains(&ModuleFailActionAfterAlert::PciRescan)
        && let Some(pcis) = reload_pcie_for
    {
        match pcis {
            OneOrMany::One(i) => _ = do_not_rescan_for.insert(i),
            OneOrMany::Many(b) => do_not_rescan_for.extend(b),
        }

        const RESCAN_PATH: &str = "/sys/bus/pci/rescan";
        match std::fs::write(RESCAN_PATH, b"1\n") {
            Err(e) => println!(
                "[ERROR] Can't force-rescan on pci devices (by writing to {RESCAN_PATH}): {e}"
            ),
            Ok(()) => println!("[INFO] Successfully told system to rescan devices"),
        }
    }
}

#[cfg(test)]
mod tests {
    use core::convert::Infallible;

    use crate::tests::{Alert, DummyAlerter};

    use super::*;

    const TEST_MODULES_STR: &str = "cdc_ncm 81920 0 - Unloading 0xffffa94c21758000
cdc_ether 65536 1 cdc_ncm, Live 0xffffa94c21730000
usbnet 98304 2 cdc_ncm,cdc_ether, Live 0xffffa94c21768000
mii 49152 1 usbnet, Live 0xffffa94c21740000
ipheth 49152 0 - Live 0xffffa94c213e8000
snd_hrtimer 49152 1 - Loading 0xffffa94c21308000
snd_seq 147456 7 snd_seq_dummy, Live 0xffffa94c21460000";

    #[test]
    fn modules_parse_correctly() {
        let loaded = LoadedModules::from(TEST_MODULES_STR);
        let expected = LoadedModules {
            modules: [
                Module {
                    name: "cdc_ncm",
                    size_bytes: 81_920,
                    loaded_instances: 0,
                    dependencies: vec![],
                    state: ModuleState::Unloading,
                    loaded_offset: 0xffffa94c21758000,
                },
                Module {
                    name: "cdc_ether",
                    size_bytes: 65_536,
                    loaded_instances: 1,
                    dependencies: vec!["cdc_ncm"],
                    state: ModuleState::Live,
                    loaded_offset: 0xffffa94c21730000,
                },
                Module {
                    name: "usbnet",
                    size_bytes: 98_304,
                    loaded_instances: 2,
                    dependencies: vec!["cdc_ncm", "cdc_ether"],
                    state: ModuleState::Live,
                    loaded_offset: 0xffffa94c21768000,
                },
                Module {
                    name: "mii",
                    size_bytes: 49_152,
                    loaded_instances: 1,
                    dependencies: vec!["usbnet"],
                    state: ModuleState::Live,
                    loaded_offset: 0xffffa94c21740000,
                },
                Module {
                    name: "ipheth",
                    size_bytes: 49_152,
                    loaded_instances: 0,
                    dependencies: vec![],
                    state: ModuleState::Live,
                    loaded_offset: 0xffffa94c213e8000,
                },
                Module {
                    name: "snd_hrtimer",
                    size_bytes: 49_152,
                    loaded_instances: 1,
                    dependencies: vec![],
                    state: ModuleState::Loading,
                    loaded_offset: 0xffffa94c21308000,
                },
                Module {
                    name: "snd_seq",
                    size_bytes: 147_456,
                    loaded_instances: 7,
                    dependencies: vec!["snd_seq_dummy"],
                    state: ModuleState::Live,
                    loaded_offset: 0xffffa94c21460000,
                },
            ]
            .into_iter()
            .collect(),
            warnings: vec![],
        };

        assert_eq!(loaded, expected);
    }

    struct EmptyPciFetcher;

    impl PciFetcher for EmptyPciFetcher {
        type Error = Infallible;
        type InfoError = Infallible;
        type Device = EmptyPciDevice;
        fn fetch() -> DeviceCollection<Self::Device, Self::InfoError, Self::Error> {
            Ok(vec![])
        }
    }

    struct EmptyPciDevice;

    impl PciDevice for EmptyPciDevice {
        type GetDriverErr = Infallible;
        fn id(&self) -> PciId {
            todo!()
        }
        fn get_driver(&self) -> Result<&Option<String>, Self::GetDriverErr> {
            todo!()
        }
    }

    fn assert_modules<const N: usize>(
        watches: [ModuleWatch; N],
        alerts: &[Alert]
    ) {
        let mut alerter = DummyAlerter::default();
        let watches = watches.into_iter().collect::<Vec<_>>();

        let modules = LoadedModules::from(TEST_MODULES_STR);
        let mut do_not_rescan_for = BTreeSet::default();

        let Ok(()) = check_modules_with_system::<EmptyPciFetcher>(
            &watches,
            &modules.modules,
            &mut do_not_rescan_for,
            false,
            &mut alerter,
        );

        assert_eq!(&alerter.alerts, &alerts);
    }

    #[test]
    fn ensure_module_unloaded() {
        assert_modules(
            [
                ModuleWatch {
                    name: "snd_seq".to_string(),
                    ensure_state: StateToEnsure::Unloaded,
                    actions_upon_fail: vec![],
                },
                ModuleWatch {
                    name: "whatever".to_string(),
                    ensure_state: StateToEnsure::Unloaded,
                    actions_upon_fail: vec![],
                },
            ],
            &[
                Alert {
                    title: "Module in Wrong State".into(),
                    message: "Kernel module snd_seq was found loaded in state Live, but we expected it to be unloaded".into(),
                    tags: vec!["module"],
                    priority: Priority::High
                }
            ]
        );
    }

    #[test]
    fn ensure_module_loaded() {
        assert_modules(
            [
                ModuleWatch {
                    name: "cdc_ether".to_string(),
                    ensure_state: StateToEnsure::Loaded {
                        used_by: BTreeSet::default(),
                    },
                    actions_upon_fail: vec![],
                },
                ModuleWatch {
                    name: "totally_unloaded".to_string(),
                    ensure_state: StateToEnsure::Loaded {
                        used_by: BTreeSet::default(),
                    },
                    actions_upon_fail: vec![],
                },
            ],
            &[
                Alert {
                    title: "Module Not Loaded".into(),
                    message: "Kernel module totally_unloaded was found unloaded, but we expected it to be loaded".into(),
                    tags: vec!["module"],
                    priority: Priority::High
                }
            ]
        );
    }
}

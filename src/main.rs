use core::{error::Error, time::Duration};
use std::{borrow::Cow, collections::BTreeSet};

use clap::Parser;
use ntfy::{Payload, Priority, dispatcher::Blocking};

use crate::{
    config::{Config, FileReprConfig},
    modules::{SysPciFetcher, check_modules},
    mounts::{check_file_within_mountpoint, check_mountpoints},
};

mod config;
mod modules;
mod mounts;

fn main() -> Result<(), Box<dyn Error>> {
    let flags = Flags::parse();

    let config_file_text = fs_err::read_to_string(&flags.config_file)?;
    let file_config = match knus::parse::<FileReprConfig>(flags.config_file, &config_file_text) {
        Ok(c) => c,
        Err(e) => {
            println!("{:?}", miette::Report::new(e));
            std::process::exit(1);
        }
    };

    let config = Config::try_from(file_config)?;

    let mut ntfy = Ntfy {
        inner: ntfy::DispatcherBuilder::new(config.ntfy_url).build_blocking()?,
        topic: config.ntfy_topic,
    };

    let mut do_not_rescan_for = BTreeSet::default();

    loop {
        if let Err(e) = check_modules::<SysPciFetcher>(
            &config.watch_modules,
            &mut do_not_rescan_for,
            config.passive_mode,
            &mut ntfy
        ) {
            println!("[ERROR] Couldn't check modules: {e}");
        };

        check_mountpoints(&config.watch_mountpoints, &mut ntfy);

        for mp in &config.ensure_readable_files_within {
            check_file_within_mountpoint(mp, &mut ntfy);
        }

        // TODO Make sure files are readable within given directories

        std::thread::sleep(Duration::from_secs(10));
    }
}

#[derive(clap::Parser)]
struct Flags {
    #[arg(
        short,
        long,
        default_value = "/etc/dumbo.kdl",
        env = "DUMBO_CONFIG_FILE"
    )]
    config_file: String,
}

trait Alerter {
    fn send_msg(
        &mut self,
        title: impl Into<Cow<'static, str>>,
        message: impl Into<Cow<'static, str>>,
        tags: impl IntoIterator<Item = &'static str>,
        priority: Priority,
    );
}

struct Ntfy {
    inner: ntfy::Dispatcher<Blocking>,
    topic: String,
}

impl Alerter for Ntfy {
    fn send_msg(
        &mut self,
        title: impl Into<Cow<'static, str>>,
        message: impl Into<Cow<'static, str>>,
        tags: impl IntoIterator<Item = &'static str>,
        priority: Priority,
    ) {
        let payload = Payload::new(&self.topic)
            .message(message.into().as_ref())
            .title(title.into().as_ref())
            .tags(tags.into_iter().chain(std::iter::once("dumbo")))
            .priority(priority)
            .markdown(true);

        if let Err(e) = self.inner.send(&payload) {
            println!("[ERROR] Couldn't send a message to ntfy: {e}");
        }
    }
}

#[cfg(test)]
pub mod tests {
    use super::*;

    #[derive(Debug, PartialEq)]
    pub struct Alert {
        pub title: Cow<'static, str>,
        pub message: Cow<'static, str>,
        pub tags: Vec<&'static str>,
        pub priority: Priority,
    }

    #[derive(Default)]
    pub struct DummyAlerter {
        pub alerts: Vec<Alert>,
    }

    impl Alerter for DummyAlerter {
        fn send_msg(
            &mut self,
            title: impl Into<Cow<'static, str>>,
            message: impl Into<Cow<'static, str>>,
            tags: impl IntoIterator<Item = &'static str>,
            priority: Priority,
        ) {
            self.alerts.push(Alert {
                title: title.into(),
                message: message.into(),
                tags: tags.into_iter().collect(),
                priority,
            })
        }
    }
}

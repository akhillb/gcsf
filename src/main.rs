#[macro_use]
extern crate clap;
extern crate config;
extern crate ctrlc;
extern crate failure;
extern crate fuse;
extern crate gcsf;
#[macro_use]
extern crate log;
extern crate itertools;
extern crate pretty_env_logger;
extern crate serde;
extern crate serde_json;
extern crate xdg;

use clap::App;
use failure::{err_msg, Error};
use itertools::Itertools;
use std::ffi::OsStr;
use std::fs;
use std::io::prelude::*;
use std::iter;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::Arc;
use std::thread;
use std::time;

use gcsf::{Config, NullFS, GCSF};

const DEBUG_LOG: &str =
    "hyper::client=error,rustls::client_hs=error,hyper::http=error,hyper::net=error,debug";

const INFO_LOG: &str =
    "hyper::client=error,rustls::client_hs=error,hyper::http=error,hyper::net=error,fuse::session=error,info";

const DEFAULT_CONFIG: &str = "\
### This is the configuration file that GCSF uses.
### It should be placed in $XDG_CONFIG_HOME/gcsf/gcsf.toml, which is usually
### defined as $HOME/.config/gcsf/gcsf.toml

# Show additional logging info?
debug = false

# How long to cache the contents of a file after it has been accessed.
cache_max_seconds = 300

# How how many files to cache.
cache_max_items = 20

# How long to cache the size and capacity of the filesystem. These are the
# values reported by `df`.
cache_statfs_seconds = 10

# How many seconds to wait before checking for remote changes and updating them
# locally.
sync_interval = 10

# Mount options
mount_options = [
    \"fsname=GCSF\",
    \"allow_root\",
    \"big_writes\",
    \"max_write=131072\"
]

# If set to true, Google Drive will provide a code after logging in and
# authorizing GCSF. This code must be copied and pasted into GCSF in order to
# complete the process. Useful for running GCSF on a remote server.
#
# If set to false, Google Drive will attempt to communicate with GCSF directly.
# This is usually faster and more convenient.
authorize_using_code = false\n";

fn mount_gcsf(config: Config, mountpoint: &str) {
    let vals = config.mount_options();
    let mut options = iter::repeat("-o")
        .interleave_shortest(vals.iter().map(String::as_ref))
        .map(OsStr::new)
        .collect::<Vec<_>>();
    options.pop();

    unsafe {
        match fuse::spawn_mount(NullFS {}, &mountpoint, &options) {
            Ok(session) => {
                debug!("Test mount of NullFS successful. Will mount GCSF next.");
                drop(session);
            }
            Err(e) => {
                error!("Could not mount to {}: {}", &mountpoint, e);
                return;
            }
        };
    }

    info!("Creating and populating file system...");
    let fs: GCSF = GCSF::with_config(config);
    info!("File sytem created.");

    unsafe {
        info!("Mounting to {}", &mountpoint);
        match fuse::spawn_mount(fs, &mountpoint, &options) {
            Ok(_session) => {
                info!("Mounted to {}", &mountpoint);

                let running = Arc::new(AtomicBool::new(true));
                let r = running.clone();

                ctrlc::set_handler(move || {
                    info!("Ctrl-C detected");
                    r.store(false, Ordering::SeqCst);
                }).expect("Error setting Ctrl-C handler");

                while running.load(Ordering::SeqCst) {
                    thread::sleep(time::Duration::from_millis(50));
                }
            }
            Err(e) => error!("Could not mount to {}: {}", &mountpoint, e),
        };
    }
}

fn load_conf() -> Result<Config, Error> {
    let xdg_dirs = xdg::BaseDirectories::with_prefix("gcsf").unwrap();
    let config_path = xdg_dirs
        .place_config_file("gcsf.toml")
        .map_err(|_| err_msg("Cannot create configuration directory"))?;

    info!("Config file: {:?}", &config_path);

    if !config_path.exists() {
        let mut config_file = fs::File::create(config_path.clone())
            .map_err(|_| err_msg("Could not create config file"))?;
        config_file.write_all(DEFAULT_CONFIG.as_bytes())?;
    }

    let token_path = xdg_dirs
        .place_config_file("auth_token.json")
        .map_err(|_| err_msg("Cannot create configuration directory"))?;

    let mut settings = config::Config::default();
    settings
        .merge(config::File::with_name(config_path.to_str().unwrap()))
        .expect("Invalid configuration file");

    let mut config = settings.try_into::<Config>()?;
    config.token_path = Some(token_path.to_str().unwrap().to_string());

    Ok(config)
}

fn main() {
    let config = load_conf().expect("Could not load configuration file.");

    pretty_env_logger::formatted_builder()
        .unwrap()
        .parse(if config.debug() { DEBUG_LOG } else { INFO_LOG })
        .init();

    let yaml = load_yaml!("cli.yml");
    let matches = App::from_yaml(yaml).get_matches();

    if let Some(_matches) = matches.subcommand_matches("logout") {
        let filename = config.token_path.as_ref().unwrap();
        match fs::remove_file(filename) {
            Ok(_) => {
                println!("Successfully removed {}", filename);
            }
            Err(e) => {
                println!("Could not remove {}: {}", filename, e);
            }
        };
    }

    if let Some(matches) = matches.subcommand_matches("mount") {
        let mountpoint = matches.value_of("mountpoint").unwrap();
        mount_gcsf(config, mountpoint);
    }
}

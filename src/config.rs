use anyhow::{anyhow, Context, Result};
use std::{
    fs,
    net::{SocketAddr, ToSocketAddrs as _},
    time,
};

use reqwest::Url;
use serde::Deserialize;

use crate::CommandLineConf;

#[derive(Deserialize)]
struct RawConfig {
    description_url: Option<String>,
    period: Option<u64>,
    proxy: Option<String>,
    verbose: Option<u8>,
    iface: Option<String>,
    wait: Option<u64>,
    connect_timeout: Option<u64>,
    proxy_timeout: Option<u64>,
    stream_timeout: Option<u64>,
}

pub struct Config {
    pub description_url: Url,
    pub period: time::Duration,
    pub proxy: Option<SocketAddr>,
    pub broadcast_iface: Option<String>,
    pub verbose: log::LevelFilter,
    pub wait: Option<time::Duration>,
    pub connect_timeout: time::Duration,
    pub proxy_timeout: time::Duration,
    pub stream_timeout: time::Duration,
}

impl TryFrom<CommandLineConf> for Config {
    type Error = anyhow::Error;

    fn try_from(conf: CommandLineConf) -> std::result::Result<Self, Self::Error> {
        get_config(conf)
    }
}

fn get_config(args: CommandLineConf) -> Result<Config> {

    let config_as_file = args
        .config
        .map(|file| fs::read_to_string(file).context("Could not open/read config file."))
        .transpose()?;

    let (
        description_url,
        period,
        proxy,
        broadcast_iface,
        verbose,
        wait,
        connect_timeout,
        proxy_timeout,
        stream_timeout,
    ) = if let Some(config_file) = config_as_file {
        let raw_config: RawConfig =
            toml::from_str(&config_file).context("failed to parse config file.")?;

        let desc_url = raw_config
            .description_url
            .ok_or(anyhow!("Missing description URL"))
            .and_then(|s| Url::parse(&s).context("Bad description URL."))?;

        let period = raw_config.period;

        let proxy: Option<SocketAddr> = raw_config
            .proxy
            .as_deref()
            .map(str::parse)
            .transpose()
            .context("Bad proxy address")?;

        (
            desc_url,
            period,
            proxy,
            raw_config.iface,
            raw_config.verbose,
            raw_config.wait,
            raw_config.connect_timeout,
            raw_config.proxy_timeout,
            raw_config.stream_timeout,
        )
    } else {
        (
            args.description_url
                .ok_or(anyhow!("Missing description URL"))?,
            args.interval,
            args.proxy,
            args.iface,
            Some(args.verbose),
            args.wait,
            args.connect_timeout,
            args.proxy_timeout,
            args.stream_timeout,
        )
    };

    let period = period.or(Some(895)).map(time::Duration::from_secs).unwrap();

    let verbose = verbose.map_or(log::LevelFilter::Warn, |v| match v {
        0 => log::LevelFilter::Warn,
        1 => log::LevelFilter::Info,
        2 => log::LevelFilter::Debug,
        _ => log::LevelFilter::Trace,
    });

    // Default: 30 seconds retry interval when waiting
    let wait = wait.map(time::Duration::from_secs);

    // Default: 2 seconds HTTP connect timeout
    let connect_timeout = connect_timeout
        .map(time::Duration::from_secs)
        .unwrap_or(time::Duration::from_secs(2));

    // Default: 10 seconds TCP proxy connect timeout
    let proxy_timeout = proxy_timeout
        .map(time::Duration::from_secs)
        .unwrap_or(time::Duration::from_secs(10));

    // Default: 300 seconds (5 minutes) TCP stream read/write timeout
    let stream_timeout = stream_timeout
        .map(time::Duration::from_secs)
        .unwrap_or(time::Duration::from_secs(300));

    Ok(Config {
        description_url,
        proxy,
        period,
        broadcast_iface,
        verbose,
        wait,
        connect_timeout,
        proxy_timeout,
        stream_timeout,
    })
}

pub fn sockaddr_from_url(url: &Url) -> Result<SocketAddr> {
    let host = url
        .host()
        .ok_or_else(|| anyhow!("URL has no host: {}", url))?;

    let port: u16 = url
        .port_or_known_default()
        .ok_or_else(|| anyhow!("URL has no port and unknown scheme: {}", url))?;

    let address = format!("{}:{}", host, port);

    let addresses: Vec<SocketAddr> = address
        .to_socket_addrs()
        .with_context(|| format!("Couldn't resolve or build socket address from URL: {}", url))?
        .collect();

    addresses
        .first()
        .copied()
        .ok_or_else(|| anyhow!("No valid socket address resolved for URL: {}", url))
}

use std::env;

use fiducia_routing::{shard_for_customer_region, Region};

fn main() -> Result<(), String> {
    let mut args = env::args().skip(1);
    let mut latitude = env_f64("FIDUCIA_LATITUDE")?;
    let mut longitude = env_f64("FIDUCIA_LONGITUDE")?;
    let mut key = env::var("FIDUCIA_ROUTING_KEY").ok();
    let mut region = env::var("FIDUCIA_REGION")
        .ok()
        .map(|value| {
            Region::parse(&value).ok_or_else(|| format!("unknown region '{value}'; run --list"))
        })
        .transpose()?;
    let mut shard_count = env::var("FIDUCIA_SHARD_COUNT")
        .ok()
        .map(|value| parse_u32("FIDUCIA_SHARD_COUNT", Some(value)))
        .transpose()?
        .unwrap_or(256);
    let mut list = env::var("FIDUCIA_REGION_LIST")
        .is_ok_and(|value| matches!(value.as_str(), "1" | "true" | "yes" | "on"));

    while let Some(arg) = args.next() {
        match arg.as_str() {
            "--help" | "-h" => {
                print_usage();
                return Ok(());
            }
            "--list" => list = true,
            "--lat" => latitude = Some(parse_f64("--lat", args.next())?),
            "--lon" => longitude = Some(parse_f64("--lon", args.next())?),
            "--key" => key = args.next(),
            "--region" => {
                let value = args
                    .next()
                    .ok_or_else(|| "--region needs a value".to_string())?;
                region = Some(
                    Region::parse(&value)
                        .ok_or_else(|| format!("unknown region '{value}'; run --list"))?,
                );
            }
            "--shards" => shard_count = parse_u32("--shards", args.next())?,
            other => return Err(format!("unknown argument '{other}'; run --help")),
        }
    }

    if list {
        for region in Region::ALL {
            println!("{}\tcluster={}", region.code(), region.cluster_name());
        }
        return Ok(());
    }

    let nearest = match (latitude, longitude) {
        (Some(lat), Some(lon)) => Some(Region::nearest_to(lat, lon)),
        (None, None) => None,
        _ => return Err("--lat and --lon must be provided together".to_string()),
    };
    let selected = region.or(nearest);

    match (selected, key) {
        (Some(region), Some(key)) => {
            let shard = shard_for_customer_region(region, &key, shard_count);
            println!(
                "region={}\tcluster={}\tkey={}\tshard={}",
                region.code(),
                region.cluster_name(),
                key,
                shard
            );
        }
        (Some(region), None) => {
            println!(
                "region={}\tcluster={}",
                region.code(),
                region.cluster_name()
            );
        }
        (None, Some(_)) => {
            return Err("--key needs --region or --lat/--lon".to_string());
        }
        (None, None) => print_usage(),
    }

    Ok(())
}

fn parse_f64(name: &str, value: Option<String>) -> Result<f64, String> {
    value
        .ok_or_else(|| format!("{name} needs a value"))?
        .parse()
        .map_err(|_| format!("{name} must be a number"))
}

fn env_f64(name: &str) -> Result<Option<f64>, String> {
    env::var(name)
        .ok()
        .map(|value| parse_f64(name, Some(value)))
        .transpose()
}

fn parse_u32(name: &str, value: Option<String>) -> Result<u32, String> {
    let parsed: u32 = value
        .ok_or_else(|| format!("{name} needs a value"))?
        .parse()
        .map_err(|_| format!("{name} must be a positive integer"))?;
    if parsed == 0 {
        return Err(format!("{name} must be greater than zero"));
    }
    Ok(parsed)
}

fn print_usage() {
    println!(
        "Usage:\n  fiducia-region --list\n  fiducia-region --lat <latitude> --lon <longitude>\n  fiducia-region --region <region> --key <key> [--shards <count>]\n  fiducia-region --lat <latitude> --lon <longitude> --key <key> [--shards <count>]"
    );
}

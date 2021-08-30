extern crate clap;
extern crate flate2;
extern crate hyper;
#[macro_use]
extern crate lazy_static;
extern crate r2d2;
extern crate r2d2_sqlite;
extern crate regex;
extern crate serde;
extern crate serde_json;

use hyper::service::{make_service_fn, service_fn};
use hyper::Server;
use std::process;
use std::sync::{RwLock, Arc};
use std::thread::sleep;
use std::time::Duration;

mod config;
mod errors;
mod service;
mod tiles;
mod utils;



#[tokio::main]
async fn main() {
    pretty_env_logger::init();

    let args = match config::parse(config::get_app().get_matches()) {
        Ok(args) => args,
        Err(err) => {
            println!("{}", err);
            process::exit(1)
        }
    };

    println!("Serving tiles from {}", args.directory.display());
    if args.allowed_hosts.len() > 0 {
        println!("Allowed Hosts {}", args.allowed_hosts[0]);
    }
    let _d = args.directory.clone();
    let _si = args.scan_interval;

    println!("Scan Interval: {}", _si);

    let tilesets = tiles::discover_tilesets(String::new(), args.directory);
    let shared = Arc::new(RwLock::new(service::SharedData{tileset: tilesets.clone() }));

    let addr = ([0, 0, 0, 0], args.port).into();

    let allowed_hosts = args.allowed_hosts;
    let headers = args.headers;

    let _ts = shared.clone();
    let _si = args.scan_interval.clone();
    let _subdomain = args.sub_domain;
    if _si > 0 {
        println!("Folder Scan activated, scanning every {}s", _si);
        tokio::task::spawn( async move {
            let _tst = _ts.clone();
            loop{
                sleep(Duration::from_secs(u64::from(_si)));
                println!("Scanning Directory");
                _tst.write().unwrap().tileset = tiles::discover_tilesets(String::new(),  _d.clone());
            }

        });
    }else{
        println!("Folder Scan deactivated.")
    }


    let disable_preview = args.disable_preview;
    let make_service = make_service_fn(move |_conn| {
        let _s = shared.clone();
        let _subdomain = _subdomain.clone();
        let allowed_hosts = allowed_hosts.clone();
        let headers = headers.clone();
        async move {
            Ok::<_, hyper::Error>(service_fn(move |req| {

                service::get_service(
                    req,
                    allowed_hosts.clone(),
                    headers.clone(),
                    disable_preview,
                    _s.clone(),
                    _subdomain.clone()
                )
            }))
        }
    });

    let server = match Server::try_bind(&addr) {
        Ok(builder) => builder.serve(make_service),
        Err(err) => {
            println!("{}", err);
            process::exit(1);
        }
    };

    println!("Listening on http://{}", addr);

    if let Err(e) = server.await {
        eprintln!("server error: {}", e);
    }
}

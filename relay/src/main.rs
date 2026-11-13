use parsecplus_relay::{serve, DEFAULT_PORT};
use std::{
    net::TcpListener,
    sync::{atomic::AtomicBool, Arc},
    time::Duration,
};

fn usage() -> &'static str {
    "ParsecPlus relay\n\nUsage:\n  parsecplus-relay [--bind 0.0.0.0] [--port 39210]\n\nBoth devices must be able to reach this TCP port. The relay only pairs\npeers and forwards opaque bytes; pairing codes and session media are\nencrypted end to end and are never readable by the relay."
}

fn main() {
    let mut bind = String::from("0.0.0.0");
    let mut port = DEFAULT_PORT;
    let mut args = std::env::args().skip(1);
    while let Some(argument) = args.next() {
        match argument.as_str() {
            "--bind" => {
                bind = args.next().unwrap_or_else(|| {
                    eprintln!("--bind needs an address");
                    std::process::exit(2);
                });
            }
            "--port" => {
                port = args
                    .next()
                    .and_then(|value| value.parse().ok())
                    .unwrap_or_else(|| {
                        eprintln!("--port needs a port number");
                        std::process::exit(2);
                    });
            }
            "-h" | "--help" => {
                println!("{}", usage());
                return;
            }
            other => {
                eprintln!("Unknown option: {other}\n\n{}", usage());
                std::process::exit(2);
            }
        }
    }

    let listener = TcpListener::bind((bind.as_str(), port)).unwrap_or_else(|error| {
        eprintln!("Could not bind {bind}:{port}: {error}");
        std::process::exit(1);
    });
    println!("ParsecPlus relay listening on {bind}:{port}");
    serve(listener, Arc::new(AtomicBool::new(false)));
    std::thread::sleep(Duration::from_millis(1));
}

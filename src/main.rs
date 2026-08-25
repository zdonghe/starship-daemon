mod server;

#[cfg(debug_assertions)]
use std::sync::atomic::Ordering;

use server::Server;
use starship_daemon::cache;
use starship_daemon::daemon::DaemonState;
#[cfg(debug_assertions)]
use starship_daemon::watch::{STATS_ENABLED, drain_stats};

fn main() {
    if std::env::args()
        .skip(1)
        .any(|a| a == "--version" || a == "-V")
    {
        let variant = if cfg!(feature = "fork") {
            "fork"
        } else {
            "stock"
        };
        println!("starship-daemon {} ({variant})", env!("CARGO_PKG_VERSION"));
        return;
    }

    let state = match DaemonState::new(cache::default_config_path()) {
        Ok(s) => s,
        Err(_) => {
            eprintln!("Could not load config");
            std::process::exit(1);
        }
    };

    #[cfg(debug_assertions)]
    if starship_daemon::debug_enabled() {
        start_stats_reporter();
    }

    if std::env::var("STARSHIP_DAEMON_THROTTLE").as_deref() != Ok("1") {
        starship_daemon::ffi::disable_power_throttling();
    }

    let mut server = Server::new(state, starship_daemon::pipe_name());
    server.run();
}

#[cfg(debug_assertions)]
fn start_stats_reporter() {
    STATS_ENABLED.store(true, Ordering::Relaxed);
    std::thread::spawn(|| {
        loop {
            std::thread::sleep(std::time::Duration::from_secs(5));
            eprintln!("[watcher-stats]\n{}", drain_stats());
        }
    });
}

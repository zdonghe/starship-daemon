mod server;

use std::sync::atomic::Ordering;

use server::Server;
use starship_daemon::cache;
use starship_daemon::daemon::DaemonState;
use starship_daemon::watch::{STATS_ENABLED, drain_stats};

fn main() {
    let state = match DaemonState::new(cache::default_config_path()) {
        Ok(s) => s,
        Err(_) => {
            eprintln!("Could not load config");
            std::process::exit(1);
        }
    };

    if std::env::var("STARSHIP_WATCH_STATS").is_ok() {
        STATS_ENABLED.store(true, Ordering::Relaxed);
        std::thread::spawn(|| loop {
            std::thread::sleep(std::time::Duration::from_secs(5));
            eprintln!("[watcher-stats]\n{}", drain_stats());
        });
    }

    let mut server = Server::new(state, starship_daemon::pipe_name());
    server.run();
}

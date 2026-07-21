//! `cswap watch` — redraw the usage table on an interval.
//!
//! Default 300s: the usage endpoint budgets roughly 20-30 requests/hour per
//! token, so with several accounts a 5-minute cadence stays well clear.

use anyhow::Result;
use chrono::Local;
use std::io::{self, Write};
use std::time::Duration;

use crate::cmds::list;

pub fn run(interval: u64) -> Result<()> {
    let interval = interval.max(60); // never hammer the usage endpoint
    loop {
        print!("\x1b[2J\x1b[H"); // clear + home
        println!(
            "cswap watch — {}  (refresh {}s, Ctrl-C to quit)\n",
            Local::now().format("%H:%M:%S"),
            interval
        );
        if let Err(e) = list::print_table(false) {
            println!("error: {e:#}");
        }
        io::stdout().flush()?;
        std::thread::sleep(Duration::from_secs(interval));
    }
}

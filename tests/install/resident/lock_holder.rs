//! A separate process holding a real advisory lock for the CLI journey.
use std::{
    fs::OpenOptions,
    io::{self, BufRead, Write},
    path::PathBuf,
};

fn main() -> io::Result<()> {
    let mut arguments = std::env::args_os();
    arguments.next();
    let path = arguments
        .next()
        .map(PathBuf::from)
        .ok_or_else(|| io::Error::new(io::ErrorKind::InvalidInput, "lock path required"))?;
    let file = OpenOptions::new().read(true).write(true).open(&path)?;
    file.lock()?;
    println!("held");
    io::stdout().flush()?;
    let mut release = String::new();
    io::stdin().lock().read_line(&mut release)?;
    file.unlock()?;
    Ok(())
}

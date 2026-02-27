use anyhow::Result;
use thinkbook_eink::Display;

fn main() -> Result<()> {
    let display = Display::connect()?;
    println!("{}", display.info());
    Ok(())
}

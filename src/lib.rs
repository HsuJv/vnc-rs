mod error;

use anyhow::Result;

fn test() -> Result<()> {
    error::test()?;
    Ok(())
}

#[cfg(test)]
mod tests {
    use anyhow::Context;

    use super::*;

    #[should_panic]
    #[test]
    fn test_error() {
        error::test().context("Error Test in test mode").unwrap();
    }
}

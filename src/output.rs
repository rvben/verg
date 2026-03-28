use std::io::IsTerminal;

pub struct OutputConfig {
    pub json: bool,
}

impl OutputConfig {
    pub fn new(json_flag: bool) -> Self {
        let json = json_flag || !std::io::stdout().is_terminal();
        Self { json }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn json_flag_forces_json() {
        let output = OutputConfig::new(true);
        assert!(output.json);
    }
}

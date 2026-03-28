use std::io::IsTerminal;

pub struct OutputConfig {
    pub json: bool,
    pub color: bool,
}

impl OutputConfig {
    pub fn new(json_flag: bool) -> Self {
        let json = json_flag || !std::io::stdout().is_terminal();
        let color = std::io::stderr().is_terminal();
        Self { json, color }
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

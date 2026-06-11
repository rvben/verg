use std::io::IsTerminal;

#[derive(Debug, Clone, PartialEq, clap::ValueEnum)]
pub enum OutputFormat {
    Auto,
    Text,
    Json,
}

pub struct OutputConfig {
    pub json: bool,
    pub color: bool,
    pub format: OutputFormat,
}

impl OutputConfig {
    pub fn new(format: OutputFormat, json_alias: bool) -> Self {
        let json = match (&format, json_alias) {
            (OutputFormat::Json, _) | (_, true) => true,
            (OutputFormat::Auto, false) => !std::io::stdout().is_terminal(),
            (OutputFormat::Text, false) => false,
        };
        let color = std::io::stderr().is_terminal() && !json;
        Self {
            json,
            color,
            format,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn json_flag_forces_json() {
        let output = OutputConfig::new(OutputFormat::Text, true);
        assert!(output.json);
    }

    #[test]
    fn explicit_json_format_forces_json() {
        let output = OutputConfig::new(OutputFormat::Json, false);
        assert!(output.json);
    }

    #[test]
    fn explicit_text_format_no_json() {
        let output = OutputConfig::new(OutputFormat::Text, false);
        assert!(!output.json);
    }
}

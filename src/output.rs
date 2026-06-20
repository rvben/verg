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
    pub quiet: bool,
}

impl OutputConfig {
    pub fn new(format: OutputFormat, json_alias: bool, quiet: bool) -> Self {
        let json = match (&format, json_alias) {
            (OutputFormat::Json, _) | (_, true) => true,
            (OutputFormat::Auto, false) => !std::io::stdout().is_terminal(),
            (OutputFormat::Text, false) => false,
        };
        let color = std::io::stdout().is_terminal() && !json;
        Self {
            json,
            color,
            format,
            quiet,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn json_flag_forces_json() {
        let output = OutputConfig::new(OutputFormat::Text, true, false);
        assert!(output.json);
    }

    #[test]
    fn explicit_json_format_forces_json() {
        let output = OutputConfig::new(OutputFormat::Json, false, false);
        assert!(output.json);
    }

    #[test]
    fn explicit_text_format_no_json() {
        let output = OutputConfig::new(OutputFormat::Text, false, false);
        assert!(!output.json);
    }
}

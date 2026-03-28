pub mod exit_codes {
    pub const SUCCESS: i32 = 0;
    pub const NOTHING_CHANGED: i32 = 1;
    pub const PARTIAL_FAILURE: i32 = 2;
    pub const TOTAL_FAILURE: i32 = 3;
    pub const CONNECTION_ERROR: i32 = 4;
    pub const INVALID_CONFIG: i32 = 5;
    pub const TARGET_NOT_FOUND: i32 = 6;
    pub const INTERNAL_ERROR: i32 = 7;
}

#[derive(Debug, thiserror::Error)]
pub enum Error {
    #[error("config error: {0}")]
    Config(String),

    #[error("connection error: {0}")]
    Connection(String),

    #[error("target not found: {0}")]
    TargetNotFound(String),

    #[error("resource error: {0}")]
    Resource(String),

    #[error("parse error: {0}")]
    Parse(String),

    #[error("{0}")]
    Io(#[from] std::io::Error),

    #[error("{0}")]
    Other(String),
}

impl Error {
    pub fn exit_code(&self) -> i32 {
        match self {
            Error::Config(_) | Error::Parse(_) => exit_codes::INVALID_CONFIG,
            Error::Connection(_) => exit_codes::CONNECTION_ERROR,
            Error::TargetNotFound(_) => exit_codes::TARGET_NOT_FOUND,
            Error::Resource(_) => exit_codes::PARTIAL_FAILURE,
            Error::Io(_) | Error::Other(_) => exit_codes::INTERNAL_ERROR,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn config_error_exit_code() {
        let err = Error::Config("bad toml".into());
        assert_eq!(err.exit_code(), exit_codes::INVALID_CONFIG);
    }

    #[test]
    fn connection_error_exit_code() {
        let err = Error::Connection("ssh failed".into());
        assert_eq!(err.exit_code(), exit_codes::CONNECTION_ERROR);
    }

    #[test]
    fn target_not_found_exit_code() {
        let err = Error::TargetNotFound("web99".into());
        assert_eq!(err.exit_code(), exit_codes::TARGET_NOT_FOUND);
    }

    #[test]
    fn resource_error_exit_code() {
        let err = Error::Resource("pkg install failed".into());
        assert_eq!(err.exit_code(), exit_codes::PARTIAL_FAILURE);
    }

    #[test]
    fn error_display() {
        let err = Error::Config("missing field".into());
        assert_eq!(err.to_string(), "config error: missing field");
    }
}

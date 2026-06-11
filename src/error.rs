pub mod exit_codes {
    pub const SUCCESS: i32 = 0;
    pub const NOTHING_CHANGED: i32 = 1;
    pub const PARTIAL_FAILURE: i32 = 2;
    pub const TOTAL_FAILURE: i32 = 3;
    pub const CONNECTION_ERROR: i32 = 4;
    pub const INVALID_CONFIG: i32 = 5;
    pub const TARGET_NOT_FOUND: i32 = 6;
    pub const INTERNAL_ERROR: i32 = 7;
    pub const CONFLICT: i32 = 8;
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

    #[error("confirmation required: {0}")]
    ConfirmationRequired(String),

    #[error("conflict: {0}")]
    Conflict(String),

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
            Error::ConfirmationRequired(_) => exit_codes::PARTIAL_FAILURE,
            Error::Conflict(_) => exit_codes::CONFLICT,
            Error::Io(_) | Error::Other(_) => exit_codes::INTERNAL_ERROR,
        }
    }

    pub fn kind_str(&self) -> &'static str {
        match self {
            Error::Config(_) | Error::Parse(_) => "invalid_config",
            Error::Connection(_) => "connection_error",
            Error::TargetNotFound(_) => "not_found",
            Error::Resource(_) => "resource_error",
            Error::ConfirmationRequired(_) => "confirmation_required",
            Error::Conflict(_) => "conflict",
            Error::Io(_) | Error::Other(_) => "internal_error",
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

    #[test]
    fn confirmation_required_exit_code() {
        let err = Error::ConfirmationRequired("pass --yes".into());
        assert_eq!(err.exit_code(), exit_codes::PARTIAL_FAILURE);
        assert_eq!(err.kind_str(), "confirmation_required");
    }

    #[test]
    fn conflict_exit_code() {
        let err = Error::Conflict("state mismatch".into());
        assert_eq!(err.exit_code(), exit_codes::CONFLICT);
        assert_eq!(err.kind_str(), "conflict");
    }

    #[test]
    fn kind_str_covers_all_variants() {
        assert_eq!(Error::Config("".into()).kind_str(), "invalid_config");
        assert_eq!(Error::Parse("".into()).kind_str(), "invalid_config");
        assert_eq!(Error::Connection("".into()).kind_str(), "connection_error");
        assert_eq!(Error::TargetNotFound("".into()).kind_str(), "not_found");
        assert_eq!(Error::Resource("".into()).kind_str(), "resource_error");
        assert_eq!(
            Error::ConfirmationRequired("".into()).kind_str(),
            "confirmation_required"
        );
        assert_eq!(Error::Conflict("".into()).kind_str(), "conflict");
        assert_eq!(Error::Other("".into()).kind_str(), "internal_error");
    }
}

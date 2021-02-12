//! Protocol versions that are declared by WAGI to comply with CGI.

/// This sets the version of CGI that WAGI adheres to.
///
/// At the point at which WAGI diverges from CGI, this value will be replaced with
/// WAGI/1.0
pub const WAGI_VERSION: &str = "CGI/1.1";

/// The CGI-defined "server software version".
pub const SERVER_SOFTWARE_VERSION: &str = "WAGI/1";

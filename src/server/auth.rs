/// Abstraction over authentication logic.
/// Currently: room password (claim + verify).
/// Future: swap to JWT, OIDC, or session tokens — the server handlers
/// call `check_password()` and nothing else about identity.

#[derive(Debug, PartialEq, Eq)]
pub enum AuthResult {
    /// Access granted
    Granted,
    /// Room is new, user must supply a password to claim it
    NeedPassword,
    /// Room exists, user must supply the correct password
    PasswordRequired,
}

/// Check whether a participant is allowed into the room.
///
/// - `stored`: the room's stored password (None = room has no password yet)
/// - `supplied`: password provided by the joining user (for verification)
/// - `claim`: password provided for first-time room creation
/// - `is_new`: whether the room was just created (no participants yet)
pub fn check_password(
    stored: &Option<String>,
    supplied: Option<&str>,
    claim: Option<&str>,
    is_new: bool,
) -> AuthResult {
    if is_new {
        // Room is brand new — require a claim password
        match claim {
            Some(_pw) if !_pw.is_empty() => AuthResult::Granted,
            _ => AuthResult::NeedPassword,
        }
    } else {
        // Room exists — verify supplied password against stored
        match (stored, supplied) {
            (None, _) => AuthResult::Granted, // password-less room (future: always set)
            (Some(stored), Some(supplied)) => {
                if constant_time_eq::constant_time_eq(
                    stored.as_bytes(),
                    supplied.as_bytes(),
                ) {
                    AuthResult::Granted
                } else {
                    AuthResult::PasswordRequired
                }
            }
            (Some(_), None) => AuthResult::PasswordRequired,
        }
    }
}

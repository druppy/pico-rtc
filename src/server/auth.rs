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

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn new_room_requires_claim() {
        assert_eq!(
            check_password(&None, None, None, true),
            AuthResult::NeedPassword
        );
        assert_eq!(
            check_password(&None, None, Some("pw"), true),
            AuthResult::Granted
        );
        // Empty claim password does not grant
        assert_eq!(
            check_password(&None, None, Some(""), true),
            AuthResult::NeedPassword
        );
    }

    #[test]
    fn existing_room_verifies_password() {
        assert_eq!(
            check_password(&Some("pw".into()), Some("pw"), None, false),
            AuthResult::Granted
        );
        assert_eq!(
            check_password(&Some("pw".into()), Some("nope"), None, false),
            AuthResult::PasswordRequired
        );
        assert_eq!(
            check_password(&Some("pw".into()), None, None, false),
            AuthResult::PasswordRequired
        );
        // Claim alone never grants on an existing room
        assert_eq!(
            check_password(&Some("pw".into()), None, Some("other"), false),
            AuthResult::PasswordRequired
        );
    }

    #[test]
    fn passwordless_existing_room_is_open() {
        assert_eq!(
            check_password(&None, None, None, false),
            AuthResult::Granted
        );
    }
}

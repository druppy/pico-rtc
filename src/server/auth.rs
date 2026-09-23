/// Room-password check. Handlers call this and nothing else about identity.
///
/// New rooms are claimed in `handle_join` before this runs, so the only question
/// left is: does the supplied password match what the room already has?
#[derive(Debug, PartialEq, Eq)]
pub enum AuthResult {
    Granted,
    /// Room is locked and the password was missing or wrong.
    PasswordRequired,
}

/// Verify `supplied` against `stored`. A room with no password is open.
pub fn check_password(stored: &Option<String>, supplied: Option<&str>) -> AuthResult {
    match (stored.as_deref(), supplied) {
        (None, _) => AuthResult::Granted,
        (Some(stored), Some(supplied))
            if constant_time_eq::constant_time_eq(stored.as_bytes(), supplied.as_bytes()) =>
        {
            AuthResult::Granted
        }
        _ => AuthResult::PasswordRequired,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn matching_password_grants() {
        assert_eq!(
            check_password(&Some("pw".into()), Some("pw")),
            AuthResult::Granted
        );
    }

    #[test]
    fn wrong_or_missing_password_is_refused() {
        assert_eq!(
            check_password(&Some("pw".into()), Some("nope")),
            AuthResult::PasswordRequired
        );
        assert_eq!(
            check_password(&Some("pw".into()), None),
            AuthResult::PasswordRequired
        );
    }

    #[test]
    fn passwordless_room_is_open() {
        assert_eq!(check_password(&None, None), AuthResult::Granted);
        assert_eq!(check_password(&None, Some("ignored")), AuthResult::Granted);
    }
}

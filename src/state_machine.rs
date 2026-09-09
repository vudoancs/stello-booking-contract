//! Centralized booking state-transition validation.
use crate::errors::Error;
use crate::types::BookingState;

/// Returns `Ok(())` iff `from → to` is an allowed edge.
pub fn validate_transition(from: BookingState, to: BookingState) -> Result<(), Error> {
    let allowed = matches!(
        (from, to),
        (BookingState::Created, BookingState::Escrowed)
            | (BookingState::Created, BookingState::Cancelled)
            | (BookingState::Escrowed, BookingState::Completed)
            | (BookingState::Escrowed, BookingState::Cancelled)
            | (BookingState::Escrowed, BookingState::Disputed)
            | (BookingState::Disputed, BookingState::Completed)
    );

    if allowed {
        Ok(())
    } else {
        Err(Error::InvalidStateTransition)
    }
}

pub fn is_terminal(state: BookingState) -> bool {
    matches!(
        state,
        BookingState::Completed | BookingState::Cancelled
    )
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn valid_edges() {
        assert!(validate_transition(BookingState::Created, BookingState::Escrowed).is_ok());
        assert!(validate_transition(BookingState::Escrowed, BookingState::Completed).is_ok());
        assert!(validate_transition(BookingState::Escrowed, BookingState::Cancelled).is_ok());
        assert!(validate_transition(BookingState::Escrowed, BookingState::Disputed).is_ok());
        assert!(validate_transition(BookingState::Disputed, BookingState::Completed).is_ok());
    }

    #[test]
    fn invalid_edges() {
        assert_eq!(
            validate_transition(BookingState::Created, BookingState::Completed),
            Err(Error::InvalidStateTransition)
        );
        assert_eq!(
            validate_transition(BookingState::Completed, BookingState::Cancelled),
            Err(Error::InvalidStateTransition)
        );
        assert_eq!(
            validate_transition(BookingState::Cancelled, BookingState::Escrowed),
            Err(Error::InvalidStateTransition)
        );
        assert_eq!(
            validate_transition(BookingState::Disputed, BookingState::Cancelled),
            Err(Error::InvalidStateTransition)
        );
    }

    #[test]
    fn terminals() {
        assert!(is_terminal(BookingState::Completed));
        assert!(is_terminal(BookingState::Cancelled));
        assert!(!is_terminal(BookingState::Escrowed));
        assert!(!is_terminal(BookingState::Disputed));
    }
}

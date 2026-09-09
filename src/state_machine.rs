//! Centralized booking state-transition validation.
//!
//! Lifecycle edges:
//! `Created → Escrowed → CheckedIn → Completed`
//! `Escrowed → Cancelled` (traveller cancellation)
use crate::errors::Error;
use crate::types::BookingState;

/// Returns `Ok(())` iff `from → to` is an allowed edge.
pub fn validate_transition(from: BookingState, to: BookingState) -> Result<(), Error> {
    let allowed = matches!(
        (from, to),
        (BookingState::Created, BookingState::Escrowed)
            | (BookingState::Escrowed, BookingState::CheckedIn)
            | (BookingState::CheckedIn, BookingState::Completed)
            | (BookingState::Escrowed, BookingState::Cancelled)
    );

    if allowed {
        Ok(())
    } else {
        Err(Error::InvalidStateTransition)
    }
}

pub fn is_terminal(state: BookingState) -> bool {
    matches!(state, BookingState::Completed | BookingState::Cancelled)
}

#[cfg(test)]
mod tests {
    use super::*;

    const ALL: [BookingState; 6] = [
        BookingState::Created,
        BookingState::Escrowed,
        BookingState::CheckedIn,
        BookingState::Completed,
        BookingState::Cancelled,
        BookingState::Disputed,
    ];

    fn is_valid_edge(from: BookingState, to: BookingState) -> bool {
        matches!(
            (from, to),
            (BookingState::Created, BookingState::Escrowed)
                | (BookingState::Escrowed, BookingState::CheckedIn)
                | (BookingState::CheckedIn, BookingState::Completed)
                | (BookingState::Escrowed, BookingState::Cancelled)
        )
    }

    #[test]
    fn every_valid_transition_accepted() {
        assert!(validate_transition(BookingState::Created, BookingState::Escrowed).is_ok());
        assert!(validate_transition(BookingState::Escrowed, BookingState::CheckedIn).is_ok());
        assert!(validate_transition(BookingState::CheckedIn, BookingState::Completed).is_ok());
        assert!(validate_transition(BookingState::Escrowed, BookingState::Cancelled).is_ok());
    }

    #[test]
    fn every_invalid_transition_rejected() {
        for from in ALL {
            for to in ALL {
                if is_valid_edge(from, to) {
                    continue;
                }
                assert_eq!(
                    validate_transition(from, to),
                    Err(Error::InvalidStateTransition),
                    "expected reject for {:?} → {:?}",
                    from,
                    to
                );
            }
        }
    }

    #[test]
    fn terminals() {
        assert!(is_terminal(BookingState::Completed));
        assert!(is_terminal(BookingState::Cancelled));
        assert!(!is_terminal(BookingState::Created));
        assert!(!is_terminal(BookingState::Escrowed));
        assert!(!is_terminal(BookingState::CheckedIn));
        assert!(!is_terminal(BookingState::Disputed));
    }
}

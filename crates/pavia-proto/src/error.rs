//! The error model (roadmap C1.12): every rejection in spec §5, §6 and §8.10 as one error type, and
//! the outcome each one calls for: end the session with a close code, fail one call or request
//! with a status code, or ignore the frame.

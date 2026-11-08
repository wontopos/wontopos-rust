    use super::status_is_retryable;

    #[test]
    fn a_write_is_never_retried_on_an_ambiguous_status() {
        for s in [408, 502, 503, 504] {
            assert!(!status_is_retryable(s, false), "{s} retried a write");
        }
    }

    #[test]
    fn an_idempotent_read_retries_every_ambiguous_status() {
        for s in [408, 502, 503, 504] {
            assert!(status_is_retryable(s, true), "{s} was not retried on a read");
        }
    }

    #[test]
    fn rate_limiting_retries_whatever_the_method() {
        assert!(status_is_retryable(429, false));
        assert!(status_is_retryable(429, true));
    }

    #[test]
    fn a_plain_failure_is_not_retried() {
        for s in [400, 401, 403, 404, 409, 422, 500, 501] {
            assert!(!status_is_retryable(s, true), "{s} should not retry");
            assert!(!status_is_retryable(s, false), "{s} should not retry");
        }
    }

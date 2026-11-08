    use super::plain_http_to_remote;

    #[test]
    fn userinfo_cannot_impersonate_loopback() {
        for base in [
            "http://127.0.0.1:9@evil.example/x",
            "http://localhost@evil.example",
            "http://evil.example\\@127.0.0.1",
            "http://evil.example?@127.0.0.1",
            "http://evil.example#@127.0.0.1",
        ] {
            assert!(plain_http_to_remote(base), "{base} must warn");
        }
    }

    #[test]
    fn spellings_the_transport_accepts_still_warn() {
        for base in [
            "http://evil.example",
            "HTTP://evil.example",
            " http://evil.example",
            "http:/example.net:8080",
            "http://example.net:8080",
        ] {
            assert!(plain_http_to_remote(base), "{base:?} must warn");
        }
    }

    #[test]
    fn real_loopback_and_https_stay_quiet() {
        for base in [
            "http://127.0.0.1:8080/x",
            "http://localhost:3000",
            "http://LOCALHOST",
            "http://[::1]:8080",
            "http://0.0.0.0:1",
            "http://127.0.0.2:8080",
            "http://127.255.255.254",
            "http://127.1:9",
            "http://[::]:8080",
            "http://[0:0:0:0:0:0:0:1]:9",
            "https://evil.example",
            "not a url",
        ] {
            assert!(!plain_http_to_remote(base), "{base} must not warn");
        }
        for base in ["http://[::ffff:127.0.0.1]:9", "http://128.0.0.1", "http://127.0.0.1.example.com"] {
            assert!(plain_http_to_remote(base), "{base} must warn");
        }
    }

    #[test]
    fn every_loopback_and_unspecified_address_stays_quiet_as_in_python_and_typescript() {
        for base in ["http://127.0.0.2:8080", "http://127.255.255.254", "http://[::]:8080", "http://[0:0:0:0:0:0:0:1]"] {
            assert!(!plain_http_to_remote(base), "{base} must not warn");
        }
        for base in ["http://203.0.113.7","http://[::ffff:127.0.0.1]", "http://[::2]", "http://localhost.example.com", "http://127.0.0.1.example.com"] {
            assert!(plain_http_to_remote(base), "{base} must warn");
        }
    }

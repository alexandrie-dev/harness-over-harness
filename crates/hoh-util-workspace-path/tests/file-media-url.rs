use hoh_util_workspace_path::{FileMediaUrlError, file_media_url};

// Independently written encodeURIComponent results; checking the complete URL
// retains the upstream pathname and decoded search parameter assertions.
const NATIVE_PATHS: [(&str, &str); 2] = [
    (
        "/work/测试 文件#100%.png",
        "%2Fwork%2F%E6%B5%8B%E8%AF%95%20%E6%96%87%E4%BB%B6%23100%25.png",
    ),
    (
        r"C:\work\测试 文件.png",
        "C%3A%5Cwork%5C%E6%B5%8B%E8%AF%95%20%E6%96%87%E4%BB%B6.png",
    ),
];

#[test]
fn preserves_decoded_native_path_under_mounted_application() {
    for (path, encoded) in NATIVE_PATHS {
        assert_eq!(
            file_media_url("https://host/tools/dsh/", path),
            Ok(Some(format!(
                "https://host/tools/dsh/api/file?path={encoded}"
            )))
        );
    }
}

#[test]
fn rejects_non_local_or_invalid_path() {
    for path in [
        "image.png",
        "//host/image.png",
        r"\\host\image.png",
        "/work/\0.png",
        "/work/\n.png",
    ] {
        assert_eq!(file_media_url("https://host/", path), Ok(None), "{path:?}");
    }
}

#[test]
fn serves_decoded_native_paths_through_desktop_application() {
    for base in ["dsh-app://app/", "dsh-app://app/index.html"] {
        for (path, encoded) in NATIVE_PATHS {
            assert_eq!(
                file_media_url(base, path),
                Ok(Some(format!("dsh-app://app/api/file?path={encoded}"))),
                "{base}, {path}"
            );
        }
    }
}

#[test]
fn rejects_unsupported_application_base() {
    for base in [
        "file:///app/",
        "dsh-app://shell/",
        "dsh-app://app.example/",
        "dsh-app://app:80/",
        "dsh-app://user@app/",
        "dsh-app://app@other/",
        "about:blank",
    ] {
        assert_eq!(file_media_url(base, "/work/image.png"), Ok(None), "{base}");
    }
}

#[test]
fn preserves_userinfo_and_normalizes_default_port() {
    assert_eq!(
        file_media_url(
            "https://user:pa%3Ass@HOST:443/tools/dsh/",
            "/work/image.png"
        ),
        Ok(Some(
            "https://user:pa%3Ass@host/tools/dsh/api/file?path=%2Fwork%2Fimage.png".into()
        ))
    );
}

#[test]
fn serializes_ipv6_host() {
    assert_eq!(
        file_media_url(
            "https://[2001:0db8:0:0:0:0:0:1]:443/tools/dsh/",
            "/work/image.png"
        ),
        Ok(Some(
            "https://[2001:db8::1]/tools/dsh/api/file?path=%2Fwork%2Fimage.png".into()
        ))
    );
}

#[test]
fn serializes_idna_host() {
    assert_eq!(
        file_media_url("https://bücher.example/tools/dsh/", "/work/image.png"),
        Ok(Some(
            "https://xn--bcher-kva.example/tools/dsh/api/file?path=%2Fwork%2Fimage.png".into()
        ))
    );
}

#[test]
fn decodes_percent_encoded_host() {
    assert_eq!(
        file_media_url("https://%65XAMPLE.com/tools/dsh/", "/work/image.png"),
        Ok(Some(
            "https://example.com/tools/dsh/api/file?path=%2Fwork%2Fimage.png".into()
        ))
    );
}

#[test]
fn canonicalizes_numeric_ipv4_hosts() {
    for base in [
        "http://2130706433/tools/dsh/",
        "http://0x7f.1/tools/dsh/",
        "http://0177.0.0.1/tools/dsh/",
    ] {
        assert_eq!(
            file_media_url(base, "/work/image.png"),
            Ok(Some(
                "http://127.0.0.1/tools/dsh/api/file?path=%2Fwork%2Fimage.png".into()
            )),
            "{base}"
        );
    }
}

#[test]
fn trims_url_whitespace_only_after_source_prefix_guard() {
    for base in [
        "https://host/tools/dsh/ \r\n\0",
        "https://ho\tst/tools/\ndsh/\r",
        "dsh-app://app/index.html\t\n ",
    ] {
        let expected = if base.starts_with("https:") {
            "https://host/tools/dsh/api/file?path=%2Fwork%2Fimage.png"
        } else {
            "dsh-app://app/api/file?path=%2Fwork%2Fimage.png"
        };
        assert_eq!(
            file_media_url(base, "/work/image.png"),
            Ok(Some(expected.into())),
            "{base:?}"
        );
    }
    for base in [
        " https://host/",
        "\nhttps://host/",
        "\0https://host/",
        "HTTPS://host/",
    ] {
        assert_eq!(
            file_media_url(base, "/work/image.png"),
            Ok(None),
            "{base:?}"
        );
    }
}

#[test]
fn returns_error_for_invalid_base_that_reaches_url_constructor() {
    for base in [
        "https:",
        "http://",
        "https://[invalid]/",
        "https://host:65536/",
        "https://%ZZ/",
    ] {
        let error = file_media_url(base, "/work/image.png").unwrap_err();
        assert!(matches!(error, FileMediaUrlError::InvalidUrl(_)), "{base}");
        assert_eq!(error.to_string(), "Invalid URL");
        assert!(std::error::Error::source(&error).is_some());
    }
}

#[test]
fn source_guards_return_none_before_parsing_invalid_base() {
    for path in [
        "relative.png",
        "//host/image.png",
        r"/\host/image.png",
        "/work/\0.png",
        "/work/\u{7f}.png",
    ] {
        assert_eq!(
            file_media_url("https://[invalid]/", path),
            Ok(None),
            "{path:?}"
        );
    }
    for base in ["not a URL", "ftp://[invalid]/", "dsh-app://app:invalid/"] {
        assert_eq!(file_media_url(base, "/work/image.png"), Ok(None), "{base}");
    }
}

#[test]
fn accepts_http_bases_without_standard_double_slash() {
    for base in [
        "https:host/tools/dsh/",
        "https:/host/tools/dsh/",
        r"https:\host\tools\dsh\",
    ] {
        assert_eq!(
            file_media_url(base, "/work/image.png"),
            Ok(Some(
                "https://host/tools/dsh/api/file?path=%2Fwork%2Fimage.png".into()
            )),
            "{base}"
        );
    }
}

#[test]
fn resolves_dot_segments_and_replaces_base_query_and_fragment() {
    for (base, expected) in [
        (
            "https://host/tools/old/../dsh/%2e/index.html?old=1#section",
            "https://host/tools/dsh/api/file?path=%2Fwork%2Fimage.png",
        ),
        (
            "dsh-app://app/tools/../index.html?old=1#section",
            "dsh-app://app/api/file?path=%2Fwork%2Fimage.png",
        ),
    ] {
        assert_eq!(
            file_media_url(base, "/work/image.png"),
            Ok(Some(expected.into()))
        );
    }
}

#[test]
fn applies_special_query_encoding_after_encode_uri_component() {
    for (base, expected) in [
        (
            "https://host/",
            "https://host/api/file?path=%2Fwork%2Fa%27b!()*~%20%25.png",
        ),
        (
            "dsh-app://app/",
            "dsh-app://app/api/file?path=%2Fwork%2Fa'b!()*~%20%25.png",
        ),
    ] {
        assert_eq!(
            file_media_url(base, "/work/a'b!()*~ %.png"),
            Ok(Some(expected.into()))
        );
    }
}

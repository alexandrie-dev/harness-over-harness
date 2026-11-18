use hoh_error::{BoxError, HarnessError, SharedError};
use hoh_fs::*;
#[test]
fn maps_host_errno_to_fs_codes_with_the_original_error_as_cause() {
    for (errno, code, detail) in [
        ("ENOENT", "FS_NOT_FOUND", "not found"),
        ("ENOTDIR", "FS_NOT_FOUND", "not found"),
        ("EACCES", "FS_PERMISSION_DENIED", "permission denied"),
        ("EPERM", "FS_PERMISSION_DENIED", "permission denied"),
        ("EISDIR", "FS_NOT_REGULAR_FILE", "not a regular file"),
        ("ELOOP", "FS_IO_ERROR", "ELOOP: injected"),
    ] {
        let cause = SharedError::new(ErrnoError::new(
            Some(errno.into()),
            format!("{errno}: injected"),
        ));
        let e = fs_error_from_errno("read", "~/a", cause.clone().into());
        let e = HarnessError::of(e.as_ref()).unwrap();
        assert_eq!(e.name(), "FsError");
        assert_eq!(e.code(), code);
        assert_eq!(e.message(), format!("cannot read \"~/a\": {detail}"));
        assert!(
            e.cause()
                .unwrap()
                .downcast_ref::<SharedError>()
                .unwrap()
                .ptr_eq(&cause)
        );
    }
}
#[derive(Debug)]
struct NumericCode {
    code: i32,
}
impl std::fmt::Display for NumericCode {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "numeric code: {}", self.code)
    }
}
impl std::error::Error for NumericCode {}
#[derive(Debug)]
struct ThrownString(&'static str);
impl std::fmt::Display for ThrownString {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(self.0)
    }
}
impl std::error::Error for ThrownString {}
#[test]
fn returns_typed_failures_and_defects_unchanged() {
    let errors: Vec<BoxError> = vec![
        FsError::new("already typed", FsErrorCode::TooLarge).into(),
        std::io::Error::other("defect").into(),
        Box::new(NumericCode { code: 1 }),
        Box::new(ThrownString("not an error")),
        std::io::Error::from_raw_os_error(40).into(),
    ];
    for e in errors {
        let shared = SharedError::new(e);
        let result = fs_error_from_errno("read", "a", shared.clone().into());
        assert!(
            result
                .downcast_ref::<SharedError>()
                .unwrap()
                .ptr_eq(&shared)
        );
    }
}

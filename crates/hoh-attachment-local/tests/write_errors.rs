//! Only dedicated children change RLIMIT_FSIZE or inherit ignored SIGXFSZ.
#[cfg(unix)]
fn main() {
    if std::env::args().nth(1).as_deref() == Some("--attachment-write-error-child") {
        let case = std::env::args().nth(2).unwrap();
        tokio::runtime::Builder::new_multi_thread()
            .worker_threads(2)
            .max_blocking_threads(2)
            .enable_all()
            .build()
            .unwrap()
            .block_on(child(&case));
        return;
    }
    let mut failures = Vec::new();
    for case in ["immutable", "request-cache"] {
        // Ignored signals survive exec; no unsafe pre_exec/sigaction is needed.
        let output = std::process::Command::new("sh")
            .args(["-c", "trap '' XFSZ; exec \"$@\"", "attachment-write-errors"])
            .arg(std::env::current_exe().unwrap())
            .arg("--attachment-write-error-child")
            .arg(case)
            .output()
            .unwrap();
        if !output.status.success() {
            failures.push(format!(
                "{case}: child failed\n{}\n{}",
                String::from_utf8_lossy(&output.stdout),
                String::from_utf8_lossy(&output.stderr)
            ));
        }
    }
    assert!(failures.is_empty(), "{}", failures.join("\n"));
}

#[cfg(not(unix))]
fn main() {}

#[cfg(unix)]
async fn child(case: &str) {
    use hoh_attachment::{
        ImageAttachmentRef, ImageMediaType, ImageRequestTarget, StoredImageAttachment,
    };
    use hoh_attachment_local::{
        read_request_image_file, request_image_variant_id, store::publish_immutable_object,
    };
    use hoh_error::{BoxError, HarnessError};
    use rustix::process::{Resource, Rlimit, getrlimit, setrlimit};
    use sha2::{Digest, Sha256};

    let home = tempfile::tempdir().unwrap();
    let root = home.path().join("attachments/v1");
    let root = root.to_str().unwrap();
    let bytes = vec![42; 4096];
    let hash = |data: &[u8]| -> String {
        Sha256::digest(data)
            .iter()
            .map(|byte| format!("{byte:02x}"))
            .collect()
    };
    let digest = hash(&bytes);
    let object = format!("{root}/objects/{}/{digest}", &digest[..2]);
    let mut png = std::io::Cursor::new(Vec::new());
    image::DynamicImage::new_rgb8(64, 32)
        .write_to(&mut png, image::ImageFormat::Png)
        .unwrap();
    let data = png.into_inner();
    let attachment = StoredImageAttachment {
        r#ref: ImageAttachmentRef {
            attachment_id: format!("sha256:{}", hash(&data)).into(),
            media_type: ImageMediaType::Png,
            bytes: data.len() as u64,
            width: 64,
            height: 32,
            name: None,
            original_dimensions: None,
        },
        data,
    };
    let target = ImageRequestTarget {
        width: 16,
        height: 16,
        max_bytes: 4096,
    };
    let variant = request_image_variant_id(&attachment.r#ref, &target);
    let hash = &variant.as_str()[7..];
    let cache = format!("{root}/request-images/{}/{hash}", &hash[..2]);
    tokio::fs::create_dir_all(std::path::Path::new(&cache).parent().unwrap())
        .await
        .unwrap();
    let old = b"previous cached bytes";
    tokio::fs::write(&cache, old).await.unwrap();
    let before = getrlimit(Resource::Fsize);
    setrlimit(
        Resource::Fsize,
        Rlimit {
            current: Some(64),
            maximum: before.maximum,
        },
    )
    .unwrap();
    let result: Result<(), BoxError> = match case {
        "immutable" => publish_immutable_object(root, &object, &bytes, &digest).await,
        "request-cache" => read_request_image_file(root, &attachment, &target, None)
            .await
            .map(|_| ()),
        _ => panic!("unknown child case"),
    };
    setrlimit(Resource::Fsize, before).unwrap();
    let failure = result.expect_err("a short write must fail before publication");
    if case == "immutable" {
        let storage = HarnessError::of(failure.as_ref()).unwrap();
        assert_eq!(storage.code(), "ATTACHMENT_WRITE_FAILED");
        assert_eq!(storage.message(), "Unable to persist attachment.");
        assert_eq!(
            std::error::Error::source(storage).unwrap().to_string(),
            "EFBIG: file too large, write"
        );
    } else {
        assert_eq!(failure.to_string(), "EFBIG: file too large, write");
    }
    let mut cause: &(dyn std::error::Error + 'static) = failure.as_ref();
    while let Some(next) = cause.source() {
        cause = next;
    }
    assert_eq!(
        cause
            .downcast_ref::<std::io::Error>()
            .unwrap()
            .raw_os_error(),
        Some(rustix::io::Errno::FBIG.raw_os_error())
    );
    assert!(
        !std::path::Path::new(&object).exists(),
        "no partial immutable object published"
    );
    assert_eq!(
        tokio::fs::read(&cache).await.unwrap(),
        old,
        "previous file preserved"
    );
    if case == "immutable" {
        assert!(
            tokio::fs::read_dir(format!("{root}/tmp"))
                .await
                .unwrap()
                .next_entry()
                .await
                .unwrap()
                .is_none()
        );
    } else {
        let mut entries = tokio::fs::read_dir(std::path::Path::new(&cache).parent().unwrap())
            .await
            .unwrap();
        while let Some(entry) = entries.next_entry().await.unwrap() {
            assert!(!entry.file_name().to_string_lossy().ends_with(".tmp"));
        }
    }
}

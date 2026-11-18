use hoh_error::{BoxError, SharedError};
use hoh_subprocess::{SubprocessEnvironment, control::*};
use std::{cell::Cell, io};
use tokio::io::{AsyncReadExt, AsyncWriteExt, DuplexStream};

fn environment(marker: Option<&str>) -> SubprocessEnvironment {
    [(SUBPROCESS_CONTROL_ENV.to_owned(), marker.map(str::to_owned))]
        .into_iter()
        .collect()
}

#[test]
fn consumes_the_provider_marker_and_opens_a_duplex_descriptor_before_allowing_another_open() {
    let mut environment = environment(Some("pipe"));
    let calls = Cell::new(0);
    let (endpoint, _peer) = tokio::io::duplex(8);
    let opened = open_inherited_control_channel(&mut environment, |options| {
        calls.set(calls.get() + 1);
        assert_eq!(
            options,
            ControlChannelOptions {
                fd: 7,
                readable: true,
                writable: true,
                allow_half_open: true
            }
        );
        Ok(endpoint)
    })
    .unwrap();
    assert!(!environment.contains_key(SUBPROCESS_CONTROL_ENV));
    assert_eq!(calls.get(), 1);
    let again = open_inherited_control_channel::<DuplexStream>(&mut environment, |_| {
        calls.set(calls.get() + 1);
        Err(Box::new(io::Error::other("must not open")))
    });
    assert_eq!(
        again.unwrap_err().to_string(),
        "subprocess control channel was not inherited"
    );
    assert_eq!(calls.get(), 1);
    drop(opened);
}

fn rejected(mut environment: SubprocessEnvironment) {
    let calls = Cell::new(0);
    let error = open_inherited_control_channel::<DuplexStream>(&mut environment, |_| {
        calls.set(calls.get() + 1);
        Err(Box::new(io::Error::other("must not open")))
    })
    .unwrap_err();
    assert_eq!(
        error.to_string(),
        "subprocess control channel was not inherited"
    );
    assert!(!environment.contains_key(SUBPROCESS_CONTROL_ENV));
    assert_eq!(calls.get(), 0);
}

#[test]
fn rejects_an_absent_launch_marker() {
    rejected(SubprocessEnvironment::new());
    rejected(environment(None));
}

#[test]
fn rejects_an_invalid_launch_marker() {
    rejected(environment(Some("invalid")));
}

#[test]
fn consumes_the_marker_even_when_the_descriptor_opener_fails() {
    let original = SharedError::new(io::Error::other("opening fd 7 failed"));
    let mut environment = environment(Some("pipe"));
    let failure = open_inherited_control_channel::<DuplexStream>(&mut environment, |_| {
        Err(Box::new(original.clone()) as BoxError)
    })
    .unwrap_err();
    assert!(
        failure
            .downcast_ref::<SharedError>()
            .unwrap()
            .ptr_eq(&original)
    );
    assert!(!environment.contains_key(SUBPROCESS_CONTROL_ENV));
}

async fn preserves_bytes_and_half_close<S: hoh_subprocess::SubprocessDuplex>(
    mut opened: S,
    mut peer: S,
) {
    let payload = [0, 0xff, b'\n', b'\r', 0, 0x80, b'{', b'}'];
    for fragment in payload.chunks(3) {
        peer.write_all(fragment).await.unwrap();
    }
    peer.shutdown().await.unwrap();
    let mut received = Vec::new();
    opened.read_to_end(&mut received).await.unwrap();
    assert_eq!(received, payload);
    // EOF on the readable side must leave the other direction writable (allowHalfOpen).
    opened.write_all(b"\x00reply\xff\n").await.unwrap();
    opened.shutdown().await.unwrap();
    let mut response = Vec::new();
    peer.read_to_end(&mut response).await.unwrap();
    assert_eq!(response, b"\x00reply\xff\n");
}

#[tokio::test]
async fn preserves_every_control_byte_and_independent_half_close_in_memory() {
    let (endpoint, peer) = tokio::io::duplex(64);
    let mut environment = environment(Some("pipe"));
    let opened = open_inherited_control_channel(&mut environment, |_| Ok(endpoint)).unwrap();
    preserves_bytes_and_half_close(opened, peer).await;
}

#[cfg(unix)]
#[tokio::test]
async fn preserves_every_control_byte_and_independent_half_close_on_unix_socket() {
    let (endpoint, peer) = tokio::net::UnixStream::pair().unwrap();
    let mut environment = environment(Some("pipe"));
    let opened = open_inherited_control_channel(&mut environment, |options| {
        assert_eq!(options.fd, 7);
        assert!(options.allow_half_open);
        Ok(endpoint)
    })
    .unwrap();
    preserves_bytes_and_half_close(opened, peer).await;
}

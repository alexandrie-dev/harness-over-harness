use crate::{encoding::EncodedImage, request_image::*, test_support::*};
#[tokio::test]
async fn rejects_an_encoded_request_whose_decoded_facts_disagree_with_the_encoder_result() {
    let image = EncodedImage {
        data: png(),
        media_type: hoh_attachment::ImageMediaType::Png,
        width: 2,
        height: 1,
    };
    let failure = assert_code(
        verify_request_image(image, false).await,
        "ATTACHMENT_WRITE_FAILED",
    );
    assert_eq!(
        failure.to_string(),
        "Encoded model-request image does not match its verified 8-bit sRGB metadata."
    );
}

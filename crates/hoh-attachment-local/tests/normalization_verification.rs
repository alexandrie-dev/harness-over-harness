use crate::{normalization::*, test_support::*};
#[tokio::test]
async fn rejects_a_normalized_output_whose_decoded_facts_disagree_with_the_encoder_result() {
    let image = NormalizedImage {
        data: png(),
        media_type: hoh_attachment::ImageMediaType::Png,
        width: 2,
        height: 1,
    };
    let failure = assert_code(
        verify_normalized_image(image, Some(false)).await,
        "ATTACHMENT_WRITE_FAILED",
    );
    assert_eq!(
        failure.to_string(),
        "Image normalization did not produce a single-frame 8-bit sRGB image with matching metadata."
    );
}

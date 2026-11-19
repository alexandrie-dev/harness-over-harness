use hoh_attachment::{AttachmentId, ImageAttachmentRef, ImageMediaType};
use hoh_spill_policy::retention::{RetainableBlock, retain_content};

fn text(value: &str) -> RetainableBlock {
    RetainableBlock::text(value)
}

fn image(name: char) -> RetainableBlock {
    RetainableBlock::Image {
        attachment: ImageAttachmentRef {
            attachment_id: AttachmentId::from(format!("sha256:{}", name.to_string().repeat(64))),
            media_type: ImageMediaType::Png,
            width: 1,
            height: 1,
            bytes: 1,
            name: None,
            original_dimensions: None,
        },
        offloaded: None,
    }
}

fn price(block: &RetainableBlock) -> u64 {
    block
        .text_value()
        .map_or(2, |value| value.encode_utf16().count() as u64)
}

#[test]
fn keeps_images_in_position_around_truncated_middle_text() {
    let result = retain_content(
        &[
            text("AA"),
            image('a'),
            text("B1B2B3B4B5"),
            image('b'),
            text("CC"),
        ],
        12,
        price,
    );
    assert_eq!(result.head, vec![text("AA"), image('a'), text("B1")]);
    assert_eq!(result.tail, vec![text("B5"), image('b'), text("CC")]);
    assert_eq!((result.omitted_bytes, result.omitted_images), (6, 0));
}

#[test]
fn omits_a_middle_image_and_surrounding_middle_text() {
    let result = retain_content(
        &[text(&"A".repeat(20)), image('a'), text(&"C".repeat(20))],
        10,
        price,
    );
    assert_eq!(result.head, vec![text("AAAAA")]);
    assert_eq!(result.tail, vec![text("CCCCC")]);
    assert_eq!((result.omitted_bytes, result.omitted_images), (30, 1));
}

#[test]
fn does_not_cross_an_image_that_cannot_fit_wholly() {
    let result = retain_content(&[text("A"), image('a'), text("B")], 6, |block| {
        if block.image().is_some() {
            5
        } else {
            price(block)
        }
    });
    assert_eq!(result.head, vec![text("A")]);
    assert_eq!(result.tail, vec![text("B")]);
    assert_eq!((result.omitted_bytes, result.omitted_images), (0, 1));
}

#[test]
fn never_duplicates_overlapping_head_and_tail_content() {
    let original = vec![text("AB"), image('a'), text("CD")];
    let result = retain_content(&original, 100, price);
    assert_eq!([result.head, result.tail].concat(), original);
    assert_eq!((result.omitted_bytes, result.omitted_images), (0, 0));
}

#[test]
fn omits_everything_when_notices_consume_the_budget() {
    let result = retain_content(&[text("雪"), image('a')], 0, price);
    assert!(result.head.is_empty() && result.tail.is_empty());
    assert_eq!((result.omitted_bytes, result.omitted_images), (3, 1));
}

#[test]
fn omits_text_when_minimum_framing_exceeds_the_budget() {
    let result = retain_content(&[text("ABC")], 4, |block| {
        block.text_value().map_or_else(
            || price(block),
            |value| value.encode_utf16().count() as u64 + 4,
        )
    });
    assert!(result.head.is_empty() && result.tail.is_empty());
    assert_eq!((result.omitted_bytes, result.omitted_images), (3, 0));
}

#[test]
fn drops_an_incomplete_surrogate_pair_at_the_retained_prefix() {
    let result = retain_content(&[text("A😀middleZ")], 4, price);
    assert_eq!(result.head, vec![text("A")]);
    assert_eq!(result.tail, vec![text("eZ")]);
    assert_eq!(result.omitted_bytes, "😀middl".len() as u64);
}

#[test]
fn does_not_split_a_surrogate_pair_at_either_retained_end() {
    let result = retain_content(&[text("A😀middle😀Z")], 5, price);
    assert_eq!(result.head, vec![text("A😀")]);
    assert_eq!(result.tail, vec![text("Z")]);
    assert_eq!(result.omitted_bytes, "middle😀".len() as u64);
}

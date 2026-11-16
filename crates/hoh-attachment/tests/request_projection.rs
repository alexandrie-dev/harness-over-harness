use hoh_attachment::{ProjectedDimensions, long_edge_dimensions, request_image_dimensions};

fn assert_projection(width: u64, height: u64, expected_width: u64, expected_height: u64) {
    let projected = request_image_dimensions(width, height, 640_000);
    assert_eq!(
        projected,
        ProjectedDimensions {
            width: expected_width,
            height: expected_height
        }
    );
    assert!(projected.width * projected.height <= 640_000);
}

#[test]
fn projects_4096x4096_under_640000_pixels_as_800x800() {
    assert_projection(4096, 4096, 800, 800);
}

#[test]
fn projects_4096x2048_under_640000_pixels_as_1130x565() {
    assert_projection(4096, 2048, 1130, 565);
}

#[test]
fn projects_3840x2160_under_640000_pixels_as_1066x600() {
    assert_projection(3840, 2160, 1066, 600);
}

#[test]
fn projects_320x240_under_640000_pixels_as_320x240() {
    assert_projection(320, 240, 320, 240);
}

#[test]
fn projects_a_portrait_within_the_same_total_pixel_budget() {
    assert_projection(2160, 3840, 600, 1066);
}

#[test]
fn rounds_a_portrait_inward_when_integer_aspect_rounding_crosses_the_pixel_cap() {
    assert_eq!(
        request_image_dimensions(2, 4, 5),
        ProjectedDimensions {
            width: 1,
            height: 2
        }
    );
}

#[test]
fn keeps_the_long_edge_exact_and_rounds_the_short_edge() {
    for (width, height, long_edge, expected_width, expected_height) in [
        (8000, 40, 4096, 4096, 20),
        (10_000, 100, 4096, 4096, 41),
        (1080, 2400, 1862, 838, 1862),
        (1, 9000, 4096, 1, 4096),
    ] {
        assert_eq!(
            long_edge_dimensions(width, height, long_edge),
            ProjectedDimensions {
                width: expected_width,
                height: expected_height
            }
        );
    }
}

#[test]
fn never_enlarges_a_source_at_or_below_the_long_edge() {
    for edge in [320, 4096] {
        assert_eq!(
            long_edge_dimensions(320, 240, edge),
            ProjectedDimensions {
                width: 320,
                height: 240
            }
        );
    }
}

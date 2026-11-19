use hoh_output_retention::Omitted;
use hoh_spill::{SpillRef, spill_locator};
use hoh_spill_policy::notice::{format_spill_notice, has_spill_notice};

fn reference() -> SpillRef {
    SpillRef {
        locator: spill_locator("/spill/output.txt"),
        bytes: 0,
        retrieval_hint: "Read the file.".into(),
    }
}

#[test]
fn preserves_the_historical_spelling_independently_of_the_formatter() {
    let historical =
        "(Omitted 50000 bytes. Full formatted result stored at: /spill/output.txt. Read the file.)";
    assert_eq!(
        format_spill_notice(&Omitted::Exact { count: 50_000 }, &reference(), 0),
        historical
    );
    assert!(has_spill_notice(historical));
    assert!(has_spill_notice(&format!(
        "failed\n[exit code: 7]\n\n{historical}"
    )));
}

#[test]
fn recognizes_every_omission_form_the_producer_can_format() {
    for omitted in [Omitted::None, Omitted::Unknown, Omitted::Exact { count: 0 }] {
        let notice = format_spill_notice(&omitted, &reference(), 0);
        assert!(has_spill_notice(&notice));
        assert!(has_spill_notice(&format!("preview\n\n{notice}")));
    }
}

#[test]
fn reports_whole_image_omissions_and_recognizes_historical_notices() {
    let notice = format_spill_notice(&Omitted::Exact { count: 12 }, &reference(), 2);
    assert!(notice.contains("Omitted 12 bytes. Omitted 2 images."));
    assert!(has_spill_notice(&notice));
    assert!(!has_spill_notice(&notice.replace("2 images", "02 images")));
    assert!(!has_spill_notice(
        &notice.replace("2 images", "9007199254740992 images")
    ));
    assert!(!has_spill_notice(&notice.replace("2 images", "+2 images")));
}

#[test]
fn locator_and_retrieval_text_stay_opaque() {
    let reference = SpillRef {
        locator: spill_locator("/spill/报告 (1).txt"),
        bytes: 0,
        retrieval_hint: "Read it.\n\n(additional guidance)".into(),
    };
    let notice = format_spill_notice(&Omitted::Exact { count: 42 }, &reference, 0);
    assert!(has_spill_notice(&notice));
    assert!(has_spill_notice(&format!("preview\n\n{notice}")));
}

#[test]
fn scans_repeated_non_notice_prefixes_without_losing_the_final_notice() {
    let prefix = "\n\n(ordinary output".repeat(20_000);
    assert!(!has_spill_notice(&(prefix.clone() + ")")));
    let notice = format_spill_notice(&Omitted::Exact { count: 42 }, &reference(), 0);
    assert!(has_spill_notice(&(prefix + "\n\n" + &notice)));
}

#[test]
fn rejects_ordinary_partial_non_final_and_malformed_notice_text() {
    let notice = format_spill_notice(&Omitted::Exact { count: 42 }, &reference(), 0);
    let cases = [
        "".into(),
        "ordinary output)".into(),
        "(ordinary output)".into(),
        "\n\n(ordinary output)".into(),
        format!("prefix{notice}"),
        notice[..notice.len() - 1].into(),
        format!("{notice}\nother output"),
        notice.replace("42", "-1"),
        notice.replace("42", "1.5"),
        notice.replace("42", "0042"),
        notice.replace("42", "+42"),
        notice.replace("Omitted", "Ignored"),
        notice.replace(". Read the file.", ""),
    ];
    for text in cases {
        assert!(!has_spill_notice(&text), "accepted {text:?}");
    }
}

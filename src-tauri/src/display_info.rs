//! The host's shareable displays and the sizes they report.
//!
//! Capture presets are a short list (1080p, 1440p, ...) and the host's panel is
//! not necessarily one of them: the machine in the field reports 1710x1107, so
//! a 1920x1080 preset letterboxes or crops it and the remote picture looks cut
//! off even though capture "works". Reporting the display's own size lets the
//! frontend derive an aspect-correct preset instead of hard-coding 16:9, and
//! listing every shareable display is the groundwork for letting a multi-screen
//! host pick which one to share.
//!
//! The descriptions come from ScreenCaptureKit through `macos-media-toolkit`.
//! In the current toolkit version they read `display Id(1) (1710x1107)`: the
//! display id is printed with its wrapper type, while older/other versions use
//! the plain `display 3 (2560x1440)` form. Both are parsed. Note that the
//! dimensions are what ScreenCaptureKit reports for the display in its current
//! mode — logical points on a Retina panel, not necessarily the backing pixel
//! count. Parsing lives in pure helpers so the format is testable without
//! Screen Recording permission or a display, and enumeration failures become a
//! sentence the UI can show while the user falls back to a preset.

use macos_media_toolkit::screen::ScreenCapture;

/// One shareable display, in ScreenCaptureKit's order.
#[derive(Debug, Clone, serde::Serialize)]
#[serde(rename_all = "camelCase")]
pub struct DisplayInfo {
    /// Zero-based index in ScreenCaptureKit's shareable-display order.
    pub index: usize,
    /// Human label, e.g. "Built-in Retina Display" if available, otherwise the
    /// raw description.
    pub label: String,
    pub width: u32,
    pub height: u32,
}

/// Every shareable display in the order ScreenCaptureKit lists them.
///
/// The order is ScreenCaptureKit's (the main display is first), and a
/// malformed description is skipped rather than turned into a fake entry. An
/// error is returned when enumeration fails or when nothing usable is
/// reported, so the UI can say so instead of showing an empty list.
pub fn displays() -> Result<Vec<DisplayInfo>, String> {
    let descriptions = ScreenCapture::display_descriptions().map_err(|error| {
        format!("Could not read the display sizes: {error}. Choose a capture preset instead.")
    })?;
    collect_displays(&descriptions)
}

/// Size of the main shareable display.
///
/// ScreenCaptureKit lists the main display first, so the lowest parsed index is
/// the one to offer. Returns the same message as [`displays`] when enumeration
/// fails, so the host can fall back to a capture preset.
pub fn main_display_size() -> Result<(u32, u32), String> {
    let displays = displays()?;
    first_display_size(&displays)
}

/// Builds the display list from toolkit descriptions in ScreenCaptureKit order.
///
/// The public `index` is the **enumeration position**, because that is what
/// `ScreenCaptureConfig::display_index` expects; the parsed display id is only
/// used for a readable label. Pure so ordering, labels and skip rules can be
/// tested without ScreenCaptureKit. Returns an error for an empty or entirely
/// unparsable list.
fn collect_displays(descriptions: &[String]) -> Result<Vec<DisplayInfo>, String> {
    let displays: Vec<DisplayInfo> = descriptions
        .iter()
        .enumerate()
        .filter_map(|(position, description)| {
            let (display_id, width, height) = parse_display(description)?;
            Some(DisplayInfo {
                index: position,
                // The toolkit exposes no human display name; its id is the
                // stable part, so both "display 3 (…)" and "display Id(3) (…)"
                // read as "display 3".
                label: format!("display {display_id}"),
                width,
                height,
            })
        })
        .collect();
    if displays.is_empty() {
        return Err(empty_displays_error());
    }
    Ok(displays)
}

/// The main display's size: the first entry of an already ordered list.
fn first_display_size(displays: &[DisplayInfo]) -> Result<(u32, u32), String> {
    displays
        .first()
        .map(|display| (display.width, display.height))
        .ok_or_else(empty_displays_error)
}

/// Parses a toolkit display description into `(index, width, height)`.
///
/// The live format is `display Id(1) (1710x1107)`; a plain `display 3
/// (2560x1440)` is accepted too. The size is always the last parenthesised
/// group, so that is located first; whatever precedes it must carry the index,
/// with or without the toolkit's `Id(…)` wrapper.
///
/// Returns `None` for a description without an index, without a parenthesised
/// size, without both numeric dimensions, or with a zero dimension, so a
/// malformed entry is skipped instead of becoming a fake or 0-pixel display.
fn parse_display(description: &str) -> Option<(usize, u32, u32)> {
    let rest = description.trim().strip_prefix("display")?.trim();
    // The last '(' starts the size; an earlier '(' may belong to "Id(1)".
    let open = rest.rfind('(')?;
    let after_open = &rest[open + 1..];
    let close = after_open.find(')')?;
    let (width, height) = after_open[..close].split_once('x')?;
    let width = width.trim().parse::<u32>().ok()?;
    let height = height.trim().parse::<u32>().ok()?;
    if width == 0 || height == 0 {
        return None;
    }
    let index = parse_display_index(rest[..open].trim())?;
    Some((index, width, height))
}

/// Reads the display id from the text before the size group.
///
/// Accepts both the toolkit's wrapper form (`Id(1)`) and a bare number (`1`),
/// so a future change to a plainer description keeps working.
fn parse_display_index(prefix: &str) -> Option<usize> {
    let prefix = prefix.trim();
    if prefix.is_empty() {
        return None;
    }
    if let (Some(open), Some(close)) = (prefix.find('('), prefix.rfind(')')) {
        if close <= open + 1 {
            return None;
        }
        return prefix[open + 1..close].trim().parse::<usize>().ok();
    }
    prefix.parse::<usize>().ok()
}

fn empty_displays_error() -> String {
    "No display reported a usable pixel size. Choose a capture preset instead.".to_string()
}

#[cfg(test)]
mod tests {
    use super::*;

    fn descriptions(values: &[&str]) -> Vec<String> {
        values.iter().map(|value| value.to_string()).collect()
    }

    #[test]
    fn parses_toolkit_descriptions() {
        assert_eq!(
            parse_display("display 3 (2560x1440)"),
            Some((3, 2560, 1440))
        );
        assert_eq!(
            parse_display("display 1 (3456x2234)"),
            Some((1, 3456, 2234))
        );
        // The live toolkit prints the id with its wrapper type. These are the
        // real forms this Mac produced (`display Id(1) (1710x1107)`), so the
        // parser must not depend on the cleaner "display 3" spelling.
        assert_eq!(
            parse_display("display Id(1) (1710x1107)"),
            Some((1, 1710, 1107))
        );
        assert_eq!(
            parse_display("display Id(1) (2880x1712)"),
            Some((1, 2880, 1712))
        );
    }

    #[test]
    fn parses_multi_digit_indices() {
        assert_eq!(
            parse_display("display 12 (1920x1080)"),
            Some((12, 1920, 1080))
        );
        assert_eq!(
            parse_display("display Id(12) (1920x1080)"),
            Some((12, 1920, 1080))
        );
        assert_eq!(parse_display("display 0 (1024x768)"), Some((0, 1024, 768)));
    }

    #[test]
    fn rejects_descriptions_without_a_size() {
        for description in [
            "display 1",
            "display Id(1)",
            "display 1 ()",
            "display 1 (2560)",
            "display 1 (2560x)",
            "display 1 (x1440)",
            "display",
            "garbage",
            "",
        ] {
            assert_eq!(
                parse_display(description),
                None,
                "expected {description:?} to be rejected"
            );
        }
    }

    #[test]
    fn rejects_missing_or_invalid_indices() {
        assert_eq!(parse_display("display (2560x1440)"), None);
        assert_eq!(parse_display("display Id() (2560x1440)"), None);
        assert_eq!(parse_display("display Id(x) (2560x1440)"), None);
        assert_eq!(parse_display("display x (2560x1440)"), None);
        assert_eq!(parse_display("display -1 (2560x1440)"), None);
        assert_eq!(parse_display("display Id(1) (0x1440)"), None);
    }

    #[test]
    fn rejects_zero_dimensions() {
        assert_eq!(parse_display("display 1 (0x1440)"), None);
        assert_eq!(parse_display("display 1 (2560x0)"), None);
        assert_eq!(parse_display("display 1 (0x0)"), None);
    }

    #[test]
    fn rejects_non_numeric_sizes() {
        assert_eq!(parse_display("display 1 (widexhigh)"), None);
        // A bare "0x100" has no display prefix at all, even though it looks
        // like a dimension pair.
        assert_eq!(parse_display("0x100"), None);
        assert_eq!(parse_display("display 1 (-1x1440)"), None);
        assert_eq!(parse_display("display 1 (2560x-1)"), None);
        assert_eq!(parse_display("display 1 (2560x1440x2)"), None);
    }

    #[test]
    fn tolerates_whitespace_around_the_description() {
        assert_eq!(
            parse_display("  display  2 ( 2560 x 1440 )  "),
            Some((2, 2560, 1440))
        );
        assert_eq!(
            parse_display("  display   Id(2) ( 2560 x 1440 )  "),
            Some((2, 2560, 1440))
        );
    }

    #[test]
    fn collects_displays_in_enumeration_order_with_id_labels() {
        // The public index is the position ScreenCaptureKit lists the display
        // in (what ScreenCaptureConfig::display_index consumes); the parsed id
        // only labels it. Keep both distinct so a non-zero display id cannot
        // silently become an out-of-range capture index.
        let collected = collect_displays(&descriptions(&[
            "display Id(3) (2560x1440)",
            "display Id(1) (1710x1107)",
            "display Id(2) (1920x1080)",
        ]))
        .expect("valid descriptions");

        let indices: Vec<usize> = collected.iter().map(|display| display.index).collect();
        assert_eq!(indices, vec![0, 1, 2]);
        let labels: Vec<&str> = collected
            .iter()
            .map(|display| display.label.as_str())
            .collect();
        assert_eq!(labels, vec!["display 3", "display 1", "display 2"]);
        assert_eq!((collected[0].width, collected[0].height), (2560, 1440));
        assert_eq!((collected[1].width, collected[1].height), (1710, 1107));
    }

    #[test]
    fn skips_malformed_entries_without_fabricating() {
        let collected = collect_displays(&descriptions(&[
            "display Id(1) (2560x1440)",
            "garbage",
            "display Id(x) (1920x1080)",
            "display 2 (0x100)",
            "display Id(4) (1280x720)",
        ]))
        .expect("one valid description");

        // Two valid entries, in enumeration order; nothing invented for the
        // skipped descriptions.
        assert_eq!(collected.len(), 2);
        assert_eq!(collected[0].index, 0);
        // The skipped descriptions still occupy their ScreenCaptureKit slots,
        // so the surviving entry keeps its original position.
        assert_eq!(collected[1].index, 4);
        assert_eq!(collected[1].label, "display 4");
    }

    #[test]
    fn empty_or_unusable_lists_are_an_error() {
        for descriptions in [descriptions(&[]), descriptions(&["garbage", "display 1"])] {
            let error = collect_displays(&descriptions).expect_err("must not be empty");
            assert!(error.contains("No display"), "{error}");
        }
    }

    #[test]
    fn main_size_is_the_first_listed_display() {
        let collected = collect_displays(&descriptions(&[
            "display Id(3) (2880x1712)",
            "display Id(1) (2560x1440)",
        ]))
        .expect("valid descriptions");
        assert_eq!(first_display_size(&collected), Ok((2880, 1712)));

        let error = first_display_size(&[]).expect_err("empty list");
        assert!(error.contains("No display"), "{error}");
    }
}

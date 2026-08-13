//! SparkID-based unique, time-sortable media filenames.

use sparkid::SparkId;

/// Build a unique media filename `<sparkid>.<ext>`.
///
/// `ext` is normalized: a leading dot is stripped and an empty extension yields
/// just the bare id (the caller should normally have detected a real format).
pub fn sparkid_filename(ext: &str) -> String {
    let id = SparkId::new().to_string();
    let ext = ext.trim_start_matches('.').trim();
    if ext.is_empty() {
        id
    } else {
        format!("{id}.{ext}")
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn filename_has_extension() {
        let name = sparkid_filename("png");
        assert!(name.ends_with(".png"));
        assert!(name.len() > ".png".len());
    }

    #[test]
    fn filename_strips_leading_dot() {
        let name = sparkid_filename(".jpg");
        assert!(name.ends_with(".jpg"));
        assert!(!name.ends_with("..jpg"));
    }

    #[test]
    fn filename_bare_without_extension() {
        let name = sparkid_filename("");
        assert!(!name.contains('.'));
    }

    #[test]
    fn filenames_are_unique_and_ordered() {
        let a = sparkid_filename("png");
        let b = sparkid_filename("png");
        assert_ne!(a, b, "sparkid should be unique per call");
    }
}

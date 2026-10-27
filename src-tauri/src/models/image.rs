string_enum! {
    /// Where an image is in processing. Matches `property_images.processing_status`.
    pub enum ImageProcessingStatus {
        /// Not processed yet, or a transient failure that will be retried.
        Pending => "pending",
        Hashed => "hashed",
        /// Hashed and embedded by the optional local vision model (not used yet).
        Embedded => "embedded",
        /// A permanent problem (corrupt, gone, refused, unsupported...). Not retried.
        Failed => "failed",
        /// Deliberately not processed (e.g. the image host is not allowed for the source).
        Skipped => "skipped",
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn text_form_round_trips_and_rejects_unknown_values() {
        for s in ImageProcessingStatus::ALL {
            assert_eq!(s.as_str().parse::<ImageProcessingStatus>().unwrap(), *s);
        }
        assert!("done".parse::<ImageProcessingStatus>().is_err());
    }
}

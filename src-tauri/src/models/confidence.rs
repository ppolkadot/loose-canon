string_enum! {
    /// How strongly the available evidence supports an extracted value or classification.
    ///
    /// Ordered `Low < Medium < High`. Confidence is about *support*, not about whether the
    /// advertisement itself is truthful.
    #[derive(PartialOrd, Ord)]
    pub enum Confidence {
        Low => "low",
        Medium => "medium",
        High => "high",
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn confidence_is_ordered_low_to_high() {
        assert!(Confidence::Low < Confidence::Medium);
        assert!(Confidence::Medium < Confidence::High);
    }

    #[test]
    fn text_form_round_trips_and_rejects_unknown_values() {
        for c in Confidence::ALL {
            assert_eq!(c.as_str().parse::<Confidence>().unwrap(), *c);
        }
        assert!("certain".parse::<Confidence>().is_err());
    }
}

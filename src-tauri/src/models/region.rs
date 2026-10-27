string_enum! {
    /// Kind of geographic region. Matches the `regions.region_type` CHECK constraint.
    pub enum RegionType {
        State => "state",
        Municipality => "municipality",
        Neighborhood => "neighborhood",
        District => "district",
        Condominium => "condominium",
        Corridor => "corridor",
        Other => "other",
    }
}

impl RegionType {
    /// Regions finer than a municipality: what a listing's location text can resolve to.
    #[must_use]
    pub const fn is_sub_municipal(self) -> bool {
        !matches!(self, Self::State | Self::Municipality)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn text_form_round_trips_and_rejects_unknown_values() {
        for t in RegionType::ALL {
            assert_eq!(t.as_str().parse::<RegionType>().unwrap(), *t);
        }
        assert!("galaxy".parse::<RegionType>().is_err());
    }

    #[test]
    fn only_regions_below_a_municipality_are_sub_municipal() {
        assert!(!RegionType::State.is_sub_municipal());
        assert!(!RegionType::Municipality.is_sub_municipal());
        assert!(RegionType::Neighborhood.is_sub_municipal());
        assert!(RegionType::Corridor.is_sub_municipal());
    }
}

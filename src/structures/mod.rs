//! Generic Sponge .schem structure stamping (non-tree models such as the construction crane).

pub mod boat;
pub mod car;
pub mod crane;
pub mod elizabeth_tower;
pub mod excavator;
pub mod fountain;
pub mod helicopter;
pub mod jetbridge;
pub mod lighthouse;
pub mod plane;
pub mod playground;
pub mod schematic;
pub mod starship;
pub mod tombstone;
pub mod tractor;
pub mod windturbine;

/// A family of bundled schematic props, as `--props` names it.
#[derive(Clone, Copy, PartialEq, Eq, Debug, clap::ValueEnum)]
pub enum Prop {
    Boat,
    Car,
    Crane,
    Excavator,
    Fountain,
    Helicopter,
    Jetbridge,
    Landmark,
    Lighthouse,
    Plane,
    Playground,
    Starship,
    Tombstone,
    Tractor,
    Windturbine,
}

/// The prop families a run places, one bit per `Prop`.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub struct PropSet(u16);

impl PropSet {
    pub const ALL: PropSet = PropSet((1 << (Prop::Windturbine as u16 + 1)) - 1);
    pub const NONE: PropSet = PropSet(0);

    pub fn has(self, prop: Prop) -> bool {
        self.0 & (1 << prop as u16) != 0
    }

    /// `all`, `none`, or a comma list of families.
    pub fn parse(spec: &str) -> Result<PropSet, String> {
        use clap::ValueEnum;
        match spec.trim() {
            "all" => return Ok(PropSet::ALL),
            "none" => return Ok(PropSet::NONE),
            _ => {}
        }
        let mut set = PropSet::NONE;
        for name in spec.split(',').map(str::trim).filter(|n| !n.is_empty()) {
            let prop = Prop::from_str(name, true).map_err(|_| {
                format!("{name}: not a prop family (all, none, or a list of boat, car, crane, excavator, fountain, helicopter, jetbridge, landmark, lighthouse, plane, playground, starship, tombstone, tractor, windturbine)")
            })?;
            set.0 |= 1 << prop as u16;
        }
        Ok(set)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use clap::ValueEnum;

    #[test]
    fn prop_set_parses_families() {
        assert_eq!(PropSet::parse("all"), Ok(PropSet::ALL));
        assert_eq!(PropSet::parse("none"), Ok(PropSet::NONE));
        let set = PropSet::parse("Car, landmark").unwrap();
        assert!(set.has(Prop::Car) && set.has(Prop::Landmark) && !set.has(Prop::Boat));
        assert!(PropSet::parse("car,submarine").is_err());
        // Every family has a bit inside ALL, and ALL names them all.
        assert!(Prop::value_variants().iter().all(|p| PropSet::ALL.has(*p)));
        assert_eq!(
            PropSet::ALL.0.count_ones() as usize,
            Prop::value_variants().len()
        );
    }
}

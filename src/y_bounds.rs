//! `--min-y` / `--max-y`: an explicit floor and ceiling for the extended-height Java world.
//! They set the `tall_range` an existing world's pack gives a run into it, so everything
//! that reads the world's bounds follows.

use crate::args::Args;
use crate::world_utils::TALL_DATAPACK_FULL_RANGE;

/// `--min-y` / `--max-y` describe a legal dimension and a world format that has one.
pub fn check(args: &Args) -> Result<(), String> {
    if args.min_y.is_none() && args.max_y.is_none() {
        return Ok(());
    }
    if !args.disable_height_limit || !args.java() {
        return Err(
            "--min-y/--max-y set the range of the extended build height: Java worlds only."
                .to_string(),
        );
    }
    // A One World's manifest fixes the build height for every area ever added to it.
    if args.one_world || args.units.coordinates() || args.units.one_world_unit.is_some() {
        return Err(
            "--min-y/--max-y do not combine with --one-world, --unit-regions or --one-world-workers: the world fixes its own build height."
                .to_string(),
        );
    }
    let (pack_min, pack_max) = TALL_DATAPACK_FULL_RANGE;
    if let Some(y) = args.min_y {
        let top = crate::world_editor::DEFAULT_MIN_Y;
        if y.rem_euclid(16) != 0 || !(pack_min..=top).contains(&y) {
            return Err(format!(
                "--min-y must be a multiple of 16 from {pack_min} to {top} (got {y})."
            ));
        }
    }
    if let Some(y) = args.max_y {
        let bottom = crate::world_editor::DEFAULT_MAX_Y;
        if y.rem_euclid(16) != 15 || !(bottom..=pack_max).contains(&y) {
            return Err(format!(
                "--max-y must end a section (16n - 1) from {bottom} to {pack_max} (got {y})."
            ));
        }
    }
    Ok(())
}

/// Make the flags the run's tall range; the side left out keeps the pack's limit. They imply
/// the extended build height, as the window's do.
pub fn apply(args: &mut Args) {
    if args.min_y.is_some() || args.max_y.is_some() {
        args.disable_height_limit = true;
        let (min, max) = TALL_DATAPACK_FULL_RANGE;
        args.tall_range = Some((args.min_y.unwrap_or(min), args.max_y.unwrap_or(max)));
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use clap::Parser;

    fn try_parse(extra: &[&str]) -> Result<Args, clap::Error> {
        let mut cmd = vec!["arnis", "--output-dir", ".", "--bbox", "1,2,3,4"];
        cmd.extend_from_slice(extra);
        Args::try_parse_from(cmd)
    }

    fn parse(extra: &[&str]) -> Args {
        try_parse(extra).unwrap()
    }

    #[test]
    fn min_y_must_be_multiple_of_16() {
        for bad in [-100, -2048, 0, -63] {
            let y = format!("--min-y={bad}");
            assert!(
                check(&parse(&["--disable-height-limit", &y])).is_err(),
                "{bad}"
            );
        }
        for ok in [-2032, -128, -64] {
            let y = format!("--min-y={ok}");
            assert!(
                check(&parse(&["--disable-height-limit", &y])).is_ok(),
                "{ok}"
            );
        }
        // The ceiling ends a section, and stays in the pack's limit.
        for bad in ["--max-y=448", "--max-y=303", "--max-y=2047"] {
            assert!(
                check(&parse(&["--disable-height-limit", bad])).is_err(),
                "{bad}"
            );
        }
        assert!(check(&parse(&["--disable-height-limit", "--max-y=447"])).is_ok());
    }

    #[test]
    fn max_y_conflicts_vanilla_height() {
        assert!(try_parse(&["--vanilla-height", "--max-y=447"]).is_err());
        assert!(try_parse(&["--vanilla-height", "--min-y=-128"]).is_err());
        // On the command line they imply the extended height.
        for flag in ["--max-y=447", "--min-y=-128"] {
            let mut args = parse(&[flag]);
            assert!(check(&args).is_err(), "{flag} before apply");
            apply(&mut args);
            assert!(args.disable_height_limit && check(&args).is_ok(), "{flag}");
        }
        for bad in [
            &["--disable-height-limit", "--bedrock", "--min-y=-128"][..],
            &["--disable-height-limit", "--luanti", "--max-y=447"],
            &["--disable-height-limit", "--one-world", "--min-y=-128"],
        ] {
            assert!(check(&parse(bad)).is_err(), "{bad:?}");
        }
    }

    #[test]
    fn bounds_feed_tall_range() {
        let mut args = parse(&["--disable-height-limit"]);
        apply(&mut args);
        assert_eq!(args.tall_range, None);
        assert_eq!(crate::ground::extended_min_y_for(&args), -2032);
        assert_eq!(crate::ground::world_top_y_for(&args), 2031);

        let mut args = parse(&["--disable-height-limit", "--min-y", "-128", "--max-y=447"]);
        assert!(check(&args).is_ok());
        apply(&mut args);
        assert_eq!(args.tall_range, Some((-128, 447)));
        assert_eq!(crate::ground::extended_min_y_for(&args), -128);
        assert_eq!(crate::ground::world_top_y_for(&args), 447);
        assert_eq!(crate::ground::extended_max_y_for(&args), 447);
        assert_eq!(crate::ground::min_ground_level_for(&args), -126);

        // One side alone keeps the other at the pack's limit.
        let mut floor_only = parse(&["--disable-height-limit", "--min-y=-512"]);
        apply(&mut floor_only);
        assert_eq!(crate::ground::world_top_y_for(&floor_only), 2031);

        // The ground level is bounded by the floor the flag sets.
        let mut low = parse(&[
            "--disable-height-limit",
            "--min-y=-128",
            "--ground-level=-200",
        ]);
        apply(&mut low);
        assert!(crate::args::validate_args(&low).is_err());
    }
}

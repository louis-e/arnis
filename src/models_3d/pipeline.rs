//! Orchestrates 3D-model substitution: 3DMR (external) → Wikidata (external) → custom archetypes.

use crate::args::Args;
use crate::models_3d::{custom, three_dmr, wikidata};
use crate::osm_parser::ProcessedElement;
use crate::world_editor::WorldEditor;
use std::collections::HashSet;

pub struct Models3dPipeline {
    three_dmr: three_dmr::PrescanResult,
    wikidata: wikidata::PrescanResult,
    stadium: custom::stadium::PrescanResult,
    union_suppressed: HashSet<(&'static str, u64)>,
}

impl Models3dPipeline {
    pub fn prescan(elements: &[ProcessedElement], args: &Args) -> Self {
        let three_dmr = three_dmr::prescan(elements, args.rotation);
        let wikidata = wikidata::prescan(
            elements,
            &three_dmr.suppressed_ids,
            args.rotation,
            args.scale,
        );

        let mut combined: HashSet<(&'static str, u64)> = HashSet::new();
        combined.extend(three_dmr.suppressed_ids.iter().copied());
        combined.extend(wikidata.suppressed_ids.iter().copied());

        let stadium = custom::stadium::prescan(elements, &combined, args.scale);
        combined.extend(stadium.suppressed_ids.iter().copied());

        Self {
            three_dmr,
            wikidata,
            stadium,
            union_suppressed: combined,
        }
    }

    pub fn suppressed(&self) -> &HashSet<(&'static str, u64)> {
        &self.union_suppressed
    }

    pub fn place(&self, editor: &mut WorldEditor, args: &Args) {
        if self.three_dmr.placement_count() > 0 {
            three_dmr::place_three_dmr_models(editor, args, &self.three_dmr);
        }
        if self.wikidata.placement_count() > 0 {
            wikidata::place_wikidata_models(editor, args, &self.wikidata);
        }
        if self.stadium.placement_count() > 0 {
            custom::stadium::place_stadium_models(editor, args, &self.stadium);
        }
    }
}

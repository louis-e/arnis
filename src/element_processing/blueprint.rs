//! Buildings in blueprint mode: their outline on the ground, and nothing else.
use crate::block_definitions::{Block, LIGHT_BLUE_CONCRETE};
use crate::bresenham::bresenham_line;
use crate::element_processing::buildings::is_underground_building;
use crate::osm_parser::{ProcessedMemberRole, ProcessedNode, ProcessedRelation, ProcessedWay};
use crate::world_editor::WorldEditor;

/// One block high on the ground, in a colour nothing else in a blueprint uses, so a builder
/// can follow it and later replace it in one go.
pub(crate) const OUTLINE: Block = LIGHT_BLUE_CONCRETE;

pub fn generate_outline(editor: &mut WorldEditor, way: &ProcessedWay) {
    if !is_underground_building(&way.tags) {
        draw_ring(editor, &way.nodes);
    }
}

/// Outer rings and courtyards. The parts of a building relation only repeat its outline.
pub fn generate_relation_outline(editor: &mut WorldEditor, relation: &ProcessedRelation) {
    if is_underground_building(&relation.tags) {
        return;
    }
    for member in &relation.members {
        if member.role != ProcessedMemberRole::Part {
            draw_ring(editor, &member.way.nodes);
        }
    }
}

fn draw_ring(editor: &mut WorldEditor, nodes: &[ProcessedNode]) {
    for pair in nodes.windows(2) {
        let (a, b) = (&pair[0], &pair[1]);
        for (x, _, z) in bresenham_line(a.x, 0, a.z, b.x, 0, b.z) {
            editor.set_block(OUTLINE, x, 1, z, None, None);
        }
    }
}

# Minecraft world assets

These files provide Java Edition world templates and world preview images.

| File | Purpose and use |
| --- | --- |
| [icon.png](icon.png) | Java world preview image, copied into the world directory by [`write_world_skeleton`](../../src/world_utils.rs). |
| [level.dat](level.dat) | Gzip-compressed NBT template for Java world metadata. [`write_world_skeleton`](../../src/world_utils.rs) updates the world name, `LastPlayed` timestamp, and player position and rotation before saving it. |
| [region.template](region.template) | Anvil (`.mca`) region template, not a standalone NBT file. [`create_region_file`](../../src/world_editor/java.rs) copies it for fresh non-void Java regions before writing generated chunks. The [world skeleton](../../src/world_utils.rs) can also copy it to `region/r.0.0.mca`. |
| [world_icon.jpeg](world_icon.jpeg) | Bedrock world preview image, included in the `.mcworld` archive by [`package_mcworld`](../../src/world_editor/bedrock.rs). |

Bedrock generates its own `level.dat` and chunk database in
[`BedrockWriter`](../../src/world_editor/bedrock.rs), rather than using the Java templates.

## Template origins

In the [maintainer's explanation in #384](https://github.com/louis-e/arnis/issues/384#issuecomment-2822487795),
Louis identifies `level.dat` as a template created with Minecraft 1.21.4.
For `region.template`, he recalls removing the blocks from a flat-world region
so that the template would retain Minecraft-generated `block_states` palette
and data structures. The issue does not provide an exact regeneration procedure
for that region template.
